//! In-game overlay: crosshair, hotbar, debug lines. Rebuilt only when what it shows changes.

use crate::app::Ctx;
use crate::client::Client;
use crate::client::textures::GameTextures;
use crate::content::content;
use crate::ui::render::UiRenderer;
use crate::ui::text::Arg;
use crate::ui::theme::Theme;
use crate::ui::{Align, Justify, Kind, Rect, Style, Ui};

pub const TAG_HOTBAR: u32 = 1;
pub const TAG_CROSSHAIR: u32 = 2;

#[derive(Default)]
pub struct Hud {
    pub ui: Ui,
    key: Option<(bool, usize, Vec<u32>)>,
}

impl Hud {
    pub fn invalidate(&mut self) {
        self.key = None;
    }

    /// Returns true if the tree was rebuilt.
    pub fn update(&mut self, client: &Client, debug: bool) -> bool {
        let key = (debug, client.selected, client.hotbar.clone());
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        self.key = Some(key);
        let ui = &mut self.ui;
        ui.clear();
        ui.root_style(Style::default().stretch());
        // Top: debug text. Middle: crosshair. Bottom: hotbar.
        ui.container(Style::default().grow().pad(4.0).gap(1.0).align(Align::Start), |ui| {
            if debug {
                for (key, f) in DEBUG_LINES {
                    let key = key.to_string();
                    ui.text_bound("debug", Box::new(move |c: &Ctx| (key.clone(), f(c))));
                }
            }
        });
        ui.add(Kind::Custom { tag: TAG_CROSSHAIR, data: 0 }, Style::default().size(9.0, 9.0).self_align(Align::Center));
        ui.container(Style::default().grow().justify(Justify::End).align(Align::Center).pad(2.0), |ui| {
            ui.container(Style::row().gap(0.0), |ui| {
                for i in 0..9 {
                    ui.add(Kind::Custom { tag: TAG_HOTBAR, data: i }, Style::default().size(20.0, 20.0));
                }
            });
        });
        true
    }
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
    ("hud.debug.server", |c| vec![Arg::Float(c.server_ms as f64, 2)]),
    ("hud.debug.looking", |c| {
        let name = c
            .client
            .as_ref()
            .and_then(|cl| cl.target)
            .map(|h| content().state_name(h.state))
            .unwrap_or_else(|| "-".into());
        vec![Arg::Text(name)]
    }),
];

/// Engine-drawn nodes of the HUD.
#[allow(clippy::too_many_arguments)]
pub fn draw_custom(
    ren: &mut UiRenderer,
    theme: &Theme,
    textures: &GameTextures,
    client: Option<&Client>,
    tag: u32,
    data: u64,
    r: Rect,
    _hovered: bool,
    scale: f32,
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
            if let Some(&item) = client.hotbar.get(data as usize) {
                let s = 16.0 * scale;
                ren.item_icon(content(), textures, r.x + (r.w - s) / 2.0, r.y + (r.h - s) / 2.0, s, item);
            }
        }
        _ => {}
    }
}

pub fn slot_background(ren: &mut UiRenderer, theme: &Theme, r: Rect, selected: bool, scale: f32) {
    ren.rect(r, theme.color("slot", "fill", 0x00000080));
    let b = scale;
    let edge = if selected { 0xFFFFFFFF } else { theme.color("slot", "edge", 0x8B8B8BFF) };
    let w = if selected { 2.0 * b } else { b };
    ren.rect(Rect { h: w, ..r }, edge);
    ren.rect(Rect { y: r.y + r.h - w, h: w, ..r }, edge);
    ren.rect(Rect { w, ..r }, edge);
    ren.rect(Rect { x: r.x + r.w - w, w, ..r }, edge);
}
