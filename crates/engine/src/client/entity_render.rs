//! Entities as lists of textured boxes built on the CPU each frame (a few hundred at most), plus
//! the block selection outline and break progress overlay.

use glam::{DVec3, IVec3, Mat4, Quat, Vec3, Vec4};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use voxide_abi::{Model, part_role as role};
use voxide_render::*;

use super::ClientEntity;
use super::textures::GameTextures;
use super::world_render::Camera;
use crate::assets::{Assets, decode_png};
use crate::content::content;
use crate::physics::Hit;

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuBox {
    rows: [[f32; 4]; 3],
    dims: [f32; 4],
    uvo: [f32; 4],
    color: [f32; 4],
    ids: [u32; 4],
    ids2: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct View {
    view_proj: [[f32; 4]; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
}

pub struct EntityRenderer {
    pipeline: PipelineId,
    overlay: PipelineId,
    buffers: Vec<(BufferId, u32)>,
    views: Vec<BufferId>,
    overlay_views: Vec<BufferId>,
    preview: Vec<(BufferId, BufferId)>,
    /// Texture, bindless index and size, by name.
    skins: FxHashMap<String, Option<(TextureId, u32, [u32; 2])>>,
    pub fog: ([f32; 4], f32, f32),
}

fn shader() -> Result<Shader, ShaderError> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders/entity.wgsl");
    let src = std::fs::read_to_string(path).unwrap_or_else(|_| include_str!("../../shaders/entity.wgsl").to_string());
    Shader::from_wgsl("entity.wgsl", &src)
}

fn lightmap(v: f32) -> f32 {
    let b = v / (3.0 - 2.0 * v);
    let bright = 1.0 - (1.0 - b).powi(4);
    (b + (bright - b) * 0.5) * 0.96 + 0.04
}

impl EntityRenderer {
    pub fn new(gpu: &mut dyn Backend) -> Result<EntityRenderer, String> {
        let (pipeline, overlay) = Self::pipelines(gpu)?;
        let frames = gpu.frames_in_flight();
        let views = (0..frames).map(|_| gpu.create_buffer(size_of::<View>() as u64, usage::STORAGE | usage::HOST, "entity view")).collect();
        let overlay_views = (0..frames).map(|_| gpu.create_buffer(size_of::<View>() as u64, usage::STORAGE | usage::HOST, "overlay view")).collect();
        let preview = (0..frames)
            .map(|_| {
                let b = gpu.create_buffer(128 * 64, usage::STORAGE | usage::HOST, "preview boxes");
                let v = gpu.create_buffer(size_of::<View>() as u64, usage::STORAGE | usage::HOST, "preview view");
                (b, v)
            })
            .collect();
        Ok(EntityRenderer {
            pipeline,
            overlay,
            buffers: Vec::new(),
            views,
            overlay_views,
            preview,
            skins: FxHashMap::default(),
            fog: ([0.0; 4], 0.0, 1.0),
        })
    }

    fn pipelines(gpu: &mut dyn Backend) -> Result<(PipelineId, PipelineId), String> {
        let s = shader().map_err(|e| e.0)?;
        let main = gpu
            .create_pipeline(&PipelineDesc {
                label: "entities",
                shader: &s,
                vertex: "vs",
                fragment: Some("fs"),
                color: Some(Format::Surface),
                depth: Some(DepthState { write: true, compare: Compare::GreaterEqual }),
                blend: Blend::None,
                cull_back: true,
                topology: Topology::Triangles,
            })
            .map_err(|e| e.0)?;
        let overlay = gpu
            .create_pipeline(&PipelineDesc {
                label: "overlay",
                shader: &s,
                vertex: "vs",
                fragment: Some("fs_overlay"),
                color: Some(Format::Surface),
                depth: Some(DepthState { write: false, compare: Compare::GreaterEqual }),
                blend: Blend::Alpha,
                cull_back: true,
                topology: Topology::Triangles,
            })
            .map_err(|e| e.0)?;
        Ok((main, overlay))
    }

    pub fn reload(&mut self, gpu: &mut dyn Backend) {
        match Self::pipelines(gpu) {
            Ok((a, b)) => {
                gpu.destroy_pipeline(self.pipeline);
                gpu.destroy_pipeline(self.overlay);
                self.pipeline = a;
                self.overlay = b;
            }
            Err(e) => log::error!("entity shader: {e}"),
        }
    }

    pub fn destroy(&mut self, gpu: &mut dyn Backend) {
        for (b, _) in self.buffers.drain(..) {
            gpu.destroy_buffer(b);
        }
        let preview: Vec<BufferId> = self.preview.drain(..).flat_map(|(a, b)| [a, b]).collect();
        for b in self.views.drain(..).chain(self.overlay_views.drain(..)).chain(preview) {
            gpu.destroy_buffer(b);
        }
        for t in self.skins.drain().filter_map(|(_, v)| v) {
            gpu.destroy_texture(t.0);
        }
        gpu.destroy_pipeline(self.pipeline);
        gpu.destroy_pipeline(self.overlay);
    }

    /// Loads skins and worn textures the first time something visible uses them; `worn` is what
    /// the local player wears.
    pub fn preload(&mut self, gpu: &mut dyn Backend, assets: &Assets, entities: &FxHashMap<u64, ClientEntity>, worn: &[u32]) {
        let c = content();
        let mut kinds: Vec<u32> = entities.values().map(|e| e.kind).collect();
        let mut items: Vec<u32> = entities.values().flat_map(|e| e.worn.iter().copied()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        items.sort_unstable();
        items.dedup();
        for k in kinds {
            self.load(gpu, assets, &c.game.entities[k as usize].texture);
        }
        for i in items {
            if let Some((_, t)) = c.worn(i) {
                self.load(gpu, assets, t);
            }
        }
        self.load(gpu, assets, &c.game.entities[c.game.player_kind as usize].texture);
        for &i in worn {
            if let Some((_, t)) = c.worn(i) {
                self.load(gpu, assets, t);
            }
        }
    }

    fn load(&mut self, gpu: &mut dyn Backend, assets: &Assets, name: &str) {
        if name.is_empty() || self.skins.contains_key(name) {
            return;
        }
        let loaded = assets.read(name, "textures", "png").and_then(|b| decode_png(&b)).map(|img| {
            let t = gpu.create_texture(
                &TextureDesc { width: img.width, height: img.height, layers: None, mips: 1, format: Format::Rgba8Srgb, render_target: false },
                name,
            );
            gpu.write_texture(t, 0, &img.rgba);
            (t, gpu.texture_index(t), [img.width, img.height])
        });
        if loaded.is_none() {
            log::warn!("missing entity texture {name}");
        }
        self.skins.insert(name.to_string(), loaded);
    }

    /// Boxes of a model posed at `body` (blocks, camera relative), plus the layers it wears.
    #[allow(clippy::too_many_arguments)]
    fn model(&self, out: &mut Vec<GpuBox>, kind: u32, body: Mat4, pitch: f32, swing: f32, color: Vec4, worn: &[u32]) {
        let c = content();
        let k = &c.game.entities[kind as usize];
        let Some(Some((_, skin, _))) = self.skins.get(&k.texture) else { return };
        // Skin plus up to seven worn layers, without allocating per entity.
        let mut layers = [(0u32, Vec4::ZERO, 0u32, 0f32); 8];
        layers[0] = (*skin, Vec4::new(0.0, 0.0, k.texture_size[0] as f32, k.texture_size[1] as f32), u32::MAX, 0.0);
        let mut n = 1;
        for &i in worn.iter().take(7) {
            if let Some((w, t)) = c.worn(i) {
                if let Some(Some((_, idx, size))) = self.skins.get(t) {
                    layers[n] = (*idx, Vec4::new(0.0, 0.0, size[0] as f32, size[1] as f32), w.roles, w.inflate);
                    n += 1;
                }
            }
        }
        let layers = &layers[..n];
        let body = body * Mat4::from_scale(Vec3::splat(1.0 / 16.0));
        for part in &k.parts {
            let angle = match part.role {
                role::HEAD => Quat::from_rotation_x(-pitch),
                role::ARM_LEFT | role::LEG_RIGHT | role::LEG_FRONT_LEFT => Quat::from_rotation_x(swing),
                role::ARM_RIGHT | role::LEG_LEFT | role::LEG_FRONT_RIGHT => Quat::from_rotation_x(-swing),
                role::ARM_FORWARD_LEFT => Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2 + swing * 0.2),
                role::ARM_FORWARD_RIGHT => Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2 - swing * 0.2),
                _ => Quat::IDENTITY,
            };
            let part_m = body * Mat4::from_translation(Vec3::from_array(part.pivot)) * Mat4::from_quat(angle);
            for &(tex, size, roles, extra) in layers {
                if roles & (1 << part.role) == 0 {
                    continue;
                }
                for cu in &part.cuboids {
                    let inflate = cu.inflate + extra;
                    let o = Vec3::from_array(cu.origin) - Vec3::splat(inflate);
                    let s = Vec3::from_array(cu.size) + Vec3::splat(inflate * 2.0);
                    let m = part_m * Mat4::from_translation(o) * Mat4::from_scale(s);
                    out.push(GpuBox {
                        rows: rows(m),
                        dims: [cu.size[0], cu.size[1], cu.size[2], 0.0],
                        uvo: [cu.uv[0], cu.uv[1], size.z, size.w],
                        color: color.to_array(),
                        ids: [tex, 0, 0, 0],
                        ids2: [0; 4],
                    });
                }
            }
        }
    }

    /// The local player's model inside `rect` (physical pixels) on top of the UI, turned towards
    /// `look` like the inventory figure. Needs its own pass with a cleared depth buffer.
    pub fn draw_preview(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, frame: &FrameInfo, rect: [f32; 4], look: glam::Vec2, worn: &[u32]) {
        let c = content();
        let [x, y, w, h] = rect;
        let (sw, sh) = (frame.width as f32, frame.height as f32);
        let s = h / 2.4;
        let (cx, feet) = (x + w / 2.0, y + h - h * 0.08);
        let head = feet - 1.5 * s;
        let yaw = std::f32::consts::PI - ((look.x - cx) / (s * 1.6)).atan() * 0.7;
        let pitch = ((head - look.y) / (s * 1.6)).atan() * 0.5;
        let proj = Mat4::from_cols(
            Vec4::new(2.0 * s / sw, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 2.0 * s / sh, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.1, 0.0),
            Vec4::new(2.0 * cx / sw - 1.0, 1.0 - 2.0 * feet / sh, 0.5, 1.0),
        );
        let mut boxes = Vec::new();
        self.model(&mut boxes, c.game.player_kind, Mat4::from_rotation_y(yaw), pitch, 0.0, Vec4::new(1.0, 1.0, 1.0, 0.0), worn);
        if boxes.is_empty() || boxes.len() > 64 {
            return;
        }
        let (buf, vb) = self.preview[frame.slot];
        gpu.mapped(buf)[..boxes.len() * 128].copy_from_slice(bytemuck::cast_slice(&boxes));
        let v = View { view_proj: proj.to_cols_array_2d(), fog_color: [0.0; 4], fog: [1e9, 2e9, 0.0, 0.0] };
        gpu.mapped(vb)[..size_of::<View>()].copy_from_slice(bytemuck::bytes_of(&v));
        list.pipeline(self.pipeline);
        list.bind(0, buf);
        list.bind(1, vb);
        list.draw(boxes.len() as u32 * 36, 1);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gpu: &mut dyn Backend,
        list: &mut CommandList,
        frame: &FrameInfo,
        cam: &Camera,
        entities: &FxHashMap<u64, ClientEntity>,
        textures: &GameTextures,
        light: &(impl Fn(IVec3) -> u32 + Sync),
        day: f32,
        target: Option<Hit>,
        breaking: Option<(IVec3, f32)>,
    ) {
        let c = content();
        let aspect = frame.width as f32 / frame.height.max(1) as f32;
        let view_proj = cam.proj(aspect) * cam.view();
        let mut boxes: Vec<GpuBox> = Vec::new();
        let rel = |p: DVec3| (p - cam.pos).as_vec3();
        let shade = |p: DVec3| -> Vec3 {
            let l = light(p.floor().as_ivec3());
            let ch = |s: u32| ((l >> s) & 255).min(15) as f32 / 15.0;
            let sky = lightmap(ch(0) * day);
            Vec3::new(sky.max(lightmap(ch(8))), sky.max(lightmap(ch(16))), sky.max(lightmap(ch(24))))
        };

        // Thousands of mobs: pose them on all cores.
        let all: Vec<&ClientEntity> = entities.values().collect();
        let parts: Vec<Vec<GpuBox>> = all
            .par_chunks(128)
            .map(|chunk| {
                let mut out = Vec::with_capacity(chunk.len() * 8);
                for e in chunk {
                    let base = rel(e.pos);
                    if base.length_squared() > 160.0 * 160.0 {
                        continue;
                    }
                    let color = shade(e.pos + DVec3::Y * 0.5).extend((e.hurt / 0.4).clamp(0.0, 1.0));
                    if !e.item.is_empty() {
                        item_boxes(&mut out, textures, base, e.age, e.item.item, color);
                        continue;
                    }
                    let body = Mat4::from_translation(base) * Mat4::from_rotation_y(e.yaw);
                    let swing = (e.walk * 6.0).sin() * 0.9 * e.swing;
                    self.model(&mut out, e.kind, body, e.pitch, swing, color, &e.worn);
                }
                out
            })
            .collect();
        for p in parts {
            boxes.extend(p);
        }

        // Selection outline: twelve thin dark boxes along the edges of the target's shape.
        if let Some(hit) = target {
            let s = &c.states[hit.state as usize];
            let (mn, mx) = match s.collision.first() {
                Some(b) if s.model != Model::Cube => (Vec3::new(b[0], b[1], b[2]), Vec3::new(b[3], b[4], b[5])),
                _ => match s.boxes.first() {
                    Some(b) if s.model == Model::Boxes => (Vec3::from_array(b.min), Vec3::from_array(b.max)),
                    _ => (Vec3::ZERO, Vec3::ONE),
                },
            };
            let o = rel(hit.block.as_dvec3());
            let t = 0.006;
            let (a, b) = (o + mn - Vec3::splat(t), o + mx + Vec3::splat(t));
            for axis in 0..3 {
                for corner in 0..4 {
                    let mut lo = a;
                    let mut size = Vec3::splat(t * 2.0);
                    size[axis] = b[axis] - a[axis];
                    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                    lo[u] = if corner & 1 == 0 { a[u] } else { b[u] - t * 2.0 };
                    lo[v] = if corner & 2 == 0 { a[v] } else { b[v] - t * 2.0 };
                    let m = Mat4::from_translation(lo) * Mat4::from_scale(size);
                    boxes.push(GpuBox { rows: rows(m), color: [0.02, 0.02, 0.02, 0.0], ids: [0, 2, 0, 0], ..Default::default() });
                }
            }
        }
        let overlay_start = boxes.len();
        let mut overlay_alpha = 0.0;
        if let Some((p, progress)) = breaking {
            if progress > 0.0 {
                let o = rel(p.as_dvec3()) - Vec3::splat(0.004);
                let m = Mat4::from_translation(o) * Mat4::from_scale(Vec3::splat(1.008));
                boxes.push(GpuBox { rows: rows(m), color: [0.0, 0.0, 0.0, 0.0], ids: [0, 2, 0, 0], ..Default::default() });
                overlay_alpha = progress * 0.6;
            }
        }
        if boxes.is_empty() {
            return;
        }

        let frames = gpu.frames_in_flight();
        while self.buffers.len() < frames {
            self.buffers.push((gpu.create_buffer(128 * 1024, usage::STORAGE | usage::HOST, "entity boxes"), 1024));
        }
        let n = boxes.len() as u32;
        if self.buffers[frame.slot].1 < n {
            gpu.destroy_buffer(self.buffers[frame.slot].0);
            let cap = n.next_power_of_two();
            self.buffers[frame.slot] = (gpu.create_buffer(128 * cap as u64, usage::STORAGE | usage::HOST, "entity boxes"), cap);
        }
        let buf = self.buffers[frame.slot].0;
        gpu.mapped(buf)[..boxes.len() * 128].copy_from_slice(bytemuck::cast_slice(&boxes));
        let (fog_color, fog_start, fog_end) = self.fog;
        let v = View { view_proj: view_proj.to_cols_array_2d(), fog_color, fog: [fog_start, fog_end, 0.0, 0.0] };
        let vb = self.views[frame.slot];
        gpu.mapped(vb)[..size_of::<View>()].copy_from_slice(bytemuck::bytes_of(&v));
        let ov = View { fog: [fog_start, fog_end, overlay_alpha, 0.0], ..v };
        let ob = self.overlay_views[frame.slot];
        gpu.mapped(ob)[..size_of::<View>()].copy_from_slice(bytemuck::bytes_of(&ov));

        if overlay_start > 0 {
            list.pipeline(self.pipeline);
            list.bind(0, buf);
            list.bind(1, vb);
            list.draw(overlay_start as u32 * 36, 1);
        }
        if boxes.len() > overlay_start {
            list.pipeline(self.overlay);
            list.bind(0, buf);
            list.bind(1, ob);
            list.push(Cmd::Draw { vertices: (boxes.len() - overlay_start) as u32 * 36, instances: 1, first_vertex: overlay_start as u32 * 36, first_instance: 0 });
        }
    }
}

fn rows(m: Mat4) -> [[f32; 4]; 3] {
    [m.row(0).to_array(), m.row(1).to_array(), m.row(2).to_array()]
}

/// Dropped items: a small spinning block, or a flat sprite for regular items.
fn item_boxes(out: &mut Vec<GpuBox>, textures: &GameTextures, base: Vec3, age: f32, item: u32, color: Vec4) {
    let c = content();
    let Some(def) = c.items.get(item as usize) else { return };
    let bob = (age * 2.5).sin() * 0.06 + 0.15;
    let spin = Mat4::from_translation(base + Vec3::Y * bob) * Mat4::from_rotation_y(age * 1.5);
    let layer = |name: u32| textures.block_layers.get(name as usize).map_or(0, |l| l.0);
    if def.icon == u32::MAX {
        let Some(state) = def.block else { return };
        let s = &c.states[state as usize];
        let faces = if s.model == Model::Boxes { s.boxes.first().map_or([0; 6], |b| b.textures) } else { s.textures };
        if s.model == Model::Cross {
            let l = layer(faces[0]);
            let m = spin * Mat4::from_translation(Vec3::new(-0.2, 0.0, -0.01)) * Mat4::from_scale(Vec3::new(0.4, 0.4, 0.02));
            out.push(GpuBox { rows: rows(m), color: color.to_array(), ids: [textures.block_index, 1, l | l << 16, l | l << 16], ids2: [l | l << 16, 0, 0, 0], ..Default::default() });
            return;
        }
        let l: Vec<u32> = faces.iter().map(|&f| layer(f)).collect();
        let m = spin * Mat4::from_translation(Vec3::splat(-0.125)) * Mat4::from_scale(Vec3::splat(0.25));
        out.push(GpuBox {
            rows: rows(m),
            color: color.to_array(),
            ids: [textures.block_index, 1, l[0] | l[1] << 16, l[2] | l[3] << 16],
            ids2: [l[4] | l[5] << 16, 0, 0, 0],
            ..Default::default()
        });
    } else {
        let l = textures.item_layers.get(def.icon as usize).map_or(0, |x| x.0);
        let m = spin * Mat4::from_translation(Vec3::new(-0.2, 0.0, -0.01)) * Mat4::from_scale(Vec3::new(0.4, 0.4, 0.02));
        out.push(GpuBox { rows: rows(m), color: color.to_array(), ids: [textures.item_index, 1, l | l << 16, l | l << 16], ids2: [l | l << 16, 0, 0, 0], ..Default::default() });
    }
}
