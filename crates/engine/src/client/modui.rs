//! Mod-built UI: mods describe nodes through the ABI, they become ordinary engine UI nodes.
//! Bindings and clicks call back into the mod with a read-only view of the client.

use std::rc::Rc;

use voxide_abi::{self as abi, Callback, ModId, Str, ui_kind as uk};

use super::Client;
use crate::app::Ctx;
use crate::host;
use crate::ui::lang::Lang;
use crate::ui::text::{self, Arg};
use crate::ui::{Align, Justify, Kind, Len, Style, Ui};

pub const TAG_SLOT: u32 = 10;
pub const TAG_ENTITY: u32 = 11;
/// Creative inventory entry, `data` = item id.
pub const TAG_CREATIVE: u32 = 12;
/// Just an item icon, `data` = item id.
pub const TAG_ICON: u32 = 13;

#[repr(C)]
struct UiBuild<'a> {
    tag: u32,
    owner: ModId,
    ui: &'a mut Ui,
    lang: &'a Lang,
    input: Option<&'a crate::input::Input>,
    depth: u32,
}

#[repr(C)]
struct TextSink {
    tag: u32,
    out: Option<(String, Vec<Arg>)>,
}

/// What `ClientCtx` points at. `run` is set only for callbacks that may drive the client (frame
/// hooks, packets), see `app::api`.
#[repr(C)]
pub struct ClientView {
    pub client: *const Client,
    pub run: *mut std::ffi::c_void,
}

impl ClientView {
    pub fn new(client: Option<&Client>) -> ClientView {
        ClientView { client: client.map_or(std::ptr::null(), |c| c as *const Client), run: std::ptr::null_mut() }
    }

    pub fn raw(&mut self) -> *mut abi::ClientCtx {
        self as *mut ClientView as *mut abi::ClientCtx
    }
}

unsafe fn view<'a>(c: *mut abi::ClientCtx) -> Option<&'a Client> {
    unsafe { (*(c as *mut ClientView)).client.as_ref() }
}

/// Runs a mod's UI builder callback into `ui`.
pub fn build(ui: &mut Ui, lang: &Lang, input: Option<&crate::input::Input>, owner: ModId, cb: Callback, client: Option<&Client>) {
    let mut b = UiBuild { tag: 1, owner, ui, lang, input, depth: 0 };
    let mut v = ClientView::new(client);
    let mut args = abi::UiArgs { ui: &mut b as *mut UiBuild as *mut abi::UiCtx, client: v.raw() };
    host::invoke(owner, cb, &mut args);
    for _ in 0..b.depth {
        b.ui.close();
    }
}

fn args(list: abi::Slice<abi::UiArg>) -> Vec<Arg> {
    let mut out = Vec::new();
    unsafe {
        abi::each_sized(list, |a| {
            out.push(match a.kind {
                0 => Arg::Text(a.text.as_str().to_string()),
                1 => Arg::Key(a.text.as_str().to_string()),
                2 => Arg::Int(a.num as i64),
                3 => Arg::Float(a.num, 1),
                _ => Arg::Item(a.num as u32),
            })
        })
    };
    out
}

fn float_bind(owner: ModId, cb: Callback) -> Box<dyn Fn(&Ctx) -> f32> {
    Box::new(move |ctx: &Ctx| {
        let mut v = ClientView::new(ctx.client.as_deref());
        let mut sink = TextSink { tag: 2, out: None };
        let mut a = abi::UiBindArgs { client: v.raw(), value: 0.0, text: &mut sink as *mut TextSink as *mut abi::UiCtx };
        host::invoke(owner, cb, &mut a);
        a.value
    })
}

pub unsafe extern "C" fn ui_open(u: *mut abi::UiCtx, node: *const abi::UiNode) {
    unsafe {
        if u.is_null() || *(u as *const u32) != 1 {
            return;
        }
        let b = &mut *(u as *mut UiBuild);
        let n = &abi::read_sized(node);
        let owner = b.owner;
        let class = n.class.as_str().to_string();
        let len = |v: f32| if v.is_nan() { Len::Auto } else { Len::Px(v) };
        let style = Style {
            row: n.row != 0,
            pad: n.pad,
            gap: n.gap,
            w: len(n.width),
            h: len(n.height),
            grow: n.grow,
            align: match n.align {
                1 => Align::Center,
                2 => Align::End,
                3 => Align::Stretch,
                _ => Align::Start,
            },
            justify: match n.justify {
                1 => Justify::Center,
                2 => Justify::End,
                3 => Justify::Between,
                _ => Justify::Start,
            },
            ..Style::default()
        };
        let (lang, input) = (b.lang, b.input);
        let spans = |key: Str, a: abi::Slice<abi::UiArg>| {
            let mut out = Vec::new();
            if key.len == 0 {
                return out;
            }
            let items = |name: &str| crate::content::try_content().and_then(|c| c.item_names.get(name).copied());
            let cx = text::Ctx { lang, input, item_name: &items };
            text::format(lang.get(key.as_str()), &args(a), &cx, &mut out);
            out
        };
        let id = match n.kind {
            uk::TEXT => {
                let node = b.ui.text(spans(n.text, n.args), if class.is_empty() { "label" } else { &class });
                if n.bind.is_some() {
                    let cb = n.bind;
                    if let Kind::Text(t) = &mut b.ui.nodes[node].kind {
                        t.bind = Some(Box::new(move |ctx: &Ctx| {
                            let mut v = ClientView::new(ctx.client.as_deref());
                            let mut sink = TextSink { tag: 2, out: None };
                            let mut a = abi::UiBindArgs { client: v.raw(), value: 0.0, text: &mut sink as *mut TextSink as *mut abi::UiCtx };
                            host::invoke(owner, cb, &mut a);
                            sink.out.unwrap_or_default()
                        }));
                    }
                }
                b.ui.nodes[node].style = Style { ..style };
                b.ui.open_existing(node);
                node
            }
            uk::BUTTON => {
                let cb = n.on_click;
                let action: crate::ui::Action = Rc::new(move |ctx: &mut Ctx| {
                    let mut v = ClientView::new(ctx.client.as_deref());
                    let mut a = abi::UiArgs { ui: std::ptr::null_mut(), client: v.raw() };
                    host::invoke(owner, cb, &mut a);
                });
                b.ui.open(Kind::Button { class: if class.is_empty() { "button".into() } else { class }, action: Some(action), enabled: true, selected: false }, style)
            }
            uk::SLOT => {
                // Width set: a bigger slot (crafting results), text: the empty slot's silhouette.
                let size = if n.width.is_nan() { 18.0 } else { n.width };
                let id = b.ui.open(Kind::Custom { tag: TAG_SLOT, data: n.slot as u64 }, Style { w: Len::Px(size), h: Len::Px(size), ..style });
                if n.text.len > 0 {
                    b.ui.nodes[id].icon = Some(n.text.as_str().to_string());
                }
                id
            }
            uk::PROGRESS => {
                let bind: Box<dyn Fn(&Ctx) -> f32> = if n.bind.is_some() {
                    float_bind(owner, n.bind)
                } else {
                    let (data, data_max, max) = (n.data, n.data_max, n.max);
                    Box::new(move |ctx: &Ctx| {
                        let Some(m) = ctx.client.as_ref().and_then(|c| c.menu.as_ref()) else { return 0.0 };
                        let v = m.data.get(data as usize).copied().unwrap_or(0) as f32;
                        let top = if data_max != u32::MAX { m.data.get(data_max as usize).copied().unwrap_or(0) as f32 } else { max };
                        if top > 0.0 { v / top } else { 0.0 }
                    })
                };
                b.ui.open(Kind::Progress { class: if class.is_empty() { "progress".into() } else { class }, bind, value: 0.0, dir: n.dir }, style)
            }
            uk::ICONS => {
                let bind = if n.bind.is_some() { float_bind(owner, n.bind) } else { Box::new(|_: &Ctx| 0.0f32) as Box<dyn Fn(&Ctx) -> f32> };
                b.ui.open(Kind::Icons { class, count: n.count, bind, value: -1.0, rtl: n.data == 1 }, style)
            }
            uk::IMAGE => b.ui.open(Kind::Image { texture: n.text.as_str().to_string(), color: 0xFFFFFFFF }, style),
            uk::ENTITY => b.ui.open(Kind::Custom { tag: TAG_ENTITY, data: 0 }, style),
            uk::SPACER => b.ui.open(Kind::Container { class: None }, Style { grow: n.grow.max(1.0), ..style }),
            uk::PANEL => b.ui.open(Kind::Container { class: Some(if class.is_empty() { "panel".into() } else { class }) }, style),
            _ => b.ui.open(Kind::Container { class: (!class.is_empty()).then_some(class) }, style),
        };
        if n.tooltip.len > 0 {
            b.ui.nodes[id].tooltip = Some(spans(n.tooltip, abi::Slice::EMPTY));
        }
        b.depth += 1;
    }
}

pub unsafe extern "C" fn ui_close(u: *mut abi::UiCtx) {
    unsafe {
        if u.is_null() || *(u as *const u32) != 1 {
            return;
        }
        let b = &mut *(u as *mut UiBuild);
        if b.depth > 0 {
            b.depth -= 1;
            b.ui.close();
        }
    }
}

pub unsafe extern "C" fn ui_text(u: *mut abi::UiCtx, key: Str, a: abi::Slice<abi::UiArg>) {
    unsafe {
        if u.is_null() || *(u as *const u32) != 2 {
            return;
        }
        let s = &mut *(u as *mut TextSink);
        s.out = Some((key.as_str().to_string(), args(a)));
    }
}

pub unsafe extern "C" fn client_player(c: *mut abi::ClientCtx) -> u64 {
    unsafe { view(c).map_or(0, |cl| cl.player_id) }
}

pub unsafe extern "C" fn client_component(c: *mut abi::ClientCtx, id: u64, comp: u32) -> *const u8 {
    unsafe {
        match view(c) {
            Some(cl) if id == cl.player_id || id == 0 => cl.components.get(&comp).map_or(std::ptr::null(), |v| v.as_ptr()),
            // Other entities' components the server shares (`SYNC_WATCHERS`, `SYNC_MENU`).
            Some(cl) => cl.entity_components.get(&(id, comp)).map_or(std::ptr::null(), |v| v.as_ptr()),
            None => std::ptr::null(),
        }
    }
}

pub unsafe extern "C" fn client_menu_owner(c: *mut abi::ClientCtx) -> u64 {
    unsafe { view(c).and_then(|cl| cl.menu.as_ref()).map_or(0, |m| m.owner) }
}

pub unsafe extern "C" fn client_entity(c: *mut abi::ClientCtx, id: u64, out: *mut abi::EntityState) -> u32 {
    unsafe {
        let Some(cl) = view(c) else { return 0 };
        let content = crate::content::content();
        if id == cl.player_id || id == 0 {
            let state = abi::EntityState {
                kind: content.game.player_kind,
                pos: cl.player.pos.to_array(),
                vel: cl.player.vel.to_array(),
                yaw: cl.player.yaw,
                pitch: cl.player.pitch,
                on_ground: cl.player.on_ground as u32,
                in_liquid: cl.player.in_liquid as u32,
                health: cl.health,
                max_health: cl.max_health,
                ..Default::default()
            };
            abi::write_sized(out, &state);
            return 1;
        }
        match cl.entities.get(&id) {
            Some(e) => {
                let state = abi::EntityState {
                    kind: e.kind,
                    pos: e.pos.to_array(),
                    yaw: e.yaw,
                    pitch: e.pitch,
                    health: e.health,
                    max_health: content.game.entities[e.kind as usize].max_health,
                    ..Default::default()
                };
                abi::write_sized(out, &state);
                1
            }
            None => 0,
        }
    }
}

pub unsafe extern "C" fn client_creative(c: *mut abi::ClientCtx) -> u32 {
    unsafe { view(c).is_some_and(|cl| cl.mode.invulnerable()) as u32 }
}

pub unsafe extern "C" fn client_menu_data(c: *mut abi::ClientCtx, index: u32) -> i64 {
    unsafe { view(c).and_then(|cl| cl.menu.as_ref()).and_then(|m| m.data.get(index as usize).copied()).unwrap_or(0) }
}
