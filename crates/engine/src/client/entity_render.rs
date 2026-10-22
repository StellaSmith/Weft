//! Entities as lists of textured boxes built on the CPU each frame (a few hundred at most), plus
//! the block selection outline and break progress overlay.

use glam::{DVec3, IVec3, Mat4, Quat, Vec3, Vec4};
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
    skins: FxHashMap<String, Option<(TextureId, u32)>>,
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
        Ok(EntityRenderer { pipeline, overlay, buffers: Vec::new(), views, overlay_views, skins: FxHashMap::default(), fog: ([0.0; 4], 0.0, 1.0) })
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
        for b in self.views.drain(..).chain(self.overlay_views.drain(..)) {
            gpu.destroy_buffer(b);
        }
        for t in self.skins.drain().filter_map(|(_, v)| v) {
            gpu.destroy_texture(t.0);
        }
        gpu.destroy_pipeline(self.pipeline);
        gpu.destroy_pipeline(self.overlay);
    }

    /// Loads entity skins the first time an entity of that kind becomes visible.
    pub fn preload(&mut self, gpu: &mut dyn Backend, assets: &Assets, entities: &FxHashMap<u64, ClientEntity>) {
        let c = content();
        for e in entities.values() {
            let k = &c.game.entities[e.kind as usize];
            if k.texture.is_empty() || self.skins.contains_key(&k.texture) {
                continue;
            }
            let loaded = assets.read(&k.texture, "textures", "png").and_then(|b| decode_png(&b)).map(|img| {
                let t = gpu.create_texture(
                    &TextureDesc { width: img.width, height: img.height, layers: None, mips: 1, format: Format::Rgba8Srgb, render_target: false },
                    &k.texture,
                );
                gpu.write_texture(t, 0, &img.rgba);
                (t, gpu.texture_index(t))
            });
            if loaded.is_none() {
                log::warn!("missing entity texture {}", k.texture);
            }
            self.skins.insert(k.texture.clone(), loaded);
        }
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
        light: &impl Fn(IVec3) -> u32,
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

        for e in entities.values() {
            let k = &c.game.entities[e.kind as usize];
            let base = rel(e.pos);
            if base.length_squared() > 160.0 * 160.0 {
                continue;
            }
            let color = shade(e.pos + DVec3::Y * 0.5).extend((e.hurt / 0.4).clamp(0.0, 1.0));
            if !e.item.is_empty() {
                item_boxes(&mut boxes, textures, base, e.age, e.item.item, color);
                continue;
            }
            let Some(Some((_, skin))) = self.skins.get(&k.texture) else { continue };
            let tex_size = Vec4::new(0.0, 0.0, k.texture_size[0] as f32, k.texture_size[1] as f32);
            let body = Mat4::from_translation(base) * Mat4::from_rotation_y(e.yaw) * Mat4::from_scale(Vec3::splat(1.0 / 16.0));
            let swing = (e.walk * 6.0).sin() * 0.9 * e.swing;
            for part in &k.parts {
                let angle = match part.role {
                    role::HEAD => Quat::from_rotation_x(-e.pitch),
                    role::ARM_LEFT | role::LEG_RIGHT | role::LEG_FRONT_LEFT => Quat::from_rotation_x(swing),
                    role::ARM_RIGHT | role::LEG_LEFT | role::LEG_FRONT_RIGHT => Quat::from_rotation_x(-swing),
                    role::ARM_FORWARD_LEFT => Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2 + swing * 0.2),
                    role::ARM_FORWARD_RIGHT => Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2 - swing * 0.2),
                    _ => Quat::IDENTITY,
                };
                let pivot = Vec3::from_array(part.pivot);
                let part_m = body * Mat4::from_translation(pivot) * Mat4::from_quat(angle);
                for cu in &part.cuboids {
                    let o = Vec3::from_array(cu.origin) - Vec3::splat(cu.inflate);
                    let s = Vec3::from_array(cu.size) + Vec3::splat(cu.inflate * 2.0);
                    let m = part_m * Mat4::from_translation(o) * Mat4::from_scale(s);
                    boxes.push(GpuBox {
                        rows: rows(m),
                        dims: [cu.size[0], cu.size[1], cu.size[2], 0.0],
                        uvo: [cu.uv[0], cu.uv[1], tex_size.z, tex_size.w],
                        color: color.to_array(),
                        ids: [*skin, 0, 0, 0],
                        ids2: [0; 4],
                    });
                }
            }
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
