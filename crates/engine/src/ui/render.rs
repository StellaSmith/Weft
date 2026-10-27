//! Turns a laid-out `Ui` into quads. Rebuilt only when the tree is dirty; the GPU copy is reused
//! across frames otherwise.

use crate::gpu_layout::ui_mode;
use rustc_hash::FxHashMap;
use weft_render::*;

use std::sync::Arc;

use super::font::Font;
use crate::model::{Animation, Prop};
use super::text::Placed;
use super::theme::Theme;
use super::{Kind, Rect, Ui};
use crate::assets::{Assets, decode_png};
use crate::shaders::{Hook, Slots};
use crate::client::textures::GameTextures;
use crate::content::Content;
pub use crate::gpu_layout::Quad;

/// Quads of a node and its children moved by keyframes every frame: `<class>.animation =
/// ns:name` in a theme names `animations/<name>.animation.json`, whose `open` animation plays
/// once when the screen opens and `idle` loops after. Any bone: position in GUI pixels (y up),
/// rotation z in degrees clockwise and scale, about the node's center.
#[derive(Clone)]
pub struct Animated {
    pub quads: std::ops::Range<usize>,
    open: Option<Arc<Animation>>,
    idle: Option<Arc<Animation>>,
    since: f32,
    center: [f32; 2],
    scale: f32,
}

/// Moves the animated ranges of `quads` (all quads of the frame) to this moment.
pub fn animate(quads: &mut [Quad], list: &[Animated]) {
    let now = crate::shaders::clock();
    for a in list {
        // Automated runs skip opening so screenshots do not depend on timing.
        let (anim, t) = match (&a.open, &a.idle) {
            (Some(o), _) if !crate::shaders::pinned() && now - a.since < o.length => (o, now - a.since),
            (_, Some(i)) => (i, crate::shaders::time().rem_euclid(i.length)),
            _ => continue,
        };
        let get = |prop: Prop, default: [f32; 3]| anim.channels.iter().find(|c| c.prop == prop).map_or(default, |c| c.sample(t));
        let (pos, rot, sc) = (get(Prop::Position, [0.0; 3]), get(Prop::Rotation, [0.0; 3]), get(Prop::Scale, [1.0; 3]));
        let (s, c) = rot[2].to_radians().sin_cos();
        let turn = |p: [f32; 2]| {
            let (x, y) = (p[0] * sc[0], p[1] * sc[1]);
            [x * c - y * s, x * s + y * c]
        };
        let to = [a.center[0] + pos[0] * a.scale, a.center[1] - pos[1] * a.scale];
        for q in quads.get_mut(a.quads.clone()).into_iter().flatten() {
            let r = turn([q.p0[0] - a.center[0], q.p0[1] - a.center[1]]);
            (q.p0, q.u, q.v) = ([to[0] + r[0], to[1] + r[1]], turn(q.u), turn(q.v));
        }
    }
}

/// How a GUI texture fills a rectangle of another size. Set by the texture itself: a `.gui` file
/// next to it (`scaling = nine_slice | tile | stretch`, `border = 3` or `border = l t r b`,
/// `size = w h` when the image is a higher resolution than the GUI pixels it stands for). Themes
/// only give a fallback for bare images.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slicing {
    /// Corners keep their size, edges and center stretch; borders left, top, right, bottom in GUI
    /// pixels of an image `size` GUI pixels big.
    Nine { size: [f32; 2], border: [f32; 4] },
    Tile { size: [f32; 2] },
    Stretch,
}

fn parse_gui(src: &str, pixels: [f32; 2]) -> Slicing {
    let mut kind = "nine_slice";
    let mut border = [0.0f32; 4];
    let mut size = pixels;
    for (_, k, v) in crate::kv::pairs(src) {
        let nums: Vec<f32> = v.split_whitespace().filter_map(|n| n.parse().ok()).collect();
        match k {
            "scaling" => kind = if v.trim() == "tile" { "tile" } else if v.trim() == "stretch" { "stretch" } else { "nine_slice" },
            "border" if nums.len() == 1 => border = [nums[0]; 4],
            "border" if nums.len() == 4 => border = [nums[0], nums[1], nums[2], nums[3]],
            "size" if nums.len() == 2 => size = [nums[0], nums[1]],
            _ => {}
        }
    }
    match kind {
        "tile" => Slicing::Tile { size },
        "stretch" => Slicing::Stretch,
        _ => Slicing::Nine { size, border },
    }
}

pub struct UiRenderer {
    pipeline: PipelineId,
    pub font: Font,
    font_tex: TextureId,
    font_idx: u32,
    textures: FxHashMap<String, Option<(u32, u32, u32)>>,
    /// Slicing a texture declares for itself, and how far it reaches outside the box it is
    /// drawn for (`outset` in its `.gui`: shadows, frames drawn around the content).
    slicing: FxHashMap<String, Option<Slicing>>,
    outset: FxHashMap<String, f32>,
    owned: Vec<TextureId>,
    buffers: Vec<(BufferId, u32)>,
    screen: Vec<BufferId>,
    pub quads: Vec<Quad>,
    /// Clip of each deferred item icon, in the order they were queued.
    icon_clips: Vec<[f32; 4]>,
    /// Stack counts drawn over queued item icons: (icon number, count, text pixel size).
    icon_counts: Vec<(usize, u32, f32)>,
    /// Text effects and GUI fills the shader has, and the effect slot by `text::fx_id`.
    fx: Slots,
    gui: Slots,
    fx_slots: Vec<u32>,
    /// Theme animation files by name, and the nodes the last `build` animated.
    anims: FxHashMap<String, Vec<Arc<Animation>>>,
    pub animated: Vec<Animated>,
}

pub const FX: Hook = Hook {
    kind: "fx",
    entry: "effect",
    dispatch: "apply_fx",
    params: "g: Glyph",
    args: "g",
    ret: "Glyph",
    fallback: "return g;",
};

pub const GUI: Hook = Hook {
    kind: "gui",
    entry: "fill",
    dispatch: "gui_fill",
    params: "p: GuiPixel",
    args: "p",
    ret: "vec4<f32>",
    fallback: "return p.color;",
};

fn load_shader(assets: &Assets) -> Result<(Shader, Slots, Slots), ShaderError> {
    let (fx, gui) = (crate::shaders::snippets(assets, &FX), crate::shaders::snippets(assets, &GUI));
    let (shader, mut slots) = crate::packs::compose("ui.wgsl", include_str!("../../shaders/ui.wgsl"), &[(&FX, &fx), (&GUI, &gui)])?;
    let gui = slots.pop().unwrap_or_default();
    let fx = slots.pop().unwrap_or_default();
    Ok((shader, fx, gui))
}

impl UiRenderer {
    pub fn new(gpu: &mut dyn Backend, assets: &Assets) -> Result<UiRenderer, String> {
        let (pipeline, fx, gui) = Self::pipeline(gpu, assets)?;
        let (font, font_tex, font_idx) = Self::load_font(gpu, assets).ok_or("font missing")?;
        let frames = gpu.frames_in_flight();
        let screen = (0..frames).map(|_| gpu.create_buffer(16, usage::STORAGE | usage::HOST, "ui screen")).collect();
        Ok(UiRenderer {
            pipeline,
            font,
            font_tex,
            font_idx,
            textures: FxHashMap::default(),
            slicing: FxHashMap::default(),
            outset: FxHashMap::default(),
            owned: Vec::new(),
            buffers: Vec::new(),
            screen,
            quads: Vec::new(),
            icon_clips: Vec::new(),
            icon_counts: Vec::new(),
            fx,
            gui,
            fx_slots: Vec::new(),
            anims: FxHashMap::default(),
            animated: Vec::new(),
        })
    }

    /// `core:fonts/default` (atlas and its `.font`) over `core:fonts/unifont`; packs can replace either.
    fn load_font(gpu: &mut dyn Backend, assets: &Assets) -> Option<(Font, TextureId, u32)> {
        // Built in too: text must show even without the engine's asset folder.
        let hex = assets.read("core:fonts/unifont", "", "hex").unwrap_or_else(|| include_bytes!("../../assets/fonts/unifont.hex").to_vec());
        let desc = assets.read("core:fonts/default", "", "font").unwrap_or_else(|| include_bytes!("../../assets/fonts/default.font").to_vec());
        let img = assets.read("core:fonts/default", "", "png").unwrap_or_else(|| include_bytes!("../../assets/fonts/default.png").to_vec());
        let (desc, img) = (Some(desc), crate::assets::decode_png(&img));
        let desc = desc.map(|d| String::from_utf8_lossy(&d).into_owned());
        let font = Font::new(&String::from_utf8_lossy(&hex), desc.as_deref().zip(img.as_ref()));
        let tex = gpu.create_texture(
            &TextureDesc { width: font.atlas_width, height: font.atlas_height, mips: 1, format: Format::R8Unorm, render_target: false },
            "font",
        );
        gpu.write_texture(tex, 0, &font.atlas);
        let idx = gpu.texture_index(tex);
        Some((font, tex, idx))
    }

    fn pipeline(gpu: &mut dyn Backend, assets: &Assets) -> Result<(PipelineId, Slots, Slots), String> {
        let (shader, fx, gui) = load_shader(assets).map_err(|e| e.0)?;
        let p = gpu.create_pipeline(&PipelineDesc {
            label: "ui",
            shader: &shader,
            vertex: "vs",
            fragment: Some("fs"),
            color: Some(Format::Surface),
            depth: None,
            blend: Blend::Alpha,
            cull_back: false,
            topology: Topology::Triangles,
        })
        .map_err(|e| e.0)?;
        Ok((p, fx, gui))
    }

    /// Shader slot of a text effect (`text::fx_id`), 0 when no snippet of that name exists.
    fn fx_slot(&mut self, id: u16) -> u32 {
        if id == 0 {
            return 0;
        }
        while self.fx_slots.len() < id as usize {
            let name = super::text::fx_name(self.fx_slots.len() as u16 + 1).unwrap_or_default();
            let slot = self.fx.slot(&name);
            if slot == 0 {
                log::warn!("text effect {name}: no shaders/fx/<name>.wgsl for it in any mod or pack");
            }
            self.fx_slots.push(slot);
        }
        self.fx_slots[id as usize - 1]
    }

    /// A rectangle filled by GUI shader `name` (`shaders/gui/<name>.wgsl`); false if there is none.
    pub fn shader_fill(&mut self, r: Rect, name: &str, color: u32) -> bool {
        let slot = self.gui.slot(name);
        if slot == 0 {
            return false;
        }
        self.quads.push(Quad { p0: [r.x, r.y], u: [r.w, 0.0], v: [0.0, r.h], uv: [0.0, 0.0, 1.0, 1.0], color, mode: ui_mode::GUI, fx: slot, ..Default::default() });
        true
    }

    pub fn reload(&mut self, gpu: &mut dyn Backend, assets: &Assets) {
        if let Some((font, tex, idx)) = Self::load_font(gpu, assets) {
            gpu.destroy_texture(self.font_tex);
            (self.font, self.font_tex, self.font_idx) = (font, tex, idx);
        }
        match Self::pipeline(gpu, assets) {
            Ok((p, fx, gui)) => {
                gpu.destroy_pipeline(self.pipeline);
                (self.pipeline, self.fx, self.gui) = (p, fx, gui);
                self.fx_slots.clear();
            }
            Err(e) => log::error!("ui shader: {e}"),
        }
        for t in self.owned.drain(..) {
            gpu.destroy_texture(t);
        }
        self.textures.clear();
        // How textures slice comes with them: a pack's `.mcmeta` must not outlive the pack.
        self.slicing.clear();
        self.outset.clear();
        self.anims.clear();
    }

    /// Bindless index and size of a theme/GUI texture, loaded the first time it is used.
    pub fn texture(&mut self, gpu: &mut dyn Backend, assets: &Assets, name: &str) -> Option<(u32, u32, u32)> {
        if let Some(t) = self.textures.get(name) {
            return *t;
        }
        let loaded = assets.read(name, "textures", "png").and_then(|b| decode_png(&b)).map(|img| {
            let t = gpu.create_texture(
                &TextureDesc { width: img.width, height: img.height, mips: 1, format: Format::Rgba8Srgb, render_target: false },
                name,
            );
            gpu.write_texture(t, 0, &img.rgba);
            self.owned.push(t);
            (gpu.texture_index(t), img.width, img.height)
        });
        if loaded.is_none() {
            log::warn!("missing gui texture {name}");
        }
        self.textures.insert(name.to_string(), loaded);
        loaded
    }

    fn declared_slicing(&mut self, assets: &Assets, name: &str, pixels: [f32; 2]) -> Option<Slicing> {
        if let Some(s) = self.slicing.get(name) {
            return *s;
        }
        let read = |ext: &str| assets.read(name, "textures", ext).map(|b| String::from_utf8_lossy(&b).into_owned());
        let gui = read("gui");
        let outset = gui.as_deref().and_then(|src| {
            crate::kv::pairs(src).find(|p| p.1 == "outset").and_then(|(line, k, v)| crate::kv::number(name, line, k, v))
        });
        self.outset.insert(name.to_string(), outset.unwrap_or(0.0));
        let s = gui.map(|src| parse_gui(&src, pixels));
        self.slicing.insert(name.to_string(), s);
        s
    }

    /// Loads the textures of a text's inline images, so `glyph_run` can draw them.
    pub fn load_images(&mut self, gpu: &mut dyn Backend, assets: &Assets, layout: &super::text::TextLayout) {
        for id in super::text::images(layout) {
            if let Some(name) = super::text::image_name(id) {
                self.texture(gpu, assets, &name);
            }
        }
    }

    /// Draws GUI texture `name` over `r` the way the texture asks for, or with `fallback` (from the
    /// theme) when it says nothing. False if the texture is missing.
    pub fn draw_gui(&mut self, gpu: &mut dyn Backend, assets: &Assets, name: &str, r: Rect, scale: f32, color: u32, fallback: Slicing) -> bool {
        let Some(tex) = self.texture(gpu, assets, name) else { return false };
        let pixels = [tex.1 as f32, tex.2 as f32];
        let declared = self.declared_slicing(assets, name, pixels);
        let o = self.outset.get(name).copied().unwrap_or(0.0) * scale;
        let r = Rect { x: r.x - o, y: r.y - o, w: r.w + 2.0 * o, h: r.h + 2.0 * o };
        let slicing = declared.unwrap_or(match fallback {
            Slicing::Nine { border, .. } => Slicing::Nine { size: pixels, border },
            f => f,
        });
        match slicing {
            Slicing::Nine { size, border } => self.nine_sliced(r, tex.0, size, border, scale, color),
            Slicing::Tile { size } => self.tile(r, tex, size[0] * scale, color),
            Slicing::Stretch => self.image(r, tex.0, [0.0, 0.0, 1.0, 1.0], color),
        }
        true
    }

    /// `draw_gui` for a texture already loaded (by an earlier `texture` call); false otherwise.
    pub fn draw_loaded(&mut self, name: &str, r: Rect, scale: f32, color: u32) -> bool {
        let Some(Some(tex)) = self.textures.get(name).copied() else { return false };
        let pixels = [tex.1 as f32, tex.2 as f32];
        let slicing = self.slicing.get(name).copied().flatten().unwrap_or(Slicing::Stretch);
        let o = self.outset.get(name).copied().unwrap_or(0.0) * scale;
        let r = Rect { x: r.x - o, y: r.y - o, w: r.w + 2.0 * o, h: r.h + 2.0 * o };
        match slicing {
            Slicing::Nine { size, border } => self.nine_sliced(r, tex.0, if size == [0.0; 2] { pixels } else { size }, border, scale, color),
            Slicing::Tile { size } => self.tile(r, tex, size[0] * scale, color),
            Slicing::Stretch => self.image(r, tex.0, [0.0, 0.0, 1.0, 1.0], color),
        }
        true
    }

    pub fn rect(&mut self, r: Rect, color: u32) {
        self.quads.push(Quad { p0: [r.x, r.y], u: [r.w, 0.0], v: [0.0, r.h], uv: [0.0; 4], color, mode: ui_mode::SOLID, ..Default::default() });
    }

    pub fn image(&mut self, r: Rect, tex: u32, uv: [f32; 4], color: u32) {
        self.quads.push(Quad { p0: [r.x, r.y], u: [r.w, 0.0], v: [0.0, r.h], uv, color, tex, mode: ui_mode::TEXTURE, ..Default::default() });
    }

    /// 9-slice: corners keep their size (scaled), edges and center stretch.
    pub fn nine(&mut self, r: Rect, tex: (u32, u32, u32), border: f32, scale: f32, color: u32) {
        self.nine_sliced(r, tex.0, [tex.1 as f32, tex.2 as f32], [border; 4], scale, color);
    }

    /// 9-slice with its own border per side, in GUI pixels of an image `size` GUI pixels big.
    pub fn nine_sliced(&mut self, r: Rect, idx: u32, size: [f32; 2], border: [f32; 4], scale: f32, color: u32) {
        let [l, t, rr, b] = border.map(|v| v * scale);
        let (l, rr) = (l.min(r.w / 2.0), rr.min(r.w / 2.0));
        let (t, b) = (t.min(r.h / 2.0), b.min(r.h / 2.0));
        let xs = [r.x, r.x + l, r.x + r.w - rr, r.x + r.w];
        let ys = [r.y, r.y + t, r.y + r.h - b, r.y + r.h];
        let us = [0.0, border[0] / size[0], 1.0 - border[2] / size[0], 1.0];
        let vs = [0.0, border[1] / size[1], 1.0 - border[3] / size[1], 1.0];
        for j in 0..3 {
            for i in 0..3 {
                let rr = Rect { x: xs[i], y: ys[j], w: xs[i + 1] - xs[i], h: ys[j + 1] - ys[j] };
                if rr.w > 0.0 && rr.h > 0.0 {
                    self.image(rr, idx, [us[i], vs[j], us[i + 1], vs[j + 1]], color);
                }
            }
        }
    }

    /// Repeats the texture every `size` physical pixels from the rect's corner.
    pub fn tile(&mut self, r: Rect, tex: (u32, u32, u32), size: f32, color: u32) {
        let mut y = r.y;
        while y < r.y + r.h {
            let h = size.min(r.y + r.h - y);
            let mut x = r.x;
            while x < r.x + r.w {
                let w = size.min(r.x + r.w - x);
                self.image(Rect { x, y, w, h }, tex.0, [0.0, 0.0, w / size, h / size], color);
                x += size;
            }
            y += size;
        }
    }

    pub fn glyph_run(&mut self, items: &[Placed], x: f32, y: f32, px: f32, default_color: u32, shadow: Option<u32>, icons: &mut Vec<(f32, f32, f32, u32)>) {
        for pass in [true, false] {
            if pass && shadow.is_none() {
                continue;
            }
            let off = if pass { px * self.font.pixel } else { 0.0 };
            for (index, it) in items.iter().enumerate() {
                match *it {
                    Placed::Glyph { x: gx, y: gy, ch, color, fx } => {
                        if ch == ' ' {
                            continue;
                        }
                        let fx = self.fx_slot(fx) | if pass && fx != 0 { 1 << 31 } else { 0 };
                        let g = self.font.glyph(ch);
                        let uv = self.font.uv(g);
                        let w = (g.advance as f32 - 1.0) * px;
                        let c = if pass {
                            shadow.unwrap()
                        } else {
                            color.map_or(default_color, |c| c << 8 | (default_color & 0xFF))
                        };
                        // Whole physical pixels: centered text starts mid-pixel, and nearest sampling
                        // at texel borders would drop or double glyph columns.
                        self.quads.push(Quad {
                            p0: [(x + gx * px + off).round(), (y + gy * px + off).round()],
                            u: [w, 0.0],
                            v: [0.0, 16.0 * px],
                            uv,
                            color: c,
                            tex: self.font_idx,
                            mode: ui_mode::FONT,
                            fx,
                            index: index as f32,
                            ..Default::default()
                        });
                    }
                    Placed::Icon { x: ix, y: iy, item } if !pass => icons.push((x + ix * px, y + iy * px, 16.0 * px, item)),
                    Placed::Block { x: ix, y: iy, state } if !pass => icons.push((x + ix * px, y + iy * px, 16.0 * px, state | super::text::STATE_ICON)),
                    // Loaded by `load_images` before.
                    Placed::Image { x: ix, y: iy, w, h, image } if !pass => {
                        if let Some(Some(t)) = super::text::image_name(image).and_then(|n| self.textures.get(&n).copied()) {
                            self.image(Rect { x: x + ix * px, y: y + iy * px, w: w * px, h: h * px }, t.0, [0.0, 0.0, 1.0, 1.0], 0xFFFFFFFF);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Item icon: flat sprite, or an isometric cube for block items.
    pub fn item_icon(&mut self, c: &Content, tex: &GameTextures, x: f32, y: f32, size: f32, item: u32) {
        let Some(def) = c.items.get(item as usize) else { return };
        if item == 0 {
            return;
        }
        if def.icon != u32::MAX {
            self.quads.push(Quad {
                p0: [x, y],
                u: [size, 0.0],
                v: [0.0, size],
                uv: [0.0, 0.0, 1.0, 1.0],
                color: 0xFFFFFFFF,
                tex: tex.texture(def.icon),
                mode: ui_mode::ICON,
                ..Default::default()
            });
            return;
        }
        let Some(state) = def.block else { return };
        self.state_icon(c, tex, x, y, size, state);
    }

    /// A block state the way block items show: a sprite for flat blocks, else an isometric model.
    pub fn state_icon(&mut self, c: &Content, tex: &GameTextures, x: f32, y: f32, size: f32, state: u32) {
        let Some(s) = c.states.get(state as usize) else { return };
        let tint = |face: usize| -> u32 {
            if s.tint == 0 || s.tint_faces & (1 << face) == 0 {
                return 0xFFFFFFFF;
            }
            crate::content::item_tint(s.tint) << 8 | 0xFF
        };
        let shade = |c: u32, k: f32| {
            let ch = |s: u32| (((c >> s) & 255) as f32 * k) as u32;
            ch(24) << 24 | ch(16) << 16 | ch(8) << 8 | 0xFF
        };
        if let Some(t) = s.item_sprite() {
            self.quads.push(Quad {
                p0: [x, y],
                u: [size, 0.0],
                v: [0.0, size],
                uv: [0.0, 0.0, 1.0, 1.0],
                color: tint(0),
                tex: tex.texture(t),
                mode: ui_mode::ICON,
                ..Default::default()
            });
            return;
        }
        // Isometric: block point (X, Y, Z) lands at origin + X*ex + Y*ey + Z*ez.
        let (h, q) = (size / 2.0, size / 4.0);
        let o = [x + h, y + size * 0.03 + h];
        let at = |p: [f32; 3]| [o[0] + (p[0] - p[2]) * h, o[1] + (p[0] + p[2]) * q - p[1] * h];
        let grass = crate::content::item_tint(s.tint) << 8 | 0xFF;
        let mut shape = s.item_shape();
        // Back to front.
        shape.sort_by(|a, b| (a.min[0] + a.min[1] + a.min[2]).total_cmp(&(b.min[0] + b.min[1] + b.min[2])));
        for b in &shape {
            let (n, m) = (b.min, b.max);
            let (dx, dy, dz) = (m[0] - n[0], m[1] - n[1], m[2] - n[2]);
            // (corner, u edge, v edge, face, shade, uv rect): the faces seen from the front, with
            // the part of the texture the world mesher shows there.
            let faces = [
                (at([n[0], m[1], m[2]]), [dx * h, dx * q], [0.0, dy * h], 5, 0.8, [n[0], 1.0 - m[1], m[0], 1.0 - n[1]]),
                (at([m[0], m[1], m[2]]), [dz * h, -dz * q], [0.0, dy * h], 1, 0.62, [1.0 - m[2], 1.0 - m[1], 1.0 - n[2], 1.0 - n[1]]),
                (at([n[0], m[1], n[2]]), [dx * h, dx * q], [-dz * h, dz * q], 3, 1.0, [n[0], n[2], m[0], m[2]]),
            ];
            for (p0, u, v, face, k, uv) in faces {
                self.quads.push(Quad { p0, u, v, uv, color: shade(tint(face), k), tex: tex.texture(b.textures[face]), mode: ui_mode::ICON, ..Default::default() });
                // Tinted overlay on top of its face.
                if let Some(&ov) = tex.overlays.get(b.textures[face] as usize).filter(|&&ov| ov != 0) {
                    let color = shade(if s.tint == 0 { 0xFFFFFFFF } else { grass }, k);
                    self.quads.push(Quad { p0, u, v, uv, color, tex: tex.texture(ov - 1), mode: ui_mode::ICON, ..Default::default() });
                }
            }
        }
    }

    /// Regenerates quads for the whole tree. `custom` draws engine-specific nodes (slots).
    pub fn build(
        &mut self,
        gpu: &mut dyn Backend,
        assets: &Assets,
        ui: &Ui,
        theme: &Theme,
        scale: f32,
        game: Option<(&Content, &GameTextures)>,
        custom: &mut dyn FnMut(&mut UiRenderer, u32, u64, Rect, bool, Option<u32>),
    ) {
        self.quads.clear();
        self.icon_clips.clear();
        self.icon_counts.clear();
        self.animated.clear();
        let mut icons = Vec::new();
        self.node(gpu, assets, ui, theme, 0, scale, &mut icons, custom, None);
        if let Some((c, t)) = game {
            let clips = std::mem::take(&mut self.icon_clips);
            let counts = std::mem::take(&mut self.icon_counts);
            for (i, (x, y, s, item)) in icons.into_iter().enumerate() {
                let start = self.quads.len();
                self.text_icon(c, t, x, y, s, item);
                // The count over the icon, at its bottom right like in a slot.
                if let Some(&(_, n, px)) = counts.iter().find(|c| c.0 == i) {
                    let layout = super::text::layout(&super::text::plain(&n.to_string()), &self.font, None);
                    let mut none = Vec::new();
                    self.glyph_run(&layout.items, x + s - layout.width * px + px * 2.0, y + s - 16.0 * px + px * 2.0, px, 0xFFFFFFFF, Some(0x3F3F3FFF), &mut none);
                }
                let clip = clips.get(i).copied().unwrap_or_default();
                for q in &mut self.quads[start..] {
                    q.clip = clip;
                }
            }
        }
    }

    /// The hovered node's tooltip next to the mouse (physical pixels), drawn every frame on top.
    pub fn tooltip(&mut self, gpu: &mut dyn Backend, assets: &Assets, ui: &Ui, theme: &Theme, scale: f32, at: glam::Vec2, game: Option<(&Content, &GameTextures)>) {
        let Some(tip) = ui.hovered.and_then(|h| ui.nodes.get(h)).and_then(|n| n.tooltip.as_ref()) else { return };
        // Vanilla's spacing: lines 10 GUI pixels apart, 3 pixels of padding inside the frame.
        let px = scale * ui.text_unit;
        let layout = super::text::layout_spaced(tip, &self.font, Some(320.0), 10.0 * scale / px);
        let pad = 4.0 * scale;
        let (w, hgt) = (layout.width * px + 2.0 * pad, layout.height * px + 2.0 * pad);
        // Like vanilla: right of and slightly above the cursor, flipped to the left near the edge.
        let mut x = at.x + 12.0 * scale;
        let y = (at.y - 12.0 * scale).min(ui.size.y * scale - hgt).max(0.0);
        if x + w > ui.size.x * scale {
            x = (at.x - w - 12.0 * scale).max(0.0);
        }
        let bg = Rect { x, y, w, h: hgt };
        let drawn = theme.get("tooltip", "texture").map(str::to_string).is_some_and(|n| self.draw_gui(gpu, assets, &n, bg, scale, 0xFFFFFFFF, theme_slicing(theme, "tooltip")));
        if !drawn {
            self.rect(bg, 0x100010F0);
        }
        if let Some(frame) = theme.get("tooltip", "frame").map(str::to_string) {
            self.draw_gui(gpu, assets, &frame, bg, scale, 0xFFFFFFFF, theme_slicing(theme, "tooltip"));
        }
        let mut icons = Vec::new();
        let color = theme.color("tooltip", "text", 0xFFFFFFFF);
        self.load_images(gpu, assets, &layout);
        self.glyph_run(&layout.items, x + pad, y + pad, px, color, Some(0x3F3F3FFF), &mut icons);
        if let Some((c, t)) = game {
            for (ix, iy, s, item) in icons {
                self.text_icon(c, t, ix, iy, s, item);
            }
        }
    }

    /// An icon placed by rich text: an item, or a block state (`{block:..}`).
    fn text_icon(&mut self, c: &Content, t: &GameTextures, x: f32, y: f32, s: f32, icon: u32) {
        if icon & super::text::STATE_ICON != 0 {
            self.state_icon(c, t, x, y, s, icon & !super::text::STATE_ICON);
        } else {
            self.item_icon(c, t, x, y, s, icon);
        }
    }

    /// Text centered on `cx`, by default in one color (`0xRRGGBBAA`), with a shadow.
    pub fn label_colored(&mut self, spans: &[super::text::Span], cx: f32, y: f32, px: f32, color: u32) {
        let layout = super::text::layout(spans, &self.font, None);
        let mut icons = Vec::new();
        let x = (cx - layout.width * px / 2.0).round();
        self.glyph_run(&layout.items, x, y.round(), px, color, Some(0x00000000 | (color & 0xFF) / 2), &mut icons);
    }

    /// One line of text centered on `cx` with its top at `y` (physical pixels); `alpha` fades it.
    pub fn label(&mut self, spans: &[super::text::Span], cx: f32, y: f32, px: f32, alpha: u32) {
        let layout = super::text::layout(spans, &self.font, None);
        let mut icons = Vec::new();
        let x = (cx - layout.width * px / 2.0).round();
        self.glyph_run(&layout.items, x, y, px, 0xFFFFFF00 | alpha, Some(0x3F3F3F00 | alpha), &mut icons);
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        &mut self,
        gpu: &mut dyn Backend,
        assets: &Assets,
        ui: &Ui,
        theme: &Theme,
        id: usize,
        scale: f32,
        icons: &mut Vec<(f32, f32, f32, u32)>,
        custom: &mut dyn FnMut(&mut UiRenderer, u32, u64, Rect, bool, Option<u32>),
        surface: Option<&str>,
    ) {
        let n = &ui.nodes[id];
        let r = Rect { x: n.rect.x * scale, y: n.rect.y * scale, w: n.rect.w * scale, h: n.rect.h * scale };
        let (quad_start, icon_start) = (self.quads.len(), icons.len());
        let hovered = ui.hovered == Some(id);
        let mut background = |me: &mut UiRenderer, class: &str, state: &str| {
            let color = theme.color(class, "color", 0xFFFFFFFF);
            if let Some(shader) = theme.get(class, "shader").map(str::to_string) {
                if me.shader_fill(r, &shader, color) {
                    return;
                }
            }
            let name = theme.get(class, state).or_else(|| theme.get(class, "texture")).map(str::to_string);
            let drawn = name.is_some_and(|n| me.draw_gui(gpu, assets, &n, r, scale, color, theme_slicing(theme, class)));
            if !drawn && theme.get(class, "fill").is_some() {
                me.rect(r, theme.color(class, "fill", 0));
            }
        };
        match &n.kind {
            Kind::Container { class: Some(class) } => background(self, class, "texture"),
            Kind::Container { class: None } => {}
            Kind::Button { class, enabled, selected, .. } => {
                let state = if !enabled {
                    "disabled"
                } else if ui.pressed == Some(id) || *selected {
                    "pressed"
                } else if hovered {
                    "hover"
                } else {
                    "texture"
                };
                background(self, class, state);
            }
            Kind::Text(t) => {
                let px = scale * ui.text_unit * t.scale;
                let parent = ui.nodes.iter().position(|p| p.children.contains(&id));
                let parent_hover = parent.is_some_and(|p| ui.hovered == Some(p));
                let parent_disabled = parent.is_some_and(|p| matches!(ui.nodes[p].kind, Kind::Button { enabled: false, .. }));
                let state = if parent_disabled { "disabled_text" } else if parent_hover { "hover_text" } else { "text" };
                // Generic classes inherit from the surface: `default` its text color, `dim` its
                // dim color.
                let class = match (t.class.as_str(), surface) {
                    ("default", Some(s)) => s,
                    _ => t.class.as_str(),
                };
                let state = if t.class == "dim" && surface.is_some_and(|s| theme.get(s, "dim_text").is_some()) { "dim_text" } else { state };
                let class = if state == "dim_text" { surface.unwrap_or(class) } else { class };
                let default = t.color.unwrap_or_else(|| theme.color(class, state, theme.color(class, "text", 0xFFFFFFFF)));
                // `none` turns off a shadow the default class would give (dark text on light panels).
                let shadow = if parent_disabled { None } else { theme.get(class, "shadow").filter(|v| *v != "none").map(|_| theme.color(class, "shadow", 0x3F3F3FFF)) };
                let x = if t.center { r.x + (r.w - t.layout.width * px) / 2.0 } else { r.x };
                self.load_images(gpu, assets, &t.layout);
                self.glyph_run(&t.layout.items, x, r.y, px, default, shadow, icons);
            }
            Kind::Image { texture, color } => {
                if let Some((idx, _, _)) = self.texture(gpu, assets, texture) {
                    self.image(r, idx, [0.0, 0.0, 1.0, 1.0], *color);
                }
            }
            Kind::Item { last, .. } => {
                if last.0 != 0 {
                    if last.1 > 1 {
                        self.icon_counts.push((icons.len(), last.1, scale * ui.text_unit));
                    }
                    icons.push((r.x, r.y, r.w.min(r.h), last.0));
                }
            }
            Kind::Progress { class, value, dir, .. } => {
                background(self, class, "background");
                let fill = theme.get(class, "fill_texture").map(str::to_string).and_then(|n| self.texture(gpu, assets, &n));
                let v = value.clamp(0.0, 1.0);
                let (fr, uv) = match *dir {
                    weft_abi::ui_dir::UP => (Rect { y: r.y + r.h * (1.0 - v), h: r.h * v, ..r }, [0.0, 1.0 - v, 1.0, 1.0]),
                    weft_abi::ui_dir::LEFT => (Rect { x: r.x + r.w * (1.0 - v), w: r.w * v, ..r }, [1.0 - v, 0.0, 1.0, 1.0]),
                    weft_abi::ui_dir::DOWN => (Rect { h: r.h * v, ..r }, [0.0, 0.0, 1.0, v]),
                    _ => (Rect { w: r.w * v, ..r }, [0.0, 0.0, v, 1.0]),
                };
                match fill {
                    Some((idx, _, _)) => self.image(fr, idx, uv, 0xFFFFFFFF),
                    None => self.rect(fr, theme.color(class, "fill", 0x60C060FF)),
                }
            }
            Kind::Icons { class, count, value, rtl, .. } if *value >= 0.0 => {
                let names = ["full", "half", "empty"].map(|s| theme.get(class, s).map(str::to_string));
                let tex = names.map(|n| n.and_then(|n| self.texture(gpu, assets, &n)));
                for i in 0..*count {
                    let v = *value - i as f32 * 2.0;
                    let which = if v >= 2.0 { 0 } else if v >= 1.0 { 1 } else { 2 };
                    let slot = if *rtl { count - 1 - i } else { i };
                    let rr = Rect { x: r.x + slot as f32 * 8.0 * scale, y: r.y, w: 9.0 * scale, h: 9.0 * scale };
                    if let Some((idx, _, _)) = tex[2] {
                        self.image(rr, idx, [0.0, 0.0, 1.0, 1.0], 0xFFFFFFFF);
                    }
                    if which < 2 {
                        if let Some((idx, _, _)) = tex[which] {
                            self.image(rr, idx, [0.0, 0.0, 1.0, 1.0], 0xFFFFFFFF);
                        } else {
                            self.rect(rr, theme.color(class, "fill", 0xD02020FF));
                        }
                    }
                }
            }
            Kind::Icons { .. } => {}
            Kind::Slider { value, .. } => {
                let k = Rect { x: r.x + (r.w - 8.0 * scale) * value.clamp(0.0, 1.0), y: r.y, w: 8.0 * scale, h: r.h };
                let fallback = theme_slicing(theme, "slider");
                let track = theme.get("slider", "track").map(str::to_string);
                if !track.is_some_and(|n| self.draw_gui(gpu, assets, &n, r, scale, 0xFFFFFFFF, fallback)) {
                    self.rect(Rect { x: r.x, y: r.y + r.h * 0.4, w: r.w, h: r.h * 0.2 }, theme.color("slider", "track", 0x404040FF));
                }
                let knob = theme.get("slider", if hovered { "knob_hover" } else { "knob" }).or_else(|| theme.get("slider", "knob")).map(str::to_string);
                if !knob.is_some_and(|n| self.draw_gui(gpu, assets, &n, k, scale, 0xFFFFFFFF, fallback)) {
                    self.rect(k, theme.color("slider", if hovered { "knob_hover" } else { "knob" }, 0xC0C0C0FF));
                }
            }
            Kind::Input { value, caret, .. } => {
                background(self, "input", if ui.focused == Some(id) { "focused" } else { "texture" });
                let px = scale * ui.text_unit;
                let spans = super::text::plain(value);
                let layout = super::text::layout(&spans, &self.font, None);
                let before: String = value.chars().take(*caret).collect();
                let caret_x = super::text::layout(&super::text::plain(&before), &self.font, None).width * px;
                // Text longer than the field slides left so the caret stays visible; the rest is
                // clipped to the field.
                let room = r.w - 8.0 * scale;
                let shift = (caret_x - room + px).max(0.0);
                let tx = r.x + 4.0 * scale - shift;
                let ty = r.y + (r.h - 16.0 * px) / 2.0;
                let start = self.quads.len();
                if let Some(a) = ui.anchor.filter(|&a| ui.focused == Some(id) && a != *caret) {
                    let x_at = |n: usize| super::text::layout(&super::text::plain(&value.chars().take(n).collect::<String>()), &self.font, None).width * px;
                    let (x0, x1) = (x_at(a.min(*caret)), x_at(a.max(*caret)));
                    self.rect(Rect { x: tx + x0, y: ty, w: x1 - x0, h: 16.0 * px }, theme.color("input", "selection", 0x3060C0C0));
                }
                self.glyph_run(&layout.items, tx, ty, px, theme.color("input", "text", 0xFFFFFFFF), None, icons);
                if ui.focused == Some(id) {
                    self.rect(Rect { x: tx + caret_x, y: ty, w: px.max(1.0), h: 16.0 * px }, 0xFFFFFFFF);
                }
                let field = [r.x + 2.0 * scale, r.y, r.x + r.w - 2.0 * scale, r.y + r.h];
                for q in &mut self.quads[start..] {
                    q.clip = field;
                }
            }
            Kind::Custom { tag, data } => {
                for (class, state) in [("slot", "texture"), ("hotbar", "texture"), ("well", "texture")] {
                    if let Some(name) = theme.get(class, state).map(str::to_string) {
                        if let Some(t) = self.texture(gpu, assets, &name) {
                            self.declared_slicing(assets, &name, [t.1 as f32, t.2 as f32]);
                        }
                    }
                }
                let icon = n.icon.clone().and_then(|i| self.texture(gpu, assets, &i)).map(|t| t.0);
                custom(self, *tag, *data, r, hovered, icon)
            }
        }
        // Children of a themed panel or button read text colors from it.
        let own = match &n.kind {
            Kind::Container { class: Some(c) } | Kind::Button { class: c, .. } if theme.get(c, "text").is_some() => Some(c.as_str()),
            _ => None,
        };
        let inner_surface = own.or(surface);
        let clip = n.clip.map_or([0.0; 4], |c| [c.x * scale, c.y * scale, (c.x + c.w) * scale, (c.y + c.h) * scale]);
        if n.clip.is_some() {
            for q in &mut self.quads[quad_start..] {
                // Quads may carry their own clip (text fields): keep the overlap of both.
                q.clip = if q.clip[2] > q.clip[0] {
                    [q.clip[0].max(clip[0]), q.clip[1].max(clip[1]), q.clip[2].min(clip[2]), q.clip[3].min(clip[3])]
                } else {
                    clip
                };
            }
        }
        self.icon_clips.resize(icon_start, [0.0; 4]);
        self.icon_clips.resize(icons.len(), clip);
        for &c in &n.children {
            self.node(gpu, assets, ui, theme, c, scale, icons, custom, inner_surface);
        }
        if let Some(bar) = n.scrollbar {
            let start = self.quads.len();
            let px = |r: super::Rect| Rect { x: r.x * scale, y: r.y * scale, w: r.w * scale, h: r.h * scale };
            let track = px(super::Rect { x: bar.track.x + (bar.track.w - 6.0) / 2.0, w: 6.0, ..bar.track });
            if !theme.get("scrollbar", "track").map(str::to_string).is_some_and(|t| self.draw_gui(gpu, assets, &t, track, scale, 0xFFFFFFFF, Slicing::Stretch)) {
                self.rect(track, theme.color("scrollbar", "track", 0x00000060));
            }
            let handle = bar.handle();
            let over = ui.dragging_bar() == Some(id);
            let name = theme.get("scrollbar", if over { "handle_hover" } else { "handle" }).or_else(|| theme.get("scrollbar", "handle")).map(str::to_string);
            if !name.is_some_and(|t| self.draw_gui(gpu, assets, &t, px(handle), scale, 0xFFFFFFFF, Slicing::Stretch)) {
                self.rect(px(handle), theme.color("scrollbar", "handle", 0xC0C0C0FF));
            }
            if n.clip.is_some() {
                for q in &mut self.quads[start..] {
                    q.clip = clip;
                }
            }
        }
        let class = match &n.kind {
            Kind::Container { class: Some(c) } | Kind::Button { class: c, .. } | Kind::Progress { class: c, .. } | Kind::Icons { class: c, .. } => Some(c.as_str()),
            Kind::Text(t) => Some(t.class.as_str()),
            _ => None,
        };
        // Item icons are queued apart and do not follow these animations yet.
        if let Some(file) = class.and_then(|c| theme.get(c, "animation")) {
            let list = self.anims.entry(file.to_string()).or_insert_with(|| {
                let src = assets.read(file, "animations", "animation.json").map(|b| String::from_utf8_lossy(&b).into_owned());
                match src.map(|s| crate::model::load_animations(&s)) {
                    Some(Ok(list)) => list.into_iter().map(Arc::new).collect(),
                    Some(Err(e)) => {
                        log::error!("ui animation {file}: {e}");
                        Vec::new()
                    }
                    None => {
                        log::error!("ui animation {file}: no such file");
                        Vec::new()
                    }
                }
            });
            let pick = |name: &str| list.iter().find(|a| a.name == name).cloned();
            let (open, idle) = (pick("open"), pick("idle"));
            if open.is_some() || idle.is_some() {
                let center = [r.x + r.w / 2.0, r.y + r.h / 2.0];
                self.animated.push(Animated { quads: quad_start..self.quads.len(), open, idle, since: ui.shown, center, scale });
            }
        }
    }

    pub fn draw(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, frame: &FrameInfo, gui_scale: f32) {
        if self.quads.is_empty() {
            return;
        }
        let buf = crate::gpu_layout::frame_buffer(gpu, &mut self.buffers, frame.slot, self.quads.len() as u32, size_of::<Quad>() as u64, usage::STORAGE, "ui quads");
        gpu.mapped(buf)[..self.quads.len() * size_of::<Quad>()].copy_from_slice(bytemuck::cast_slice(&self.quads));
        let sb = self.screen[frame.slot];
        let time = crate::shaders::time();
        gpu.mapped(sb)[..16].copy_from_slice(bytemuck::cast_slice(&[frame.width as f32, frame.height as f32, time, gui_scale]));
        list.pipeline(self.pipeline);
        list.bind(0, buf);
        list.bind(1, sb);
        list.draw(self.quads.len() as u32 * 6, 1);
    }

    pub fn destroy(&mut self, gpu: &mut dyn Backend) {
        gpu.destroy_texture(self.font_tex);
    }
}

/// What the theme says for textures that declare nothing: `<class>.tile = n` or `<class>.border`.
fn theme_slicing(theme: &Theme, class: &str) -> Slicing {
    match theme.get(class, "tile") {
        Some(_) => {
            let t = theme.num(class, "tile", 32.0);
            Slicing::Tile { size: [t, t] }
        }
        None => Slicing::Nine { size: [0.0; 2], border: [theme.num(class, "border", 3.0); 4] },
    }
}

#[cfg(test)]
mod slicing_tests {
    use super::*;

    #[test]
    fn gui_sidecar() {
        assert_eq!(parse_gui("border = 2 3 4 5\n", [16.0, 16.0]), Slicing::Nine { size: [16.0, 16.0], border: [2.0, 3.0, 4.0, 5.0] });
        assert_eq!(parse_gui("scaling = tile\nsize = 32 32", [64.0, 64.0]), Slicing::Tile { size: [32.0, 32.0] });
    }
}

#[cfg(test)]
mod animation_tests {
    use super::*;

    #[test]
    fn keyframes_move_quads() {
        let src = r#"{"animations":{"animation.ui.idle":{"loop":true,"animation_length":2,"bones":{"node":{
            "position":{"0":[2,1,0]},"rotation":{"0":[0,0,90]}}}}}}"#;
        let idle = crate::model::load_animations(src).unwrap().pop().map(Arc::new);
        let a = Animated { quads: 0..1, open: None, idle, since: 0.0, center: [10.0, 10.0], scale: 2.0 };
        let mut q = [Quad { p0: [12.0, 10.0], u: [4.0, 0.0], v: [0.0, 4.0], ..Default::default() }];
        crate::shaders::pin_time(true);
        animate(&mut q, &[a]);
        // A quarter turn clockwise about the center, then 2 and 1 GUI pixels right and up.
        let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        assert!(near(q[0].p0, [14.0, 10.0]) && near(q[0].u, [0.0, 4.0]) && near(q[0].v, [-4.0, 0.0]), "{:?} {:?} {:?}", q[0].p0, q[0].u, q[0].v);
    }
}
