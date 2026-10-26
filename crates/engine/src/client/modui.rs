//! Mod-built UI: mods describe nodes through the ABI, they become ordinary engine UI nodes.
//! Bindings and clicks call back into the mod with a read-only view of the client.

use std::rc::Rc;

use weft_abi::{self as abi, Callback, ModId, Str, ui_kind as uk};

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
    /// Kind of the menu whose screen this is, for its slot groups.
    menu: Option<u32>,
    /// The recipe a kind's `display` builds: (kind, number).
    recipe: Option<(String, u32)>,
    client: Option<&'a Client>,
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
    let menu = client.and_then(|c| c.menu.as_ref()).map(|m| m.kind);
    let mut b = UiBuild { tag: 1, owner, ui, lang, input, depth: 0, menu, recipe: None, client };
    let mut v = ClientView::new(client);
    let mut args = abi::UiArgs { ui: &mut b as *mut UiBuild as *mut abi::UiCtx, client: v.raw() };
    host::invoke(owner, cb, &mut args);
    for _ in 0..b.depth {
        b.ui.close();
    }
}

/// What mods add to place `area` of `screen`, in order, into the open node.
pub fn parts(ui: &mut Ui, lang: &Lang, input: Option<&crate::input::Input>, client: Option<&Client>, screen: &str, area: &str) {
    let Some(c) = crate::content::try_content() else { return };
    for p in c.game.screen_parts.iter().filter(|p| p.screen == screen && p.area == area) {
        build(ui, lang, input, p.owner, p.cb, client);
    }
}

/// One recipe through its kind's `display` callback.
#[allow(clippy::too_many_arguments)]
pub fn build_recipe(ui: &mut Ui, lang: &Lang, input: Option<&crate::input::Input>, owner: ModId, cb: Callback, client: Option<&Client>, kind: &str, recipe: u32) {
    let mut b = UiBuild { tag: 1, owner, ui, lang, input, depth: 0, menu: None, recipe: Some((kind.to_string(), recipe)), client };
    let mut v = ClientView::new(client);
    let mut args = abi::RecipeUiArgs { ui: &mut b as *mut UiBuild as *mut abi::UiCtx, client: v.raw(), kind: abi::Str::new(kind), recipe };
    host::invoke(owner, cb, &mut args);
    for _ in 0..b.depth {
        b.ui.close();
    }
}

fn args(list: abi::Slice<abi::UiArg>) -> Vec<Arg> {
    let mut out = Vec::new();
    unsafe {
        list.as_slice().iter().copied().for_each(|a| {
            out.push(match a.kind {
                abi::ui_arg::TEXT => Arg::Text(a.text.as_str().to_string()),
                abi::ui_arg::KEY => Arg::Key(a.text.as_str().to_string()),
                abi::ui_arg::INT => Arg::Int(a.num as i64),
                abi::ui_arg::FLOAT => Arg::Float(a.num, 1),
                abi::ui_arg::ITEM => Arg::Item(a.num as u32),
                k => {
                    crate::host::misuse("ui_text", || format!("no argument kind {k}"));
                    Arg::Text(String::new())
                }
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
            crate::host::misuse("ui_open", || "called outside a UI build".into());
            return;
        }
        let b = &mut *(u as *mut UiBuild);
        let n = &*node;
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
                abi::ui_align::CENTER => Align::Center,
                abi::ui_align::END => Align::End,
                abi::ui_align::STRETCH => Align::Stretch,
                _ => Align::Start,
            },
            justify: match n.justify {
                abi::ui_justify::CENTER => Justify::Center,
                abi::ui_justify::END => Justify::End,
                abi::ui_justify::BETWEEN => Justify::Between,
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
            let items = |name: &str| crate::content::try_content().and_then(|c| c.item_by_ref(name));
            let cx = text::Ctx { lang, input, item_name: &items };
            text::format(lang.get(key.as_str()), &args(a), &cx, &mut out);
            out
        };
        let id = match n.kind {
            uk::TEXT => {
                // Without a class the text takes the colors of the panel it sits on.
                let node = b.ui.text(spans(n.text, n.args), if class.is_empty() { "default" } else { &class });
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
                // Run after the input with the whole client API: a mod's menu can open screens,
                // worlds or quit like the engine's.
                let action: crate::ui::Action = Rc::new(move |ctx: &mut Ctx| ctx.requests.push(crate::app::Request::ModCall(owner, cb)));
                b.ui.open(Kind::Button { class: if class.is_empty() { "button".into() } else { class }, action: Some(action), enabled: true, selected: false }, style)
            }
            uk::SLOT => {
                // A group's slots as its grid, or one of them; size and silhouette from the group.
                let c = crate::content::content();
                let found = b.menu.and_then(|k| c.game.menus.get(k as usize)).and_then(|d| d.group(&c.game.player_layout, &class));
                match found {
                    None => {
                        crate::host::misuse("ui slots", || format!("the menu has no slot group {class}"));
                        b.ui.open(Kind::Container { class: None }, style)
                    }
                    Some((g, first, count)) => {
                        let cols = g.span(&c.game.player_layout).2;
                        let slot = |ui: &mut Ui, i: u32| {
                            let s = ui.add(Kind::Custom { tag: TAG_SLOT, data: (first + i) as u64 }, Style { w: Len::Px(g.size), h: Len::Px(g.size), ..Style::default() });
                            ui.nodes[s].icon = g.background.clone();
                        };
                        if n.slot != u32::MAX {
                            let id = b.ui.open(Kind::Container { class: None }, style);
                            slot(b.ui, n.slot.min(count.saturating_sub(1)));
                            id
                        } else {
                            let id = b.ui.open(Kind::Container { class: None }, style);
                            for row in 0..count.div_ceil(cols) {
                                b.ui.open(Kind::Container { class: None }, Style::row());
                                for i in row * cols..(row * cols + cols).min(count) {
                                    slot(b.ui, i);
                                }
                                b.ui.close();
                            }
                            id
                        }
                    }
                }
            }
            uk::RECIPE_SLOT => {
                // A recipe's inputs or outputs in a kind's own display.
                let c = crate::content::content();
                let id = b.ui.open(Kind::Container { class: None }, style);
                let found = b.recipe.as_ref().and_then(|(k, i)| c.game.recipes.get(k).and_then(|l| l.get(*i as usize)).map(|r| (k.clone(), r)));
                match (found, input) {
                    (Some((kind, r)), Some(input)) if n.slot == u32::MAX => crate::game_screens::recipe_grid(b.ui, lang, input, &kind, r, n.data == 1),
                    (Some((_, r)), Some(input)) => {
                        use crate::registry::Ingredient;
                        let (ing, count, chance) = if n.data == 1 {
                            r.outputs.get(n.slot as usize).map_or((Ingredient::Empty, 0, 1.0), |o| (o.what.clone(), o.count, o.chance))
                        } else {
                            (r.inputs.get(n.slot as usize).cloned().unwrap_or(Ingredient::Empty), r.input_counts.get(n.slot as usize).copied().unwrap_or(1), 1.0)
                        };
                        crate::game_screens::recipe_slot(b.ui, lang, input, &ing, count, chance);
                    }
                    _ => crate::host::misuse("ui recipe slots", || "only inside a recipe kind's display".into()),
                }
                id
            }
            uk::PARTS => {
                let id = b.ui.open(Kind::Container { class: None }, style);
                let (screen, area) = (class.clone(), n.text.as_str().to_string());
                parts(b.ui, lang, input, b.client, &screen, &area);
                id
            }
            uk::PROGRESS => {
                let bind = if n.bind.is_some() { float_bind(owner, n.bind) } else { Box::new(|_: &Ctx| 0.0f32) as Box<dyn Fn(&Ctx) -> f32> };
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
            crate::host::misuse("ui_close", || "called outside a UI build".into());
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
            crate::host::misuse("ui_text", || "called outside a text binding".into());
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
                size: { let (hw, h, eye) = cl.player.dims(); [hw as f32 * 2.0, h as f32, eye as f32] },
                ..Default::default()
            };
            *out = state;
            return 1;
        }
        match cl.entities.get(&id) {
            Some(e) => {
                let state = abi::EntityState {
                    kind: e.kind,
                    pos: e.pos.to_array(),
                    yaw: e.yaw,
                    pitch: e.pitch,
                    moved: e.swing,
                    health: e.health,
                    // The kind's own value: clients do not see attribute modifiers of others.
                    max_health: {
                        let attr = &content.game.attributes[crate::server::MAX_HEALTH as usize];
                        attr.comp.and_then(|id| content.components.get::<f64>(crate::components::ENTITY, e.kind, id)).unwrap_or(attr.default) as f32
                    },
                    size: { let (hw, h, eye) = e.dims(); [hw as f32 * 2.0, h as f32, eye as f32] },
                    ..Default::default()
                };
                *out = state;
                1
            }
            None => 0,
        }
    }
}

pub unsafe extern "C" fn client_creative(c: *mut abi::ClientCtx) -> u32 {
    unsafe { view(c).is_some_and(|cl| cl.mode.invulnerable()) as u32 }
}

