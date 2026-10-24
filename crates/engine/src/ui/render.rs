//! Turns a laid-out `Ui` into quads. Rebuilt only when the tree is dirty; the GPU copy is reused
//! across frames otherwise.

use rustc_hash::FxHashMap;
use voxide_render::*;

use super::font::Font;
use super::text::Placed;
use super::theme::Theme;
use super::{Kind, Rect, Ui};
use crate::assets::{Assets, decode_png};
use crate::client::textures::GameTextures;
use crate::content::Content;

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Quad {
    pub p0: [f32; 2],
    pub u: [f32; 2],
    pub v: [f32; 2],
    pub pad: [f32; 2],
    pub uv: [f32; 4],
    pub color: u32,
    pub tex: u32,
    pub layer: u32,
    pub mode: u32,
    /// Visible rectangle in physical pixels; empty (all zero) means unclipped.
    pub clip: [f32; 4],
}

/// How a GUI texture fills a rectangle of another size. Set by the texture itself: a `.gui` file
/// next to it (`scaling = nine_slice | tile | stretch`, `border = 3` or `border = l t r b`,
/// `size = w h` when the image is a higher resolution than the GUI pixels it stands for), or
/// vanilla's `.png.mcmeta` with `gui.scaling`. Themes only give a fallback for bare images.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slicing {
    /// Corners keep their size, edges and centre stretch; borders left, top, right, bottom in GUI
    /// pixels of an image `size` GUI pixels big.
    Nine { size: [f32; 2], border: [f32; 4] },
    Tile { size: [f32; 2] },
    Stretch,
}

/// The number after `"key":` in a JSON text, without a full parser.
fn json_num(src: &str, key: &str) -> Option<f32> {
    let i = src.find(&format!("\"{key}\""))?;
    let rest = src[i + key.len() + 2..].trim_start().strip_prefix(':')?.trim_start();
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn parse_mcmeta(src: &str) -> Option<Slicing> {
    let s = &src[src.find("\"scaling\"")?..];
    let kind = s.find("\"type\"").map(|i| &s[i + 6..]).and_then(|r| r.split('"').nth(1)).unwrap_or("stretch");
    let size = [json_num(s, "width")?, json_num(s, "height")?];
    Some(match kind {
        "nine_slice" => {
            let border = match json_num(s, "border") {
                Some(b) => [b; 4],
                None => [json_num(s, "left")?, json_num(s, "top")?, json_num(s, "right")?, json_num(s, "bottom")?],
            };
            Slicing::Nine { size, border }
        }
        "tile" => Slicing::Tile { size },
        _ => Slicing::Stretch,
    })
}

fn parse_gui(src: &str, pixels: [f32; 2]) -> Slicing {
    let mut kind = "nine_slice";
    let mut border = [0.0f32; 4];
    let mut size = pixels;
    for line in src.lines() {
        let line = line.split('#').next().unwrap_or("");
        let Some((k, v)) = line.split_once('=') else { continue };
        let nums: Vec<f32> = v.split_whitespace().filter_map(|n| n.parse().ok()).collect();
        match k.trim() {
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
}

fn load_shader() -> Result<Shader, ShaderError> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders/ui.wgsl");
    let src = std::fs::read_to_string(path).unwrap_or_else(|_| include_str!("../../shaders/ui.wgsl").to_string());
    Shader::from_wgsl("ui.wgsl", &src)
}

impl UiRenderer {
    pub fn new(gpu: &mut dyn Backend, assets: &Assets) -> Result<UiRenderer, String> {
        let pipeline = Self::pipeline(gpu)?;
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
        })
    }

    /// `core:fonts/default` (atlas and its `.font`) over `core:fonts/unifont`; packs can replace either.
    fn load_font(gpu: &mut dyn Backend, assets: &Assets) -> Option<(Font, TextureId, u32)> {
        let builtin = |f: &str| std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts").join(f)).ok();
        let hex = assets.read("core:fonts/unifont", "", "hex").or_else(|| builtin("unifont.hex"))?;
        let desc = assets.read("core:fonts/default", "", "font").or_else(|| builtin("default.font"));
        let img = assets.read("core:fonts/default", "", "png").or_else(|| builtin("default.png")).and_then(|b| crate::assets::decode_png(&b));
        let desc = desc.map(|d| String::from_utf8_lossy(&d).into_owned());
        let font = Font::new(&String::from_utf8_lossy(&hex), desc.as_deref().zip(img.as_ref()));
        let tex = gpu.create_texture(
            &TextureDesc { width: font.atlas_width, height: font.atlas_height, layers: None, mips: 1, format: Format::R8Unorm, render_target: false },
            "font",
        );
        gpu.write_texture(tex, 0, &font.atlas);
        let idx = gpu.texture_index(tex);
        Some((font, tex, idx))
    }

    fn pipeline(gpu: &mut dyn Backend) -> Result<PipelineId, String> {
        let shader = load_shader().map_err(|e| e.0)?;
        gpu.create_pipeline(&PipelineDesc {
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
        .map_err(|e| e.0)
    }

    pub fn reload(&mut self, gpu: &mut dyn Backend, assets: &Assets) {
        if let Some((font, tex, idx)) = Self::load_font(gpu, assets) {
            gpu.destroy_texture(self.font_tex);
            (self.font, self.font_tex, self.font_idx) = (font, tex, idx);
        }
        match Self::pipeline(gpu) {
            Ok(p) => {
                gpu.destroy_pipeline(self.pipeline);
                self.pipeline = p;
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
    }

    /// Bindless index and size of a theme/GUI texture, loaded the first time it is used.
    pub fn texture(&mut self, gpu: &mut dyn Backend, assets: &Assets, name: &str) -> Option<(u32, u32, u32)> {
        if let Some(t) = self.textures.get(name) {
            return *t;
        }
        let loaded = assets.read(name, "textures", "png").and_then(|b| decode_png(&b)).map(|img| {
            let t = gpu.create_texture(
                &TextureDesc { width: img.width, height: img.height, layers: None, mips: 1, format: Format::Rgba8Srgb, render_target: false },
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
            src.lines().find_map(|l| l.split('#').next()?.split_once('=').filter(|(k, _)| k.trim() == "outset").and_then(|(_, v)| v.trim().parse().ok()))
        });
        self.outset.insert(name.to_string(), outset.unwrap_or(0.0));
        let s = gui.map(|src| parse_gui(&src, pixels)).or_else(|| read("png.mcmeta").and_then(|src| parse_mcmeta(&src)));
        self.slicing.insert(name.to_string(), s);
        s
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

    pub fn rect(&mut self, r: Rect, color: u32) {
        self.quads.push(Quad { p0: [r.x, r.y], u: [r.w, 0.0], v: [0.0, r.h], uv: [0.0; 4], color, mode: 0, ..Default::default() });
    }

    pub fn image(&mut self, r: Rect, tex: u32, uv: [f32; 4], color: u32) {
        self.quads.push(Quad { p0: [r.x, r.y], u: [r.w, 0.0], v: [0.0, r.h], uv, color, tex, mode: 1, ..Default::default() });
    }

    /// 9-slice: corners keep their size (scaled), edges and centre stretch.
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
            for it in items {
                match *it {
                    Placed::Glyph { x: gx, y: gy, ch, color } => {
                        if ch == ' ' {
                            continue;
                        }
                        let g = self.font.glyph(ch);
                        let uv = self.font.uv(g);
                        let w = (g.advance as f32 - 1.0) * px;
                        let c = if pass {
                            shadow.unwrap()
                        } else {
                            color.map_or(default_color, |c| c << 8 | (default_color & 0xFF))
                        };
                        // Whole physical pixels: centred text starts mid-pixel, and nearest sampling
                        // at texel borders would drop or double glyph columns.
                        self.quads.push(Quad {
                            p0: [(x + gx * px + off).round(), (y + gy * px + off).round()],
                            u: [w, 0.0],
                            v: [0.0, 16.0 * px],
                            uv,
                            color: c,
                            tex: self.font_idx,
                            mode: 3,
                            ..Default::default()
                        });
                    }
                    Placed::Icon { x: ix, y: iy, item } if !pass => icons.push((x + ix * px, y + iy * px, 16.0 * px, item)),
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
            let layer = tex.item_layers.get(def.icon as usize).map_or(0, |l| l.0);
            self.quads.push(Quad {
                p0: [x, y],
                u: [size, 0.0],
                v: [0.0, size],
                uv: [0.0, 0.0, 1.0, 1.0],
                color: 0xFFFFFFFF,
                tex: tex.item_index,
                layer,
                mode: 2,
                ..Default::default()
            });
            return;
        }
        let Some(state) = def.block else { return };
        let s = &c.states[state as usize];
        let layer = |name_idx: u32| tex.block_layers.get(name_idx as usize).map_or(0, |l| l.0);
        let tint = |face: usize| -> u32 {
            if s.tint == 0 || s.tint_faces & (1 << face) == 0 {
                return 0xFFFFFFFF;
            }
            let rgb = match s.tint {
                1 => 0x8db360,
                2 => 0x77ab2f,
                _ => 0x3f76e4,
            };
            rgb << 8 | 0xFF
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
                tex: tex.block_index,
                layer: layer(t),
                mode: 2,
                ..Default::default()
            });
            return;
        }
        // Isometric: block point (X, Y, Z) lands at origin + X*ex + Y*ey + Z*ez.
        let (h, q) = (size / 2.0, size / 4.0);
        let o = [x + h, y + size * 0.03 + h];
        let at = |p: [f32; 3]| [o[0] + (p[0] - p[2]) * h, o[1] + (p[0] + p[2]) * q - p[1] * h];
        let grass = match s.tint {
            1 => 0x8db360,
            2 => 0x77ab2f,
            _ => 0x3f76e4,
        } << 8 | 0xFF;
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
                self.quads.push(Quad { p0, u, v, uv, color: shade(tint(face), k), tex: tex.block_index, layer: layer(b.textures[face]), mode: 2, ..Default::default() });
                // Tinted overlay on top of its face.
                if let Some(&ov) = tex.overlays.get(b.textures[face] as usize).filter(|&&ov| ov != 0) {
                    let color = shade(if s.tint == 0 { 0xFFFFFFFF } else { grass }, k);
                    self.quads.push(Quad { p0, u, v, uv, color, tex: tex.block_index, layer: layer(ov - 1), mode: 2, ..Default::default() });
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
        let mut icons = Vec::new();
        self.node(gpu, assets, ui, theme, 0, scale, &mut icons, custom, None);
        if let Some((c, t)) = game {
            let clips = std::mem::take(&mut self.icon_clips);
            for (i, (x, y, s, item)) in icons.into_iter().enumerate() {
                let start = self.quads.len();
                self.item_icon(c, t, x, y, s, item);
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
        self.glyph_run(&layout.items, x + pad, y + pad, px, color, Some(0x3F3F3FFF), &mut icons);
        if let Some((c, t)) = game {
            for (ix, iy, s, item) in icons {
                self.item_icon(c, t, ix, iy, s, item);
            }
        }
    }

    /// One line of text centred on `cx` with its top at `y` (physical pixels); `alpha` fades it.
    /// Plain text centred on `cx` in one colour (`0xRRGGBBAA`) with a shadow.
    pub fn label_colored(&mut self, text: &str, cx: f32, y: f32, px: f32, color: u32) {
        let layout = super::text::layout(&super::text::plain(text), &self.font, None);
        let mut icons = Vec::new();
        let x = (cx - layout.width * px / 2.0).round();
        self.glyph_run(&layout.items, x, y.round(), px, color, Some(0x00000000 | (color & 0xFF) / 2), &mut icons);
    }

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
            let name = theme.get(class, state).or_else(|| theme.get(class, "texture")).map(str::to_string);
            let color = theme.color(class, "color", 0xFFFFFFFF);
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
                // Generic classes inherit from the surface: `default` its text colour, `dim` its
                // dim colour.
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
                self.glyph_run(&t.layout.items, x, r.y, px, default, shadow, icons);
            }
            Kind::Image { texture, color } => {
                if let Some((idx, _, _)) = self.texture(gpu, assets, texture) {
                    self.image(r, idx, [0.0, 0.0, 1.0, 1.0], *color);
                }
            }
            Kind::Item { last, .. } => {
                if last.0 != 0 {
                    icons.push((r.x, r.y, r.w.min(r.h), last.0));
                }
            }
            Kind::Progress { class, value, dir, .. } => {
                background(self, class, "background");
                let fill = theme.get(class, "fill_texture").map(str::to_string).and_then(|n| self.texture(gpu, assets, &n));
                let v = value.clamp(0.0, 1.0);
                let (fr, uv) = match *dir {
                    voxide_abi::ui_dir::UP => (Rect { y: r.y + r.h * (1.0 - v), h: r.h * v, ..r }, [0.0, 1.0 - v, 1.0, 1.0]),
                    voxide_abi::ui_dir::LEFT => (Rect { x: r.x + r.w * (1.0 - v), w: r.w * v, ..r }, [1.0 - v, 0.0, 1.0, 1.0]),
                    voxide_abi::ui_dir::DOWN => (Rect { h: r.h * v, ..r }, [0.0, 0.0, 1.0, v]),
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
                let icon = n.icon.clone().and_then(|i| self.texture(gpu, assets, &i)).map(|t| t.0);
                custom(self, *tag, *data, r, hovered, icon)
            }
        }
        // Children of a themed panel or button read text colours from it.
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
    }

    pub fn draw(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, frame: &FrameInfo) {
        if self.quads.is_empty() {
            return;
        }
        let frames = gpu.frames_in_flight();
        while self.buffers.len() < frames {
            self.buffers.push((gpu.create_buffer(size_of::<Quad>() as u64 * 4096, usage::STORAGE | usage::HOST, "ui quads"), 4096));
        }
        let n = self.quads.len() as u32;
        if self.buffers[frame.slot].1 < n {
            gpu.destroy_buffer(self.buffers[frame.slot].0);
            let cap = n.next_power_of_two();
            self.buffers[frame.slot] = (gpu.create_buffer(size_of::<Quad>() as u64 * cap as u64, usage::STORAGE | usage::HOST, "ui quads"), cap);
        }
        let buf = self.buffers[frame.slot].0;
        gpu.mapped(buf)[..self.quads.len() * size_of::<Quad>()].copy_from_slice(bytemuck::cast_slice(&self.quads));
        let sb = self.screen[frame.slot];
        gpu.mapped(sb)[..16].copy_from_slice(bytemuck::cast_slice(&[frame.width as f32, frame.height as f32, 0.0, 0.0]));
        list.pipeline(self.pipeline);
        list.bind(0, buf);
        list.bind(1, sb);
        list.draw(n * 6, 1);
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
    fn vanilla_mcmeta() {
        let s = r#"{"gui": {"scaling": {"type": "nine_slice", "width": 200, "height": 20, "border": 3}}}"#;
        assert_eq!(parse_mcmeta(s), Some(Slicing::Nine { size: [200.0, 20.0], border: [3.0; 4] }));
        let s = r#"{"gui":{"scaling":{"type":"nine_slice","width":32,"height":32,"border":{"left":4,"top":5,"right":6,"bottom":7}}}}"#;
        assert_eq!(parse_mcmeta(s), Some(Slicing::Nine { size: [32.0, 32.0], border: [4.0, 5.0, 6.0, 7.0] }));
        assert_eq!(parse_mcmeta(r#"{"gui":{"scaling":{"type":"tile","width":16,"height":16}}}"#), Some(Slicing::Tile { size: [16.0, 16.0] }));
    }

    #[test]
    fn gui_sidecar() {
        assert_eq!(parse_gui("border = 2 3 4 5\n", [16.0, 16.0]), Slicing::Nine { size: [16.0, 16.0], border: [2.0, 3.0, 4.0, 5.0] });
        assert_eq!(parse_gui("scaling = tile\nsize = 32 32", [64.0, 64.0]), Slicing::Tile { size: [32.0, 32.0] });
    }
}
