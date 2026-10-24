//! In-game overlay: crosshair, hotbar, debug lines, chat, and HUD parts registered by mods.
//! Rebuilt only when what it shows changes; mod parts update through their bindings.

use voxide_abi::ItemStack;

use crate::app::Ctx;
use crate::client::Client;
use crate::client::modui::{self, TAG_ENTITY, TAG_SLOT};
use crate::client::textures::GameTextures;
use crate::content::content;
use crate::ui::lang::Lang;
use crate::ui::render::UiRenderer;
use crate::ui::text::{self, Arg};
use crate::ui::theme::Theme;
use crate::ui::{Align, Justify, Kind, Rect, Style, Ui};

pub const TAG_HOTBAR: u32 = 1;
pub const TAG_CROSSHAIR: u32 = 2;

#[derive(Default)]
pub struct Hud {
    pub ui: Ui,
    key: Option<(bool, usize, usize, bool, u32, u8)>,
    /// Player overrides from `config/hud.txt`: element name -> (anchor, order).
    layout: rustc_hash::FxHashMap<String, (String, i32)>,
}

impl Hud {
    /// Reads `config/hud.txt`: lines `element = anchor [order]`, e.g. `base:food = top_right`.
    pub fn load_layout(&mut self, run_dir: &std::path::Path) {
        self.layout.clear();
        let src = std::fs::read_to_string(run_dir.join("config").join("hud.txt")).unwrap_or_default();
        for line in src.lines() {
            let Some((name, v)) = line.split_once('=') else { continue };
            let mut it = v.split_whitespace();
            let Some(anchor) = it.next() else { continue };
            let order = it.next().and_then(|o| o.parse().ok()).unwrap_or(0);
            self.layout.insert(name.trim().to_string(), (anchor.to_string(), order));
        }
        self.key = None;
    }

    pub fn invalidate(&mut self) {
        self.key = None;
    }

    /// Returns true if the tree was rebuilt.
    /// `chat_open`: the chat screen shows the messages itself.
    /// `groups`: bit per `DEBUG_GROUPS` entry shown with the debug overlay.
    pub fn update(&mut self, client: &Client, debug: bool, groups: u32, chat_open: bool, lang: &Lang, input: &crate::input::Input) -> bool {
        let chat_visible = if chat_open { 0 } else { client.chat.iter().filter(|(_, t)| t.elapsed().as_secs() < 10).count() };
        let key = (debug, chat_visible, client.chat.len(), client.dead, groups, client.mode as u8);
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        let c = content();
        // Elements per anchor, mod parts plus the hotbar itself, with the player's overrides.
        let mut placed: Vec<(String, i32, Option<(voxide_abi::ModId, voxide_abi::Callback)>)> =
            vec![("core:hotbar".into(), 0, None)].into_iter().chain(c.game.huds.iter().map(|h| (h.name.clone(), h.order, Some((h.owner, h.cb))))).collect::<Vec<_>>()
                .into_iter()
                .map(|(name, order, cb)| {
                    let default = if cb.is_none() { voxide_abi::hud_anchor::HOTBAR.to_string() } else { c.game.huds.iter().find(|h| h.name == name).map(|h| h.anchor.clone()).unwrap_or_default() };
                    let (anchor, order) = self.layout.get(&name).cloned().unwrap_or((default, order));
                    (anchor, order, cb)
                })
                .collect();
        placed.sort_by_key(|p| p.1);
        let at = |anchor: &str, upwards: bool| -> Vec<Option<(voxide_abi::ModId, voxide_abi::Callback)>> {
            let mut v: Vec<_> = placed.iter().filter(|p| p.0 == anchor).map(|p| p.2).collect();
            if upwards {
                v.reverse();
            }
            v
        };
        let draw = |ui: &mut Ui, part: Option<(voxide_abi::ModId, voxide_abi::Callback)>| match part {
            Some((owner, cb)) => modui::build(ui, lang, Some(input), owner, cb, Some(client)),
            None if client.mode == crate::world::GameMode::Spectator => {}
            None => {
                ui.container(Style::row().gap(0.0), |ui| {
                    for i in 0..9 {
                        ui.add(Kind::Custom { tag: TAG_HOTBAR, data: i }, Style::default().size(20.0, 20.0));
                    }
                });
            }
        };
        use voxide_abi::hud_anchor as a;
        let ui = &mut self.ui;
        ui.clear();
        ui.root_style(Style::default().stretch());
        // Both halves start at zero height and split the rest evenly, so the crosshair sits at the
        // exact screen centre whatever they contain.
        ui.container(Style::row().h(0.0).grow().pad(4.0).align(Align::Start), |ui| {
            ui.container(Style::default().gap(1.0).align(Align::Start), |ui| {
                if debug {
                    debug_groups(ui, groups, false);
                }
                for p in at(a::TOP_LEFT, false) {
                    draw(ui, p);
                }
            });
            ui.container(Style::default().grow(), |_| {});
            ui.container(Style::default().gap(1.0).align(Align::End), |ui| {
                if debug {
                    debug_groups(ui, groups, true);
                }
                for p in at(a::TOP_RIGHT, false) {
                    draw(ui, p);
                }
            });
        });
        ui.add(Kind::Custom { tag: TAG_CROSSHAIR, data: 0 }, Style::default().size(9.0, 9.0).self_align(Align::Center));
        ui.container(Style::default().h(0.0).grow().justify(Justify::End).align(Align::Stretch).pad(2.0).gap(1.0), |ui| {
            ui.container(Style::default().gap(1.0).pad(2.0).align(Align::Start).self_align(Align::Start), |ui| {
                let now = std::time::Instant::now();
                for (line, at) in client.chat.iter().rev().take(10).rev() {
                    if chat_visible > 0 && now.duration_since(*at).as_secs() < 10 {
                        ui.text(text::plain(line), "chat");
                    }
                }
            });
            ui.container(Style::row().align(Align::End), |ui| {
                ui.container(Style::default().w(0.0).gap(1.0).align(Align::Start), |ui| {
                    for p in at(a::BOTTOM_LEFT, true) {
                        draw(ui, p);
                    }
                });
                ui.container(Style::default().grow(), |_| {});
                ui.container(Style::default().gap(1.0).align(Align::Center), |ui| {
                    ui.container(Style::row().w(182.0).gap(2.0).align(Align::End), |ui| {
                        ui.container(Style::default().gap(1.0).align(Align::Start), |ui| {
                            for p in at(a::HOTBAR_LEFT, true) {
                                draw(ui, p);
                            }
                        });
                        ui.container(Style::default().grow(), |_| {});
                        ui.container(Style::default().gap(1.0).align(Align::End), |ui| {
                            for p in at(a::HOTBAR_RIGHT, true) {
                                draw(ui, p);
                            }
                        });
                    });
                    for p in at(a::HOTBAR, true) {
                        draw(ui, p);
                    }
                });
                ui.container(Style::default().grow(), |_| {});
                ui.container(Style::default().w(0.0).gap(1.0).align(Align::End), |ui| {
                    for p in at(a::BOTTOM_RIGHT, true) {
                        draw(ui, p);
                    }
                });
            });
        });
        true
    }
}


type DebugLine = (&'static str, fn(&Ctx) -> Vec<Arg>);

/// Debug overlay groups in F3+number order: title key, right side, lines.
pub const DEBUG_GROUPS: &[(&str, bool, &[DebugLine])] = &[
    ("hud.debug.group.performance", true, PERFORMANCE),
    ("hud.debug.group.position", false, POSITION),
    ("hud.debug.group.world", false, WORLD),
    ("hud.debug.group.mods", true, MODS),
    ("hud.debug.group.system", true, SYSTEM),
];

const PERFORMANCE: &[DebugLine] = &[
    ("hud.debug.fps", |c| vec![Arg::Int(c.stats.fps.round() as i64), Arg::Float(c.stats.frame_ms as f64, 2)]),
    ("hud.debug.cpu", |c| c.stats.cpu.iter().map(|v| Arg::Float(*v as f64, 2)).collect()),
    ("hud.debug.gpu", |c| {
        let g = &c.stats.gpu;
        let at = |i: usize| Arg::Float(g.get(i).copied().unwrap_or(0.0) as f64, 2);
        vec![Arg::Float(g.iter().sum::<f32>() as f64, 2), at(1), at(2), at(3)]
    }),
    ("hud.debug.server", |c| vec![Arg::Float(c.stats.server_ms as f64, 2), Arg::Int(c.stats.regions as i64)]),
    ("hud.debug.phases", |c| {
        let p = c.stats.phases;
        vec![Arg::Float(p[0] as f64, 2), Arg::Float(p[2] as f64, 2), Arg::Float((p[3] + p[4]) as f64, 2)]
    }),
];

const POSITION: &[DebugLine] = &[
    ("hud.debug.pos", |c| {
        let p = c.client.as_ref().map_or(glam::DVec3::ZERO, |cl| cl.player.pos);
        vec![Arg::Float(p.x, 3), Arg::Float(p.y, 3), Arg::Float(p.z, 3)]
    }),
    ("hud.debug.block", |c| {
        let p = c.client.as_ref().map_or(glam::IVec3::ZERO, |cl| cl.player.pos.floor().as_ivec3());
        vec![Arg::Int(p.x as i64), Arg::Int(p.y as i64), Arg::Int(p.z as i64)]
    }),
    ("hud.debug.cube", |c| {
        let p = c.client.as_ref().map_or(glam::IVec3::ZERO, |cl| cl.player.pos.floor().as_ivec3() >> 4i32);
        vec![Arg::Int(p.x as i64), Arg::Int(p.y as i64), Arg::Int(p.z as i64)]
    }),
    ("hud.debug.facing", |c| {
        let (yaw, pitch) = c.client.as_ref().map_or((0.0, 0.0), |cl| (cl.player.yaw, cl.player.pitch));
        // Yaw 0 looks along -Z (north); turning right goes to east (+X).
        let deg = (-yaw.to_degrees()).rem_euclid(360.0);
        let dir = ["north", "east", "south", "west"][(((deg + 45.0) / 90.0) as usize) % 4];
        vec![Arg::Key(format!("hud.debug.dir.{dir}")), Arg::Float(deg as f64, 1), Arg::Float(pitch.to_degrees() as f64, 1)]
    }),
    ("hud.debug.biome", |c| {
        let Some(cl) = c.client.as_ref() else { return vec![Arg::Text("-".into())] };
        let p = cl.player.pos.floor().as_ivec3();
        let id = cl.world.cubes.get(&(p >> 4i32)).map_or(0, |cube| cube.biome((p.x & 15) as usize, (p.z & 15) as usize));
        vec![Arg::Text(content().biomes.get(id as usize).map_or("-".into(), |b| b.name.clone()))]
    }),
    ("hud.debug.dimension", |c| {
        let d = c.client.as_ref().map_or(0, |cl| cl.world.dim);
        vec![Arg::Text(content().dims[d as usize].name.clone())]
    }),
    ("hud.debug.light", |c| {
        let l = c.client.as_ref().map_or(0, |cl| cl.world.light((cl.player.pos + glam::DVec3::Y * 0.5).floor().as_ivec3()));
        let ch = |s: u32| Arg::Int(((l >> s) & 255) as i64);
        vec![ch(0), ch(8), ch(16), ch(24)]
    }),
    ("hud.debug.looking", |c| {
        let name = c.client.as_ref().and_then(|cl| cl.target).map(|h| content().state_name(h.state)).unwrap_or_else(|| "-".into());
        vec![Arg::Text(name)]
    }),
];

const WORLD: &[DebugLine] = &[
    ("hud.debug.world", |c| {
        let Some(cl) = c.client.as_ref() else { return vec![] };
        vec![Arg::Int(cl.world.cubes.len() as i64), Arg::Int(cl.world.pending_meshes() as i64)]
    }),
    ("hud.debug.draws", |c| {
        let Some(cl) = c.client.as_ref() else { return vec![] };
        vec![Arg::Int(cl.render.drawn as i64), Arg::Int((cl.render.faces_drawn / 1000) as i64)]
    }),
    ("hud.debug.entities", |c| vec![Arg::Int(c.client.as_ref().map_or(0, |cl| cl.entities.len()) as i64)]),
    ("hud.debug.load", |c| vec![Arg::Int(c.stats.load.0 as i64), Arg::Int(c.stats.load.1 as i64)]),
];

const MODS: &[DebugLine] = &[("core.raw", |_| vec![Arg::Text(mod_profile())])];

const SYSTEM: &[DebugLine] = &[
    ("hud.debug.gpu_name", |c| vec![Arg::Text(c.stats.gpu_name.clone())]),
    ("hud.debug.cpu_name", |c| vec![Arg::Text(c.stats.cpu_name.clone()), Arg::Int(c.stats.threads as i64)]),
    ("hud.debug.window", |c| vec![Arg::Int(c.stats.window.0 as i64), Arg::Int(c.stats.window.1 as i64), Arg::Float(c.stats.gui_scale as f64, 1)]),
];

/// Milliseconds per server tick and calls per tick for each mod, averaged over the last second.
fn mod_profile() -> String {
    use std::sync::Mutex;
    static LAST: Mutex<Option<(std::time::Instant, String, crate::diag::ModWindow)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    let (at, text, window) = last.get_or_insert_with(|| (std::time::Instant::now() - std::time::Duration::from_secs(2), String::new(), Default::default()));
    if at.elapsed().as_secs_f32() < 1.0 {
        return text.clone();
    }
    let (mods, secs) = window.take();
    let ticks = (secs * 20.0).max(1.0);
    let lines: Vec<String> = mods.iter().map(|(id, ns, calls)| format!("{id}: {:.2} ms, {:.0} calls", *ns as f64 / 1e6 / ticks, *calls as f64 / ticks)).collect();
    *text = lines.join("
");
    *at = std::time::Instant::now();
    text.clone()
}

/// Engine-drawn nodes: crosshair, hotbar slots, menu slots.
#[allow(clippy::too_many_arguments)]
pub fn draw_custom(
    ren: &mut UiRenderer,
    theme: &Theme,
    textures: &GameTextures,
    client: Option<&Client>,
    tag: u32,
    data: u64,
    r: Rect,
    hovered: bool,
    icon: Option<u32>,
    scale: f32,
    text_unit: f32,
) {
    match tag {
        TAG_CROSSHAIR => {
            let c = Rect { x: r.x + r.w / 2.0 - scale / 2.0, y: r.y, w: scale, h: r.h };
            ren.rect(c, 0xFFFFFFC0);
            ren.rect(Rect { x: r.x, y: r.y + r.h / 2.0 - scale / 2.0, w: r.w, h: scale }, 0xFFFFFFC0);
        }
        TAG_HOTBAR => {
            let Some(client) = client else { return };
            let selected = client.selected as u64 == data;
            slot_background(ren, theme, r, selected, scale);
            let stack = client.inventory.get(data as usize).copied().unwrap_or_default();
            stack_icon(ren, textures, r, stack, scale, text_unit);
        }
        TAG_ENTITY => {
            // Inset frame; the model itself is drawn in a later pass.
            ren.rect(r, 0x373737FF);
            ren.rect(Rect { x: r.x + scale, y: r.y + scale, w: r.w - scale, h: r.h - scale }, 0xFFFFFFFF);
            ren.rect(Rect { x: r.x + scale, y: r.y + scale, w: r.w - 2.0 * scale, h: r.h - 2.0 * scale }, 0x000000FF);
        }
        modui::TAG_ICON => stack_icon(ren, textures, r, ItemStack::new(data as u32, 1), scale, text_unit),
        modui::TAG_CREATIVE => {
            slot_background(ren, theme, r, false, scale);
            stack_icon(ren, textures, r, ItemStack::new(data as u32, 1), scale, text_unit);
            if hovered {
                ren.rect(Rect { x: r.x + scale, y: r.y + scale, w: r.w - 2.0 * scale, h: r.h - 2.0 * scale }, 0xFFFFFF50);
            }
        }
        TAG_SLOT => {
            let Some(client) = client else { return };
            slot_background(ren, theme, r, false, scale);
            let stack = client.menu.as_ref().and_then(|m| m.slots.get(data as usize)).copied().unwrap_or_default();
            if let (Some(t), true) = (icon, stack.is_empty()) {
                let s = 16.0 * scale;
                ren.image(Rect { x: r.x + (r.w - s) / 2.0, y: r.y + (r.h - s) / 2.0, w: s, h: s }, t, [0.0, 0.0, 1.0, 1.0], 0xFFFFFFFF);
            }
            stack_icon(ren, textures, r, stack, scale, text_unit);
            let dragged = client.drag.as_ref().is_some_and(|d| d.1.contains(&(data as u32)));
            if hovered || dragged {
                ren.rect(Rect { x: r.x + scale, y: r.y + scale, w: r.w - 2.0 * scale, h: r.h - 2.0 * scale }, 0xFFFFFF50);
            }
        }
        _ => {}
    }
}

/// Item icon centred in `r`, with count and durability bar.
pub fn stack_icon(ren: &mut UiRenderer, textures: &GameTextures, r: Rect, stack: ItemStack, scale: f32, text_unit: f32) {
    if stack.is_empty() {
        return;
    }
    let c = content();
    let s = 16.0 * scale;
    let (x, y) = (r.x + (r.w - s) / 2.0, r.y + (r.h - s) / 2.0);
    ren.item_icon(c, textures, x, y, s, stack.item);
    let def = &c.items[stack.item as usize];
    if def.durability > 0 && stack.damage > 0 {
        let f = 1.0 - stack.damage as f32 / def.durability as f32;
        let bar = Rect { x: x + scale, y: y + s - 2.0 * scale, w: 14.0 * scale, h: 2.0 * scale };
        ren.rect(bar, 0x000000FF);
        let g = (f * 255.0) as u32;
        let col = ((255 - g) << 24) | (g << 16) | 0xFF;
        ren.rect(Rect { w: 13.0 * scale * f, h: scale, ..bar }, col);
    }
    if stack.count > 1 {
        let px = scale * text_unit;
        let spans = text::plain(&stack.count.to_string());
        let layout = text::layout(&spans, &ren.font, None);
        let tx = x + s - layout.width * px + scale;
        let ty = y + s - 16.0 * px + scale;
        let mut icons = Vec::new();
        ren.glyph_run(&layout.items, tx, ty, px, 0xFFFFFFFF, Some(0x3F3F3FFF), &mut icons);
    }
}

pub fn slot_background(ren: &mut UiRenderer, theme: &Theme, r: Rect, selected: bool, scale: f32) {
    ren.rect(r, theme.color("slot", "fill", 0x8B8B8BFF));
    let dark = theme.color("slot", "dark", 0x373737FF);
    let light = theme.color("slot", "light", 0xFFFFFFFF);
    ren.rect(Rect { h: scale, ..r }, dark);
    ren.rect(Rect { w: scale, ..r }, dark);
    ren.rect(Rect { y: r.y + r.h - scale, h: scale, ..r }, light);
    ren.rect(Rect { x: r.x + r.w - scale, w: scale, ..r }, light);
    if selected {
        let w = 2.0 * scale;
        let edge = 0xFFFFFFFF;
        ren.rect(Rect { x: r.x - scale, y: r.y - scale, w: r.w + 2.0 * scale, h: w }, edge);
        ren.rect(Rect { x: r.x - scale, y: r.y + r.h - scale, w: r.w + 2.0 * scale, h: w }, edge);
        ren.rect(Rect { x: r.x - scale, y: r.y - scale, w, h: r.h + 2.0 * scale }, edge);
        ren.rect(Rect { x: r.x + r.w - scale, y: r.y - scale, w, h: r.h + 2.0 * scale }, edge);
    }
}

/// Display name of an item: `item.<ns>.<path>`, then `block.<ns>.<path>`, then the raw name.
/// `item.ns.name`, or `block.ns.name` for block items that only have that.
fn item_key(lang: &Lang, item: u32) -> Option<String> {
    let d = content().items.get(item as usize)?;
    let path = d.name.replace(':', ".");
    [format!("item.{path}"), format!("block.{path}")].into_iter().find(|k| lang.has(k))
}

pub fn item_name(lang: &Lang, item: u32) -> String {
    match item_key(lang, item) {
        Some(k) => lang.get(&k).to_string(),
        None => content().items.get(item as usize).map_or(String::new(), |d| d.name.clone()),
    }
}

/// The name, then the `<name key>.tooltip` translation in grey (the part after `{shift}` only while
/// sneak is held, a hint otherwise), then durability for damaged tools.
pub fn item_tooltip(lang: &Lang, input: &crate::input::Input, stack: voxide_abi::ItemStack) -> Vec<text::Span> {
    const GREY: u32 = 0xA8A8A8;
    let items = |name: &str| crate::content::try_content().and_then(|c| c.item_names.get(name).copied());
    let cx = text::Ctx { lang, input: Some(input), item_name: &items };
    let mut out = text::plain(&item_name(lang, stack.item));
    let line = |template: &str, args: &[text::Arg], out: &mut Vec<text::Span>| {
        out.push(text::Span { piece: text::Piece::Text("\n".into()), color: None });
        let start = out.len();
        text::format(template, args, &cx, out);
        for s in &mut out[start..] {
            s.color = s.color.or(Some(GREY));
        }
    };
    if let Some(key) = item_key(lang, stack.item).map(|k| format!("{k}.tooltip")).filter(|k| lang.has(k)) {
        let template = lang.get(&key);
        match template.split_once("{shift}") {
            Some((always, more)) => {
                let always = always.trim_end_matches('\n');
                if !always.is_empty() {
                    line(always, &[], &mut out);
                }
                if input.held("core:sneak") {
                    line(more.trim_start_matches('\n'), &[], &mut out);
                } else {
                    line(lang.get("tooltip.more"), &[], &mut out);
                }
            }
            None => line(template, &[], &mut out),
        }
    }
    let max = content().items.get(stack.item as usize).map_or(0, |d| d.durability);
    if max > 0 && stack.damage > 0 {
        line(lang.get("tooltip.durability"), &[text::Arg::Int((max - stack.damage.min(max)) as i64), text::Arg::Int(max as i64)], &mut out);
    }
    out
}

/// The debug overlay groups of one side, each a dark panel with a title; the help panel on the
/// left lists the F3 combinations.
fn debug_groups(ui: &mut Ui, groups: u32, right: bool) {
    let align = if right { Align::End } else { Align::Start };
    for (i, (title, side, lines)) in DEBUG_GROUPS.iter().enumerate() {
        if *side != right || groups & (1 << i) == 0 {
            continue;
        }
        ui.container(Style::default().pad(2.0).gap(0.0).align(align), |ui| {
            ui.nodes.last_mut().unwrap().kind = Kind::Container { class: Some("debug_panel".into()) };
            let t = ui.text_bound("debug_title", Box::new(move |_: &Ctx| (title.to_string(), vec![])));
            if let Some(t) = ui.text_mut(t) {
                t.wrap = false;
            }
            for (key, f) in lines.iter() {
                let key = key.to_string();
                let n = ui.text_bound("debug", Box::new(move |c: &Ctx| (key.clone(), f(c))));
                if let Some(t) = ui.text_mut(n) {
                    t.wrap = false;
                    t.mono = true;
                }
            }
        });
    }
    if !right {
        ui.container(Style::default().pad(2.0).gap(0.0).align(align), |ui| {
            ui.nodes.last_mut().unwrap().kind = Kind::Container { class: Some("debug_panel".into()) };
            ui.text_bound("debug_title", Box::new(|_: &Ctx| ("hud.debug.group.help".into(), vec![])));
            for (i, (title, ..)) in DEBUG_GROUPS.iter().enumerate() {
                let on = groups & (1 << i) != 0;
                let state = if on { "hud.debug.shown" } else { "hud.debug.hidden" };
                let n = ui.text_bound(
                    "debug",
                    Box::new(move |_: &Ctx| ("hud.debug.toggle".into(), vec![Arg::Int(i as i64 + 1), Arg::Key(title.to_string()), Arg::Key(state.into())])),
                );
                if let Some(t) = ui.text_mut(n) {
                    t.wrap = false;
                }
            }
            for key in ["hud.debug.gamemode", "hud.debug.cubes", "hud.debug.hitboxes"] {
                let n = ui.text_bound("debug", Box::new(move |_: &Ctx| (key.into(), vec![])));
                if let Some(t) = ui.text_mut(n) {
                    t.wrap = false;
                }
            }
        });
    }
}
