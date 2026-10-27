//! Particles: kinds are data (`registry::ParticleDef`), particles live only on clients. They are
//! simulated in bulk every frame (in parallel when there are many) and drawn as instanced
//! camera-facing quads: one draw for the cut-out ones, one for the blended ones.

use glam::{DVec3, IVec3, Vec3};
use rayon::prelude::*;
use weft_abi::{ParticleBurst, particle_flags as pf};
use weft_render::*;

use super::textures::GameTextures;
use super::world::ClientWorld;
use super::world_render::Camera;
use crate::content::content;
use crate::shaders::{Hook, Slots};
use crate::gpu_layout::{GpuParticle, ParticleView as View};

/// Particles a client keeps at most; bursts beyond it are dropped.
const MAX: usize = 1 << 18;
/// Below this many particles one thread simulates them all.
const PARALLEL_FROM: usize = 4096;

pub const HOOK: Hook = Hook {
    kind: "particle",
    entry: "shade",
    dispatch: "particle_shade",
    params: "p: ParticleFrag",
    args: "p",
    ret: "vec4<f32>",
    fallback: "return p.albedo * p.color;",
};

#[derive(Clone, Copy)]
struct Particle {
    pos: DVec3,
    vel: Vec3,
    age: f32,
    life: f32,
    size: f32,
    kind: u16,
    on_ground: bool,
    /// Block dust: the block texture layer and the corner of its quarter, in sixteenths.
    layer: u32,
    piece: [u8; 2],
    cell: IVec3,
    light: u32,
}

pub struct Particles {
    list: Vec<Particle>,
    rng: u64,
    pipes: Option<[PipelineId; 2]>,
    /// Shader slot of each kind.
    kind_slots: Vec<u32>,
    instances: Vec<(BufferId, u32)>,
    views: Vec<BufferId>,
    gpu: Vec<GpuParticle>,
}

/// Per frame values the shader needs besides the camera.
pub struct Look {
    pub fog_color: [f32; 4],
    pub fog: (f32, f32),
    pub daylight: f32,
    pub ambient: f32,
    pub brightness: f32,
}

fn shader(assets: &crate::assets::Assets) -> Result<(Shader, Slots), String> {
    let snippets = crate::shaders::snippets(assets, &HOOK);
    let (s, mut slots) = crate::packs::compose("particle.wgsl", include_str!("../../shaders/particle.wgsl"), &[(&HOOK, &snippets)]).map_err(|e| e.0)?;
    Ok((s, slots.pop().unwrap_or_default()))
}

impl Particles {
    pub fn new(gpu: &mut dyn Backend, assets: &crate::assets::Assets) -> Particles {
        let mut p = Particles { list: Vec::new(), rng: 0x9E37_79B9_7F4A_7C15, pipes: None, kind_slots: Vec::new(), instances: Vec::new(), views: Vec::new(), gpu: Vec::new() };
        p.reload(gpu, assets);
        p
    }

    /// Recompiles the shader with the particle snippets mods and packs have now.
    pub fn reload(&mut self, gpu: &mut dyn Backend, assets: &crate::assets::Assets) {
        let made = shader(assets).and_then(|(s, slots)| {
            let mut make = |label: &str, fs: &str, depth: DepthState, blend: Blend| {
                gpu.create_pipeline(&PipelineDesc { label, shader: &s, vertex: "vs", fragment: Some(fs), color: Some(Format::Surface), depth: Some(depth), blend, cull_back: false, topology: Topology::Triangles })
                    .map_err(|e| e.0)
            };
            let cut = make("particles cutout", "fs_cutout", DepthState { write: true, compare: Compare::GreaterEqual }, Blend::None)?;
            let blend = make("particles blended", "fs_blend", DepthState { write: false, compare: Compare::GreaterEqual }, Blend::Alpha)?;
            Ok(([cut, blend], slots))
        });
        match made {
            Ok((pipes, slots)) => {
                if let Some(old) = self.pipes.replace(pipes) {
                    old.into_iter().for_each(|p| gpu.destroy_pipeline(p));
                }
                self.kind_slots = content().game.particles.iter().map(|k| slots.slot(&k.shader)).collect();
                for k in content().game.particles.iter().filter(|k| !k.shader.is_empty() && slots.slot(&k.shader) == 0) {
                    log::warn!("particle {}: no shader {}", k.name, k.shader);
                }
            }
            Err(e) => log::error!("particle shader: {e}"),
        }
    }

    pub fn destroy(&mut self, gpu: &mut dyn Backend) {
        for p in self.pipes.take().into_iter().flatten() {
            gpu.destroy_pipeline(p);
        }
        for b in self.instances.drain(..).map(|b| b.0).chain(self.views.drain(..)) {
            gpu.destroy_buffer(b);
        }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn spawn(&mut self, b: &ParticleBurst) {
        let c = content();
        let Some(k) = c.game.particles.get(b.kind as usize) else { return };
        let pos = DVec3::from_array(b.pos);
        if !pos.is_finite() {
            return;
        }
        // Block dust shows the block's side texture (a box model's first box's).
        let side = |s: &crate::content::StateInfo| match s.boxes.first() {
            Some(b) if s.model == weft_abi::Model::Boxes => b.textures[1],
            _ => s.textures[1],
        };
        let layer = if k.frames.is_empty() { c.states.get(b.block as usize).map_or(0, side) } else { 0 };
        let n = (b.count as usize).min(MAX.saturating_sub(self.list.len()));
        for _ in 0..n {
            let mut r3 = || Vec3::new(self.random() * 2.0 - 1.0, self.random() * 2.0 - 1.0, self.random() * 2.0 - 1.0);
            let off = r3() * Vec3::from_array(b.spread);
            let mut dir = r3();
            while dir.length_squared() > 1.0 {
                dir = r3();
            }
            let vel = Vec3::from_array(b.velocity) + dir * b.speed;
            let life = k.life[0] + (k.life[1] - k.life[0]) * self.random();
            let size = k.scale[0] + (k.scale[1] - k.scale[0]) * self.random();
            let piece = [(self.random() * 12.0) as u8, (self.random() * 12.0) as u8];
            self.list.push(Particle {
                pos: pos + off.as_dvec3(),
                vel,
                age: 0.0,
                life,
                size,
                kind: b.kind as u16,
                on_ground: false,
                layer,
                piece,
                cell: IVec3::MAX,
                light: 0,
            });
        }
    }

    /// Moves every particle by `dt` seconds; colliding kinds stop on blocks like vanilla's.
    pub fn update(&mut self, dt: f32, world: &ClientWorld) {
        if self.list.is_empty() {
            return;
        }
        let c = content();
        let kinds = &c.game.particles;
        // Speed kept this frame per kind (`drag` is per tick).
        let keep: Vec<f32> = kinds.iter().map(|k| k.drag.powf(dt * 20.0)).collect();
        let ground = 0.7f32.powf(dt * 20.0);
        let block = |p: IVec3| world.block_or_air(p);
        let step = |p: &mut Particle| {
            p.age += dt;
            let k = &kinds[p.kind as usize];
            p.vel.y -= k.gravity * dt;
            p.vel *= keep[p.kind as usize];
            let delta = (p.vel * dt).as_dvec3();
            if k.flags & pf::COLLIDE != 0 {
                // Vanilla's particles collide as a 0.2 block box around their center.
                let r = crate::physics::move_box(c, &block, p.pos - DVec3::Y * 0.1, 0.1, 0.2, delta, 0.0);
                p.pos = r.pos + DVec3::Y * 0.1;
                for i in 0..3 {
                    if r.hit[i] {
                        p.vel[i] = 0.0;
                    }
                }
                p.on_ground = r.on_ground || (p.on_ground && r.hit[1]);
                if p.on_ground {
                    p.vel.x *= ground;
                    p.vel.z *= ground;
                }
            } else {
                p.pos += delta;
            }
            let cell = p.pos.floor().as_ivec3();
            if cell != p.cell {
                p.cell = cell;
                p.light = world.light(cell);
            }
        };
        if self.list.len() >= PARALLEL_FROM {
            self.list.par_iter_mut().with_min_len(1024).for_each(step);
        } else {
            self.list.iter_mut().for_each(step);
        }
        self.list.retain(|p| p.age < p.life);
    }

    /// Cutout particles into `list`, blended ones into `late` (after translucent blocks).
    pub fn draw(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, late: &mut CommandList, frame: &FrameInfo, cam: &Camera, textures: &GameTextures, look: &Look) {
        let Some(pipes) = self.pipes else { return };
        if self.list.is_empty() {
            return;
        }
        let c = content();
        let mix = |a: u32, b: u32, t: f32| -> u32 {
            let ch = |s: u32| {
                let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
                ((x + (y - x) * t).round() as u32) << s
            };
            ch(24) | ch(16) | ch(8) | ch(0)
        };
        self.gpu.clear();
        let mut blended_from = 0;
        for pass in [false, true] {
            if pass {
                blended_from = self.gpu.len();
            }
            for p in &self.list {
                let k = &c.game.particles[p.kind as usize];
                if (k.flags & pf::TRANSLUCENT != 0) != pass {
                    continue;
                }
                let t = (p.age / p.life).clamp(0.0, 1.0);
                let size = if k.flags & pf::SHRINK != 0 { p.size * (1.0 - t) } else { p.size };
                let (tex, uv) = if k.frames.is_empty() {
                    let (u, v) = (p.piece[0] as u32 * 255 / 16, p.piece[1] as u32 * 255 / 16);
                    (textures.texture(p.layer), u | v << 8 | (u + 64).min(255) << 16 | (v + 64).min(255) << 24)
                } else {
                    let f = k.frames[((t * k.frames.len() as f32) as usize).min(k.frames.len() - 1)];
                    (textures.texture(f), 0xFFFF_0000)
                };
                let light = if k.flags & pf::FULLBRIGHT != 0 { u32::MAX } else { p.light };
                let rel = (p.pos - cam.pos).as_vec3();
                self.gpu.push(GpuParticle {
                    pos_size: [rel.x, rel.y, rel.z, size],
                    data: [mix(k.color, k.color_end, t), tex & crate::gpu_layout::particle::TEX_MASK | self.kind_slots.get(p.kind as usize).copied().unwrap_or(0) << crate::gpu_layout::particle::SLOT_SHIFT, uv, light],
                    extra: [t, 0.0, 0.0, 0.0],
                });
            }
        }
        let n = self.gpu.len() as u32;
        let frames = gpu.frames_in_flight();
        let stride = size_of::<GpuParticle>() as u64;
        while self.views.len() < frames {
            self.views.push(gpu.create_buffer(size_of::<View>() as u64, usage::STORAGE | usage::HOST, "particle view"));
        }
        let buf = crate::gpu_layout::frame_buffer(gpu, &mut self.instances, frame.slot, n, stride, usage::STORAGE, "particles");
        gpu.mapped(buf)[..self.gpu.len() * stride as usize].copy_from_slice(bytemuck::cast_slice(&self.gpu));
        let aspect = frame.width as f32 / frame.height.max(1) as f32;
        let view = cam.view();
        let inv = view.inverse();
        let v = View {
            view_proj: (cam.proj(aspect) * view).to_cols_array_2d(),
            right: inv.transform_vector3(Vec3::X).extend(0.0).to_array(),
            up: inv.transform_vector3(Vec3::Y).extend(0.0).to_array(),
            fog_color: look.fog_color,
            params: [look.fog.0, look.fog.1, look.daylight, crate::shaders::time()],
            look: [look.ambient, look.brightness, 0.0, 0.0],
        };
        let vb = self.views[frame.slot];
        gpu.mapped(vb)[..size_of::<View>()].copy_from_slice(bytemuck::bytes_of(&v));
        for (pipe, first, count, list) in [(pipes[0], 0, blended_from as u32, &mut *list), (pipes[1], blended_from as u32, n - blended_from as u32, &mut *late)] {
            if count == 0 {
                continue;
            }
            list.pipeline(pipe);
            list.bind(0, buf);
            list.bind(1, vb);
            list.push(Cmd::Draw { vertices: 6, instances: count, first_vertex: 0, first_instance: first });
        }
    }
}
