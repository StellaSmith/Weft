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

/// What the first person view holds: the item (or the bare arm), the swing progress and the
/// walk distance for bobbing.
pub struct Hand {
    pub stack: voxide_abi::ItemStack,
    pub swing: Option<f32>,
}

/// Opaque pixels of a sprite, for drawing it as a 1/16 thick solid like vanilla's held items.
struct Sprite {
    size: u32,
    pixels: Vec<(u8, u8)>,
}

/// One entity layer (skin or something worn) for `model.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct ModelInstance {
    pos: [f32; 3],
    yaw: f32,
    pitch: f32,
    swing: f32,
    hurt: f32,
    inflate: f32,
    color: [f32; 3],
    tex: u32,
    tex_size: [f32; 2],
    roles: u32,
    first_cuboid: u32,
    /// Sideways tip-over when dying, radians.
    roll: f32,
    _pad: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuCuboid {
    pivot: [f32; 3],
    role: u32,
    origin: [f32; 3],
    inflate: f32,
    size: [f32; 3],
    rest_x: f32,
    uv: [f32; 2],
    rest_yz: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct View {
    view_proj: [[f32; 4]; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
}

pub struct EntityRenderer {
    hand_pipe: PipelineId,
    pipeline: PipelineId,
    overlay: PipelineId,
    model_pipe: PipelineId,
    /// Every entity kind's cuboids, and where each kind starts and how many it has.
    cuboids: BufferId,
    kind_cuboids: Vec<(u32, u32)>,
    instances: Vec<(BufferId, u32)>,
    buffers: Vec<(BufferId, u32)>,
    views: Vec<BufferId>,
    overlay_views: Vec<BufferId>,
    /// The held item's projection: a fixed field of view, like vanilla, so it keeps its shape
    /// whatever the player's setting.
    hand_views: Vec<BufferId>,
    preview: Vec<(BufferId, BufferId)>,
    /// The inventory figure is drawn into its own texture so the UI can layer it like any image.
    preview_pipe: PipelineId,
    /// Colour, depth, size, bindless index.
    preview_target: Option<(TextureId, TextureId, [u32; 2], u32)>,
    /// Texture, bindless index and size, by name.
    skins: FxHashMap<String, Option<(TextureId, u32, [u32; 2])>>,
    sprites: FxHashMap<String, Option<Sprite>>,
    pub fog: ([f32; 4], f32, f32),
    pub brightness: f32,
}

fn shader() -> Result<Shader, ShaderError> {
    crate::packs::shader("entity.wgsl", include_str!("../../shaders/entity.wgsl"))
}

/// Same as the world shader's `lightmap`; `day` is the daylight fraction.
fn lightmap(l: [f32; 4], day: f32, brightness: f32) -> Vec3 {
    let curve = |v: f32| v / (4.0 - 3.0 * v);
    let t = ((day - 0.12) / 0.88).clamp(0.0, 1.0);
    let sky = curve(l[0]) * (0.24 + 0.76 * t);
    let mut c = Vec3::splat(0.04) + Vec3::new(0.478, 0.478, 1.0).lerp(Vec3::ONE, t) * sky;
    let block = Vec3::new(l[1], l[2], l[3]);
    let p = (block * 2.0 - Vec3::ONE) * (block * 2.0 - Vec3::ONE);
    let tint = Vec3::new(1.0, 0.847, 0.549) * (Vec3::ONE - p * 0.9) + p * 0.9;
    c += tint * Vec3::new(curve(block.x), curve(block.y), curve(block.z)) * 1.4;
    c = c.clamp(Vec3::ZERO, Vec3::ONE);
    let m = c.max_element().max(1e-4);
    c.lerp(c * ((1.0 - (1.0 - m).powi(4)) / m), brightness)
}

impl EntityRenderer {
    pub fn new(gpu: &mut dyn Backend) -> Result<EntityRenderer, String> {
        let (pipeline, overlay) = Self::pipelines(gpu)?;
        let model_pipe = Self::model_pipeline(gpu)?;
        let preview_pipe = Self::preview_pipeline(gpu)?;
        let mut table: Vec<GpuCuboid> = Vec::new();
        let mut kind_cuboids = Vec::new();
        for k in &content().game.entities {
            let first = table.len() as u32;
            for part in &k.parts {
                for cu in &part.cuboids {
                    table.push(GpuCuboid {
                        pivot: part.pivot,
                        role: part.role,
                        origin: cu.origin,
                        inflate: cu.inflate,
                        size: cu.size,
                        rest_x: part.rest[0],
                        uv: cu.uv,
                        rest_yz: [part.rest[1], part.rest[2]],
                    });
                }
            }
            kind_cuboids.push((first, table.len() as u32 - first));
        }
        if table.is_empty() {
            table.push(GpuCuboid::default());
        }
        let cuboids = gpu.create_buffer((table.len() * size_of::<GpuCuboid>()) as u64, usage::STORAGE, "model cuboids");
        gpu.write_buffer(cuboids, 0, bytemuck::cast_slice(&table));
        let frames = gpu.frames_in_flight();
        let views = (0..frames).map(|_| gpu.create_buffer(size_of::<View>() as u64, usage::STORAGE | usage::HOST, "entity view")).collect();
        let overlay_views = (0..frames).map(|_| gpu.create_buffer(size_of::<View>() as u64, usage::STORAGE | usage::HOST, "overlay view")).collect();
        let hand_views = (0..frames).map(|_| gpu.create_buffer(size_of::<View>() as u64, usage::STORAGE | usage::HOST, "hand view")).collect();
        let preview = (0..frames)
            .map(|_| {
                let b = gpu.create_buffer(128 * 64, usage::STORAGE | usage::HOST, "preview boxes");
                let v = gpu.create_buffer(size_of::<View>() as u64, usage::STORAGE | usage::HOST, "preview view");
                (b, v)
            })
            .collect();
        let hand_pipe = Self::hand_pipeline(gpu)?;
        Ok(EntityRenderer {
            pipeline,
            overlay,
            hand_pipe,
            model_pipe,
            cuboids,
            kind_cuboids,
            instances: Vec::new(),
            buffers: Vec::new(),
            views,
            overlay_views,
            hand_views,
            preview,
            preview_pipe,
            preview_target: None,
            skins: FxHashMap::default(),
            sprites: FxHashMap::default(),
            fog: ([0.0; 4], 0.0, 1.0),
            brightness: 0.5,
        })
    }

    /// The first person hand: drawn last and without depth test so it never sinks into walls.
    fn hand_pipeline(gpu: &mut dyn Backend) -> Result<PipelineId, String> {
        let s = shader().map_err(|e| e.0)?;
        gpu.create_pipeline(&PipelineDesc {
            label: "hand",
            shader: &s,
            vertex: "vs",
            fragment: Some("fs"),
            color: Some(Format::Surface),
            depth: None,
            blend: Blend::None,
            cull_back: true,
            topology: Topology::Triangles,
        })
        .map_err(|e| e.0)
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

    fn preview_pipeline(gpu: &mut dyn Backend) -> Result<PipelineId, String> {
        let s = shader().map_err(|e| e.0)?;
        gpu.create_pipeline(&PipelineDesc {
            label: "entity preview",
            shader: &s,
            vertex: "vs",
            fragment: Some("fs"),
            color: Some(Format::Rgba8Srgb),
            depth: Some(DepthState { write: true, compare: Compare::GreaterEqual }),
            blend: Blend::None,
            cull_back: true,
            topology: Topology::Triangles,
        })
        .map_err(|e| e.0)
    }

    fn model_pipeline(gpu: &mut dyn Backend) -> Result<PipelineId, String> {
        let s = crate::packs::shader("model.wgsl", include_str!("../../shaders/model.wgsl")).map_err(|e| e.0)?;
        gpu.create_pipeline(&PipelineDesc {
            label: "models",
            shader: &s,
            vertex: "vs",
            fragment: Some("fs"),
            color: Some(Format::Surface),
            depth: Some(DepthState { write: true, compare: Compare::GreaterEqual }),
            blend: Blend::None,
            cull_back: true,
            topology: Topology::Triangles,
        })
        .map_err(|e| e.0)
    }

    pub fn reload(&mut self, gpu: &mut dyn Backend) {
        match Self::model_pipeline(gpu) {
            Ok(p) => {
                gpu.destroy_pipeline(self.model_pipe);
                self.model_pipe = p;
            }
            Err(e) => log::error!("model shader: {e}"),
        }
        if let Ok(h) = Self::hand_pipeline(gpu) {
            gpu.destroy_pipeline(self.hand_pipe);
            self.hand_pipe = h;
        }
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
        for (b, _) in self.buffers.drain(..).chain(self.instances.drain(..)) {
            gpu.destroy_buffer(b);
        }
        gpu.destroy_buffer(self.cuboids);
        gpu.destroy_pipeline(self.model_pipe);
        gpu.destroy_pipeline(self.hand_pipe);
        gpu.destroy_pipeline(self.preview_pipe);
        if let Some((c, d, ..)) = self.preview_target.take() {
            gpu.destroy_texture(c);
            gpu.destroy_texture(d);
        }
        let preview: Vec<BufferId> = self.preview.drain(..).flat_map(|(a, b)| [a, b]).collect();
        for b in self.views.drain(..).chain(self.overlay_views.drain(..)).chain(self.hand_views.drain(..)).chain(preview) {
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
    pub fn preload(&mut self, gpu: &mut dyn Backend, assets: &Assets, entities: &FxHashMap<u64, ClientEntity>, worn: &[u32], held: u32) {
        let c = content();
        if let Some(name) = sprite_name(held) {
            if !self.sprites.contains_key(&name) {
                let img = assets.read(&name, "textures", "png").and_then(|b| decode_png(&b));
                let sprite = img.map(|img| {
                    let size = img.width;
                    // Animated strips: the first frame.
                    let pixels = (0..size * size)
                        .filter(|&i| img.rgba.get(i as usize * 4 + 3).is_some_and(|&a| a >= 128))
                        .map(|i| ((i % size) as u8, (i / size) as u8))
                        .collect();
                    Sprite { size, pixels }
                });
                self.sprites.insert(name, sprite);
            }
        }
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
            let [ax, ay] = match part.role {
                // Faces look along -Z: tilting the front up is a positive turn about X.
                role::HEAD => [pitch, 0.0],
                role::ARM_LEFT | role::LEG_RIGHT | role::LEG_FRONT_LEFT => [swing, 0.0],
                role::ARM_RIGHT | role::LEG_LEFT | role::LEG_FRONT_RIGHT => [-swing, 0.0],
                role::ARM_FORWARD_LEFT => [std::f32::consts::FRAC_PI_2 + swing * 0.2, 0.0],
                role::ARM_FORWARD_RIGHT => [std::f32::consts::FRAC_PI_2 - swing * 0.2, 0.0],
                role::LEG_SWEEP_A => [0.0, swing * 0.6],
                role::LEG_SWEEP_B => [0.0, -swing * 0.6],
                _ => [0.0, 0.0],
            };
            let [rx, ry, rz] = part.rest;
            let angle = Quat::from_rotation_z(rz) * Quat::from_rotation_y(ry + ay) * Quat::from_rotation_x(rx + ax);
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

    /// Renders the local player's model into a `size` texture (physical pixels), turned towards
    /// `look` (relative to the texture's top left) like the inventory figure. Returns the
    /// texture's bindless index for the UI to draw.
    pub fn draw_preview(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, frame: &FrameInfo, size: [u32; 2], look: glam::Vec2, worn: &[u32]) -> Option<u32> {
        let c = content();
        if size[0] == 0 || size[1] == 0 {
            return None;
        }
        if self.preview_target.is_none_or(|t| t.2 != size) {
            if let Some((col, dep, ..)) = self.preview_target.take() {
                gpu.destroy_texture(col);
                gpu.destroy_texture(dep);
            }
            let desc = |format| TextureDesc { width: size[0], height: size[1], layers: None, mips: 1, format, render_target: true };
            let col = gpu.create_texture(&desc(Format::Rgba8Srgb), "preview colour");
            let dep = gpu.create_texture(&desc(Format::Depth32), "preview depth");
            self.preview_target = Some((col, dep, size, gpu.texture_index(col)));
        }
        let (col, dep, _, index) = self.preview_target?;
        let (x, y, w, h) = (0.0, 0.0, size[0] as f32, size[1] as f32);
        let (sw, sh) = (w, h);
        let s = h / 2.4;
        let (cx, feet) = (x + w / 2.0, y + h - h * 0.08);
        let head = feet - 1.5 * s;
        let yaw = std::f32::consts::PI + ((look.x - cx) / (s * 1.6)).atan() * 0.7;
        let pitch = ((head - look.y) / (s * 1.6)).atan() * 0.5;
        let proj = Mat4::from_cols(
            Vec4::new(2.0 * s / sw, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 2.0 * s / sh, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.1, 0.0),
            Vec4::new(2.0 * cx / sw - 1.0, 1.0 - 2.0 * feet / sh, 0.5, 1.0),
        );
        let mut boxes = Vec::new();
        self.model(&mut boxes, c.game.player_kind, Mat4::from_rotation_y(yaw), pitch, 0.0, Vec4::new(1.0, 1.0, 1.0, 0.0), worn);
        boxes.truncate(64);
        let (buf, vb) = self.preview[frame.slot];
        gpu.mapped(buf)[..boxes.len() * 128].copy_from_slice(bytemuck::cast_slice(&boxes));
        let v = View { view_proj: proj.to_cols_array_2d(), fog_color: [0.0; 4], fog: [1e9, 2e9, 0.0, 0.0] };
        gpu.mapped(vb)[..size_of::<View>()].copy_from_slice(bytemuck::bytes_of(&v));
        list.push(Cmd::BeginPass { color: Some((Target::Texture(col), Some([0.0; 4]))), depth: Some((dep, Some(0.0))) });
        if !boxes.is_empty() {
            list.pipeline(self.preview_pipe);
            list.bind(0, buf);
            list.bind(1, vb);
            list.draw(boxes.len() as u32 * 36, 1);
        }
        list.push(Cmd::EndPass);
        Some(index)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gpu: &mut dyn Backend,
        list: &mut CommandList,
        frame: &FrameInfo,
        cam: &Camera,
        entities: &FxHashMap<u64, ClientEntity>,
        me: Option<&ClientEntity>,
        hand: Option<Hand>,
        puffs: &[super::Puff],
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
            lightmap([ch(0), ch(8), ch(16), ch(24)], day, self.brightness)
        };

        // Models are posed by the vertex shader: one small record per entity and layer, grouped by
        // kind so each kind is a single instanced draw.
        let skin_of: Vec<Option<(u32, [f32; 2])>> = c
            .game
            .entities
            .iter()
            .map(|k| match self.skins.get(&k.texture) {
                Some(Some((_, idx, _))) => Some((*idx, [k.texture_size[0] as f32, k.texture_size[1] as f32])),
                _ => None,
            })
            .collect();
        let mut by_kind: Vec<Vec<ModelInstance>> = vec![Vec::new(); c.game.entities.len()];
        for e in entities.values().chain(me) {
            let base = rel(e.pos);
            if base.length_squared() > 160.0 * 160.0 {
                continue;
            }
            let color = shade(e.pos + DVec3::Y * 0.5);
            let hurt = (e.hurt / 0.4).clamp(0.0, 1.0);
            if !e.item.is_empty() {
                item_boxes(&mut boxes, textures, base, e.age, e.item.item, color.extend(hurt));
                continue;
            }
            let Some((tex, tex_size)) = skin_of[e.kind as usize] else { continue };
            let inst = ModelInstance {
                pos: base.to_array(),
                yaw: e.yaw,
                pitch: e.pitch,
                // Vanilla's limb swing: 0.6662 rad per tick of limbSwing, which grows 4 per block.
                swing: (e.walk * 2.66).sin() * 1.2 * e.swing,
                hurt,
                inflate: 0.0,
                color: color.to_array(),
                tex,
                tex_size,
                roles: u32::MAX,
                first_cuboid: self.kind_cuboids[e.kind as usize].0,
                // Vanilla: sqrt of the death time over a second, up to a quarter turn.
                roll: e.dying.map_or(0.0, |d| (d * 1.6).sqrt().min(1.0) * std::f32::consts::FRAC_PI_2),
                _pad: [0.0; 3],
            };
            if e.burning {
                let k = &c.game.entities[e.kind as usize];
                fire_boxes(&mut boxes, textures, base, k.width, k.height, cam.yaw, e.age);
            }
            let list = &mut by_kind[e.kind as usize];
            list.push(inst);
            for &i in &e.worn {
                if let Some((w, t)) = c.worn(i) {
                    if let Some(Some((_, idx, size))) = self.skins.get(t) {
                        list.push(ModelInstance { tex: *idx, tex_size: [size[0] as f32, size[1] as f32], roles: w.roles, inflate: w.inflate, ..inst });
                    }
                }
            }
        }
        let (fog_color, fog_start, fog_end) = self.fog;
        let v = View { view_proj: view_proj.to_cols_array_2d(), fog_color, fog: [fog_start, fog_end, 0.0, 0.0] };
        let vb = self.views[frame.slot];
        gpu.mapped(vb)[..size_of::<View>()].copy_from_slice(bytemuck::bytes_of(&v));
        let total: usize = by_kind.iter().map(Vec::len).sum();
        if total > 0 {
            let frames = gpu.frames_in_flight();
            let stride = size_of::<ModelInstance>() as u64;
            while self.instances.len() < frames {
                self.instances.push((gpu.create_buffer(stride * 1024, usage::STORAGE | usage::HOST, "model instances"), 1024));
            }
            if (self.instances[frame.slot].1 as usize) < total {
                gpu.destroy_buffer(self.instances[frame.slot].0);
                let cap = total.next_power_of_two() as u32;
                self.instances[frame.slot] = (gpu.create_buffer(stride * cap as u64, usage::STORAGE | usage::HOST, "model instances"), cap);
            }
            let ibuf = self.instances[frame.slot].0;
            let mapped = gpu.mapped(ibuf);
            let mut at = 0usize;
            let mut draws = Vec::new();
            for (kind, list) in by_kind.iter().enumerate() {
                if list.is_empty() {
                    continue;
                }
                let bytes: &[u8] = bytemuck::cast_slice(list);
                mapped[at * stride as usize..at * stride as usize + bytes.len()].copy_from_slice(bytes);
                draws.push((self.kind_cuboids[kind].1 * 36, list.len() as u32, at as u32));
                at += list.len();
            }
            list.pipeline(self.model_pipe);
            list.bind(0, ibuf);
            list.bind(1, vb);
            list.bind(2, self.cuboids);
            for (vertices, instances, first_instance) in draws {
                list.push(Cmd::Draw { vertices, instances, first_vertex: 0, first_instance });
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
        for p in puffs {
            let s = p.size * (1.0 - p.age / p.life);
            let m = Mat4::from_translation(rel(p.pos) - Vec3::splat(s / 2.0)) * Mat4::from_scale(Vec3::splat(s));
            let g = 0.75 - p.age / p.life * 0.3;
            boxes.push(GpuBox { rows: rows(m), color: [g, g, g, 0.0], ids: [0, 2, 0, 0], ..Default::default() });
        }
        let overlay_start = boxes.len();
        let mut overlay_alpha = 0.0;
        if let Some((p, progress)) = breaking {
            if progress > 0.0 {
                let o = rel(p.as_dvec3()) - Vec3::splat(0.004);
                let m = Mat4::from_translation(o) * Mat4::from_scale(Vec3::splat(1.008));
                let stage = ((progress * 10.0) as usize).min(9);
                let l = textures.block_layers.get(c.crack_textures[stage] as usize).map_or(0, |x| x.0);
                let color = shade(p.as_dvec3() + DVec3::splat(0.5));
                boxes.push(GpuBox {
                    rows: rows(m),
                    color: color.extend(0.0).to_array(),
                    ids: [textures.block_index, 1, l | l << 16, l | l << 16],
                    ids2: [l | l << 16, 0, 0, 0],
                    ..Default::default()
                });
                overlay_alpha = 1.0;
            }
        }
        let hand_start = boxes.len();
        if let Some(h) = hand {
            let player_skin = skin_of.get(c.game.player_kind as usize).copied().flatten();
            let sprite = sprite_name(h.stack.item).filter(|_| !h.stack.is_empty()).and_then(|n| self.sprites.get(&n)).and_then(|s| s.as_ref());
            hand_boxes(&mut boxes, textures, cam, &h, player_skin, sprite, shade(cam.pos));
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
        let ov = View { fog: [fog_start, fog_end, overlay_alpha, 0.0], ..v };
        let ob = self.overlay_views[frame.slot];
        gpu.mapped(ob)[..size_of::<View>()].copy_from_slice(bytemuck::bytes_of(&ov));

        if overlay_start > 0 {
            list.pipeline(self.pipeline);
            list.bind(0, buf);
            list.bind(1, vb);
            list.draw(overlay_start as u32 * 36, 1);
        }
        if hand_start > overlay_start {
            list.pipeline(self.overlay);
            list.bind(0, buf);
            list.bind(1, ob);
            list.push(Cmd::Draw { vertices: (hand_start - overlay_start) as u32 * 36, instances: 1, first_vertex: overlay_start as u32 * 36, first_instance: 0 });
        }
        if boxes.len() > hand_start {
            let hand_cam = Camera { fov: 70.0, ..*cam };
            let hv = View { view_proj: (hand_cam.proj(aspect) * cam.view()).to_cols_array_2d(), ..v };
            let hb = self.hand_views[frame.slot];
            gpu.mapped(hb)[..size_of::<View>()].copy_from_slice(bytemuck::bytes_of(&hv));
            list.pipeline(self.hand_pipe);
            list.bind(0, buf);
            list.bind(1, hb);
            list.push(Cmd::Draw { vertices: (boxes.len() - hand_start) as u32 * 36, instances: 1, first_vertex: hand_start as u32 * 36, first_instance: 0 });
        }
    }
}

/// Two layers of flames facing the camera, the inner one smaller, like vanilla's burning mobs.
fn fire_boxes(out: &mut Vec<GpuBox>, textures: &GameTextures, base: Vec3, width: f32, height: f32, cam_yaw: f32, age: f32) {
    let c = content();
    let first = textures.block_layers.get(c.fire_texture as usize).map_or((0, 1), |l| *l);
    let frame = |offset: f32| first.0 + ((age * 20.0 + offset) as u32 % first.1.max(1));
    for (i, (scale, back)) in [(1.4, 0.0), (1.0, 0.2)].into_iter().enumerate() {
        let l = frame(i as f32 * 11.0);
        let w = width * scale;
        let m = Mat4::from_translation(base)
            * Mat4::from_rotation_y(cam_yaw)
            * Mat4::from_translation(Vec3::new(-w / 2.0, 0.0, back * width))
            * Mat4::from_scale(Vec3::new(w, height * scale * 0.95, 0.0));
        out.push(GpuBox { rows: rows(m), color: [1.0, 1.0, 1.0, 0.0], ids: [textures.block_index, 3, l | l << 16, l | l << 16], ids2: [l | l << 16, 0, 0, 0], ..Default::default() });
    }
}

/// Texture a held item is drawn from as a solid sprite: its icon, or the texture of a plant block.
fn sprite_name(item: u32) -> Option<String> {
    let c = content();
    let def = c.items.get(item as usize).filter(|_| item != 0)?;
    if def.icon != u32::MAX {
        return c.item_textures.get(def.icon as usize).cloned();
    }
    let st = &c.states[def.block? as usize];
    st.item_sprite().and_then(|t| c.block_textures.get(t as usize).cloned())
}

/// The held item or the bare right arm in front of the camera, posed like vanilla's
/// ItemInHandRenderer: swing translation, arm offset, attack rotation, then the item model's
/// first person display transform.
fn hand_boxes(out: &mut Vec<GpuBox>, textures: &GameTextures, cam: &Camera, h: &Hand, skin: Option<(u32, [f32; 2])>, sprite: Option<&Sprite>, light: Vec3) {
    use std::f32::consts::PI;
    let c = content();
    let p = h.swing.unwrap_or(0.0);
    let (s, sr) = ((p * PI).sin(), (p.sqrt() * PI).sin());
    // From view space to the camera-relative world: the hand stays put on screen while the
    // bobbing view moves.
    let view = cam.view().inverse();
    let color = light.extend(0.0).to_array();
    let def = c.items.get(h.stack.item as usize).filter(|_| !h.stack.is_empty());
    let Some(def) = def else {
        let Some((tex, size)) = skin else { return };
        let swing = Mat4::from_translation(Vec3::new(-0.4 * sr, 0.2 * (p.sqrt() * PI * 2.0).sin(), -0.2 * s));
        // Right arm (skin box at 40,16, 4x12x4 pixels), raised and turned in from the right.
        let m = view
            * swing
            * Mat4::from_translation(Vec3::new(0.56, -0.78, -0.36))
            * Mat4::from_rotation_y(0.32 + sr * 0.3)
            * Mat4::from_rotation_x(1.85 + s * 0.5)
            * Mat4::from_rotation_z(0.12)
            * Mat4::from_scale(Vec3::splat(1.0 / 16.0))
            * Mat4::from_translation(Vec3::new(-2.0, -10.0, -2.0))
            * Mat4::from_scale(Vec3::new(4.0, 12.0, 4.0));
        out.push(GpuBox { rows: rows(m), dims: [4.0, 12.0, 4.0, 0.0], uvo: [40.0, 16.0, size[0], size[1]], color, ids: [tex, 0, 0, 0], ids2: [0; 4] });
        return;
    };
    let deg = |d: f32| d.to_radians();
    let swing_sin = (p * p * PI).sin();
    let arm = view
        * Mat4::from_translation(Vec3::new(-0.4 * sr, 0.2 * (p.sqrt() * PI * 2.0).sin(), -0.2 * s))
        * Mat4::from_translation(Vec3::new(0.56, -0.52, -0.72))
        * Mat4::from_rotation_y(deg(45.0 - swing_sin * 20.0))
        * Mat4::from_rotation_z(deg(-sr * 20.0))
        * Mat4::from_rotation_x(deg(-sr * 80.0))
        * Mat4::from_rotation_y(deg(-45.0));
    let layer = |name: u32| textures.block_layers.get(name as usize).map_or(0, |l| l.0);
    match (def.block.map(|st| &c.states[st as usize]), sprite) {
        (Some(st), _) if def.icon == u32::MAX && st.item_sprite().is_none() => {
            // block.json: rotated 45 degrees, scale 0.4.
            let m = arm * Mat4::from_rotation_y(deg(45.0)) * Mat4::from_scale(Vec3::splat(0.4)) * Mat4::from_translation(Vec3::splat(-0.5));
            block_shape(out, textures, m, st, Vec4::from_array(color));
        }
        (_, Some(sp)) => {
            // generated.json / handheld.json first person: rotation 0 -90 25, translation
            // 1.13 3.2 1.13 (pixels), scale 0.68; the sprite is one pixel thick.
            let (index, l) = if def.icon != u32::MAX {
                (textures.item_index, textures.item_layers.get(def.icon as usize).map_or(0, |x| x.0))
            } else {
                (textures.block_index, layer(def.block.map_or(0, |st| c.states[st as usize].textures[0])))
            };
            let model = arm
                * Mat4::from_translation(Vec3::new(1.13, 3.2, 1.13) / 16.0)
                * Mat4::from_rotation_y(deg(-90.0))
                * Mat4::from_rotation_z(deg(25.0))
                * Mat4::from_scale(Vec3::splat(0.68))
                * Mat4::from_translation(Vec3::splat(-0.5));
            let n = sp.size as f32;
            for &(x, y) in &sp.pixels {
                let m = model * Mat4::from_translation(Vec3::new(x as f32 / n, 1.0 - (y as f32 + 1.0) / n, 7.5 / 16.0)) * Mat4::from_scale(Vec3::new(1.0 / n, 1.0 / n, 1.0 / 16.0));
                // Mode 4: every face takes the colour of this one texel.
                out.push(GpuBox {
                    rows: rows(m),
                    uvo: [(x as f32 + 0.5) / n, (y as f32 + 0.5) / n, 0.0, 0.0],
                    color,
                    ids: [index, 4, l, 0],
                    ..Default::default()
                });
            }
        }
        _ => {}
    }
}

/// A block item drawn as its model's boxes; `m` maps the block's unit cube.
fn block_shape(out: &mut Vec<GpuBox>, textures: &GameTextures, m: Mat4, s: &crate::content::StateInfo, color: Vec4) {
    let layer = |name: u32| textures.block_layers.get(name as usize).map_or(0, |l| l.0);
    for b in s.item_shape() {
        let (mn, mx) = (Vec3::from_array(b.min), Vec3::from_array(b.max));
        let mb = m * Mat4::from_translation(mn) * Mat4::from_scale(mx - mn);
        let l = b.textures.map(layer);
        out.push(GpuBox {
            rows: rows(mb),
            dims: mn.extend(1.0).to_array(),
            uvo: mx.extend(0.0).to_array(),
            color: color.to_array(),
            ids: [textures.block_index, 1, l[0] | l[1] << 16, l[2] | l[3] << 16],
            ids2: [l[4] | l[5] << 16, 0, 0, 0],
        });
        tinted_faces(out, textures, mb, s, &b.textures, color);
    }
}

fn rows(m: Mat4) -> [[f32; 4]; 3] {
    [m.row(0).to_array(), m.row(1).to_array(), m.row(2).to_array()]
}

/// Faces of a block item drawn again on top, tinted: faces the block tints (grass tops) and the
/// overlays of its textures (grass sides). `m` maps the unit cube.
fn tinted_faces(out: &mut Vec<GpuBox>, textures: &GameTextures, m: Mat4, s: &crate::content::StateInfo, faces: &[u32; 6], color: Vec4) {
    if s.tint == 0 {
        return;
    }
    let tint = match s.tint {
        1 => 0x8db360u32,
        2 => 0x77ab2f,
        _ => 0x3f76e4,
    };
    let rgb = Vec3::new(((tint >> 16) & 255) as f32, ((tint >> 8) & 255) as f32, (tint & 255) as f32) / 255.0;
    let layer = |name: u32| textures.block_layers.get(name as usize).map_or(0, |l| l.0);
    for d in 0..6 {
        let tex = if textures.overlays.get(faces[d] as usize).is_some_and(|&o| o != 0) {
            textures.overlays[faces[d] as usize] - 1
        } else if s.tint_faces & (1 << d) != 0 {
            faces[d]
        } else {
            continue;
        };
        let l = layer(tex);
        // A zero-thick box just outside that face.
        let axis = d / 2;
        let mut offset = Vec3::ZERO;
        let mut scale = Vec3::ONE;
        offset[axis] = if d % 2 == 1 { 1.002 } else { -0.002 };
        scale[axis] = 0.0;
        let f = m * Mat4::from_translation(offset) * Mat4::from_scale(scale);
        out.push(GpuBox {
            rows: rows(f),
            color: (color.truncate() * rgb).extend(color.w).to_array(),
            ids: [textures.block_index, 1, l | l << 16, l | l << 16],
            ids2: [l | l << 16, 0, 0, 0],
            ..Default::default()
        });
    }
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
        if let Some(t) = s.item_sprite() {
            let l = layer(t);
            let m = spin * Mat4::from_translation(Vec3::new(-0.2, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(0.4, 0.4, 0.0));
            out.push(GpuBox { rows: rows(m), color: color.to_array(), ids: [textures.block_index, 1, l | l << 16, l | l << 16], ids2: [l | l << 16, 0, 0, 0], ..Default::default() });
            return;
        }
        let m = spin * Mat4::from_translation(Vec3::splat(-0.125)) * Mat4::from_scale(Vec3::splat(0.25));
        block_shape(out, textures, m, s, color);
    } else {
        // A flat sprite: zero thickness, so the side faces (which would smear the edge texels
        // into lines) have no area.
        let l = textures.item_layers.get(def.icon as usize).map_or(0, |x| x.0);
        let m = spin * Mat4::from_translation(Vec3::new(-0.2, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(0.4, 0.4, 0.0));
        out.push(GpuBox { rows: rows(m), color: color.to_array(), ids: [textures.item_index, 1, l | l << 16, l | l << 16], ids2: [l | l << 16, 0, 0, 0], ..Default::default() });
    }
}
