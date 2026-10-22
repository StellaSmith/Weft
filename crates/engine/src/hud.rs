//! In-game overlay: crosshair, hotbar, debug lines, chat, and HUD parts registered by mods.
//! Rebuilt only when what it shows changes; mod parts update through their bindings.

use voxide_abi::ItemStack;

use crate::app::Ctx;
use crate::client::Client;
use crate::client::modui::{self, TAG_SLOT};
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
    key: Option<(bool, usize, usize, bool)>,
}

impl Hud {
    pub fn invalidate(&mut self) {
        self.key = None;
    }

    /// Returns true if the tree was rebuilt.
    pub fn update(&mut self, client: &Client, debug: bool, lang: &Lang) -> bool {
        let chat_visible = client.chat.iter().filter(|(_, t)| t.elapsed().as_secs() < 10).count();
        let key = (debug, chat_visible, client.chat.len(), client.dead);
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        let ui = &mut self.ui;
        ui.clear();
        ui.root_style(Style::default().stretch());
        ui.container(Style::default().grow().pad(4.0).gap(1.0).align(Align::Start), |ui| {
            if debug {
                for (key, f) in DEBUG_LINES {
                    let key = key.to_string();
                    ui.text_bound("debug", Box::new(move |c: &Ctx| (key.clone(), f(c))));
                }
                ui.text_bound("debug", Box::new(|_: &Ctx| ("hud.debug.mods".into(), vec![Arg::Text(mod_profile())])));
            }
        });
        ui.add(Kind::Custom { tag: TAG_CROSSHAIR, data: 0 }, Style::default().size(9.0, 9.0).self_align(Align::Center));
        ui.container(Style::default().grow().justify(Justify::End).align(Align::Center).pad(2.0).gap(1.0), |ui| {
            ui.container(Style::default().gap(1.0).pad(2.0).align(Align::Start).self_align(Align::Start), |ui| {
                let now = std::time::Instant::now();
                for (line, at) in client.chat.iter().rev().take(10).rev() {
                    if now.duration_since(*at).as_secs() < 10 {
                        ui.text(text::plain(line), "chat");
                    }
                }
            });
            // Mod HUD parts (health, hunger...) stack above the hotbar.
            for &(owner, cb) in &content().game.huds {
                modui::build(ui, lang, owner, cb, Some(client));
            }
            ui.container(Style::row().gap(0.0), |ui| {
                for i in 0..9 {
                    ui.add(Kind::Custom { tag: TAG_HOTBAR, data: i }, Style::default().size(20.0, 20.0));
                }
            });
        });
        true
    }
}

fn mod_profile() -> String {
    let c = content();
    let mut parts = Vec::new();
    for (i, m) in c.mods.iter().enumerate() {
        let s = crate::host::stats(voxide_abi::ModId(i as u32));
        let ns = s.nanos.swap(0, std::sync::atomic::Ordering::Relaxed);
        let calls = s.calls.swap(0, std::sync::atomic::Ordering::Relaxed);
        parts.push(format!("{} {:.2}ms/{}c", m.id, ns as f64 / 1e6, calls));
    }
    parts.join(", ")
}

type DebugLine = (&'static str, fn(&Ctx) -> Vec<Arg>);

const DEBUG_LINES: &[DebugLine] = &[
    ("hud.debug.fps", |c| vec![Arg::Int(c.fps.round() as i64), Arg::Float(c.frame_ms as f64, 2)]),
    ("hud.debug.pos", |c| {
        let p = c.client.as_ref().map_or(glam::DVec3::ZERO, |cl| cl.player.pos);
        vec![Arg::Float(p.x, 2), Arg::Float(p.y, 2), Arg::Float(p.z, 2)]
    }),
    ("hud.debug.cube", |c| {
        let Some(cl) = c.client.as_ref() else { return vec![] };
        let p = cl.player.pos.floor().as_ivec3() >> 4i32;
        let dim = content().dims[cl.world.dim as usize].name.clone();
        vec![Arg::Int(p.x as i64), Arg::Int(p.y as i64), Arg::Int(p.z as i64), Arg::Text(dim)]
    }),
    ("hud.debug.world", |c| {
        let Some(cl) = c.client.as_ref() else { return vec![] };
        vec![
            Arg::Int(cl.world.cubes.len() as i64),
            Arg::Int(cl.world.pending_meshes() as i64),
            Arg::Int(cl.render.drawn as i64),
            Arg::Int((cl.render.faces_drawn / 1000) as i64),
        ]
    }),
    ("hud.debug.entities", |c| vec![Arg::Int(c.client.as_ref().map_or(0, |cl| cl.entities.len()) as i64)]),
    ("hud.debug.server", |c| vec![Arg::Float(c.server_ms as f64, 2)]),
    ("hud.debug.looking", |c| {
        let name = c.client.as_ref().and_then(|cl| cl.target).map(|h| content().state_name(h.state)).unwrap_or_else(|| "-".into());
        vec![Arg::Text(name)]
    }),
];

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
        TAG_SLOT => {
            let Some(client) = client else { return };
            slot_background(ren, theme, r, false, scale);
            let stack = client.menu.as_ref().and_then(|m| m.slots.get(data as usize)).copied().unwrap_or_default();
            stack_icon(ren, textures, r, stack, scale, text_unit);
            if hovered {
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
pub fn item_name(lang: &Lang, item: u32) -> String {
    let c = content();
    let Some(d) = c.items.get(item as usize) else { return String::new() };
    let path = d.name.replace(':', ".");
    for key in [format!("item.{path}"), format!("block.{path}")] {
        if lang.has(&key) {
            return lang.get(&key).to_string();
        }
    }
    d.name.clone()
}
