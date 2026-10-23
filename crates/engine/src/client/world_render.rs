//! Draws meshed cubes. All face records live in a few big storage-buffer pages; each frame the CPU
//! culls cubes and writes one indirect draw per (cube, layer) plus the cube origin it uses.

use glam::{DVec3, IVec3, Mat4, Vec3, Vec4};
use rustc_hash::FxHashMap;
use voxide_render::*;

use super::mesher::Mesh;
use super::textures::GameTextures;
use crate::content::Content;

const FACE_WORDS: u32 = 4;
const QUAD_WORDS: u32 = 8;
const PAGE_BYTES: u64 = 64 << 20;
/// A cube holds at most 6 * 4096 faces; plants and boxes add a few quads per block.
const MAX_QUADS_PER_DRAW: u32 = 6 * 4096 * 2;

#[derive(Clone, Copy)]
struct Alloc {
    page: u32,
    offset: u32,
    len: u32,
}

struct Pool {
    words: u32,
    pages: Vec<(BufferId, Vec<(u32, u32)>)>,
    label: &'static str,
}

impl Pool {
    fn records_per_page(&self) -> u32 {
        (PAGE_BYTES / (self.words as u64 * 4)) as u32
    }

    fn alloc(&mut self, gpu: &mut dyn Backend, n: u32) -> Alloc {
        let per = self.records_per_page();
        assert!(n <= per, "mesh larger than a page");
        for (pi, (_, free)) in self.pages.iter_mut().enumerate() {
            if let Some(i) = free.iter().position(|&(_, len)| len >= n) {
                let (start, len) = free[i];
                if len == n {
                    free.remove(i);
                } else {
                    free[i] = (start + n, len - n);
                }
                return Alloc { page: pi as u32, offset: start, len: n };
            }
        }
        let buf = gpu.create_buffer(PAGE_BYTES, usage::STORAGE, self.label);
        self.pages.push((buf, vec![(n, per - n)]));
        Alloc { page: self.pages.len() as u32 - 1, offset: 0, len: n }
    }

    fn free(&mut self, a: Alloc) {
        let free = &mut self.pages[a.page as usize].1;
        let i = free.partition_point(|&(s, _)| s < a.offset);
        free.insert(i, (a.offset, a.len));
        if i + 1 < free.len() && free[i].0 + free[i].1 == free[i + 1].0 {
            free[i].1 += free[i + 1].1;
            free.remove(i + 1);
        }
        if i > 0 && free[i - 1].0 + free[i - 1].1 == free[i].0 {
            free[i - 1].1 += free[i].1;
            free.remove(i);
        }
    }
}

struct GpuMesh {
    faces: Option<Alloc>,
    /// Per layer, per direction rank (see `mesher::DIR_RANK`).
    face_counts: [[u32; 6]; 3],
    quads: Option<Alloc>,
    quad_counts: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub view_proj: [[f32; 4]; 4],
    pub cam_frac: [f32; 4],
    pub sky_color: [f32; 4],
    pub fog_color: [f32; 4],
    pub params: [f32; 4],
    pub ids: [u32; 4],
    pub look: [f32; 4],
}

pub struct Camera {
    pub pos: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}

impl Camera {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_to_mat4(Vec3::ZERO, self.forward(), Vec3::Y)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        // Y-up clip space: naga already flips Y for Vulkan.
        glam::camera::rh::proj::directx::perspective_infinite_reverse(self.fov.to_radians(), aspect, 0.05)
    }
}

pub struct FrameParams {
    pub daylight: f32,
    pub sky_color: Vec3,
    pub fog_color: Vec3,
    pub fog_start: f32,
    pub fog_end: f32,
    pub time: f32,
    pub ambient: u32,
    pub brightness: f32,
}

pub struct WorldRenderer {
    face_pipes: [PipelineId; 3],
    quad_pipes: [PipelineId; 3],
    faces: Pool,
    quads: Pool,
    meshes: FxHashMap<IVec3, GpuMesh>,
    globals: Vec<BufferId>,
    origins: Vec<(BufferId, u32)>,
    indirect: Vec<(BufferId, u32)>,
    biomes: BufferId,
    tex_info: BufferId,
    anim: BufferId,
    /// (0 1 2)(0 2 3) for as many quads as one draw can hold; every face and quad draw shares it
    /// so each corner is shaded once instead of once per triangle.
    quad_index: BufferId,
    block_index: u32,
    pub drawn: u32,
    pub faces_drawn: u64,
}

fn load_shader(name: &str, builtin: &str) -> String {
    // Development builds read shaders from the source tree so they can be edited live.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders").join(name);
    std::fs::read_to_string(path).unwrap_or_else(|_| builtin.to_string())
}

pub fn world_shader() -> Result<Shader, ShaderError> {
    Shader::from_wgsl("world.wgsl", &load_shader("world.wgsl", include_str!("../../shaders/world.wgsl")))
}

impl WorldRenderer {
    pub fn new(gpu: &mut dyn Backend, c: &Content, textures: &GameTextures) -> Result<WorldRenderer, String> {
        let shader = world_shader().map_err(|e| e.0)?;
        let (face_pipes, quad_pipes) = Self::pipelines(gpu, &shader)?;

        // Two words per texture name: static -> (layer, 0); animated -> (offset into the frame
        // list, count | total ticks << 16 | interpolate << 31).
        let mut info: Vec<u32> = Vec::with_capacity(textures.block_layers.len() * 2);
        let mut frames: Vec<u32> = vec![0];
        for (i, &(first, _)) in textures.block_layers.iter().enumerate() {
            match textures.block_anims.get(i).cloned().flatten() {
                Some(a) if !a.frames.is_empty() => {
                    let total: u32 = a.frames.iter().map(|f| f.1).sum::<u32>().min(0x7FFF);
                    info.push(frames.len() as u32);
                    info.push(a.frames.len() as u32 | total << 16 | (a.interpolate as u32) << 31);
                    frames.extend(a.frames.iter().map(|&(layer, ticks)| layer | ticks.min(0xFFFF) << 16));
                }
                _ => {
                    info.push(first);
                    info.push(0);
                }
            }
        }
        let tex_info = gpu.create_buffer((info.len() * 4) as u64, usage::STORAGE, "texture info");
        gpu.write_buffer(tex_info, 0, bytemuck::cast_slice(&info));
        let anim = gpu.create_buffer((frames.len() * 4) as u64, usage::STORAGE, "texture frames");
        gpu.write_buffer(anim, 0, bytemuck::cast_slice(&frames));

        let biome_data: Vec<[u32; 4]> = if c.biomes.is_empty() {
            vec![[0x7fb238, 0x48b518, 0x3f76e4, 0]]
        } else {
            c.biomes.iter().map(|b| [b.grass, b.foliage, b.water, 0]).collect()
        };
        let biomes = gpu.create_buffer((biome_data.len() * 16) as u64, usage::STORAGE, "biomes");
        gpu.write_buffer(biomes, 0, bytemuck::cast_slice(&biome_data));

        let indices: Vec<u32> = (0..MAX_QUADS_PER_DRAW).flat_map(|q| [0, 1, 2, 0, 2, 3].map(|i| q * 4 + i)).collect();
        let quad_index = gpu.create_buffer((indices.len() * 4) as u64, usage::INDEX, "quad indices");
        gpu.write_buffer(quad_index, 0, bytemuck::cast_slice(&indices));

        let frames = gpu.frames_in_flight();
        let globals = (0..frames).map(|_| gpu.create_buffer(size_of::<Globals>() as u64, usage::STORAGE | usage::HOST, "globals")).collect();
        Ok(WorldRenderer {
            face_pipes,
            quad_pipes,
            faces: Pool { words: FACE_WORDS, pages: Vec::new(), label: "face page" },
            quads: Pool { words: QUAD_WORDS, pages: Vec::new(), label: "quad page" },
            meshes: FxHashMap::default(),
            globals,
            origins: Vec::new(),
            indirect: Vec::new(),
            biomes,
            tex_info,
            anim,
            quad_index,
            block_index: textures.block_index,
            drawn: 0,
            faces_drawn: 0,
        })
    }

    fn pipelines(gpu: &mut dyn Backend, shader: &Shader) -> Result<([PipelineId; 3], [PipelineId; 3]), String> {
        let depth_write = Some(DepthState { write: true, compare: Compare::GreaterEqual });
        let depth_read = Some(DepthState { write: false, compare: Compare::GreaterEqual });
        let mut make = |label: &str, vs: &str, fs: &str, depth, blend, cull| {
            gpu.create_pipeline(&PipelineDesc {
                label,
                shader,
                vertex: vs,
                fragment: Some(fs),
                color: Some(Format::Surface),
                depth,
                blend,
                cull_back: cull,
                topology: Topology::Triangles,
            })
            .map_err(|e| e.0)
        };
        let faces = [
            make("faces opaque", "vs_face", "fs_opaque", depth_write, Blend::None, true)?,
            make("faces cutout", "vs_face", "fs_cutout", depth_write, Blend::None, true)?,
            make("faces translucent", "vs_face", "fs_translucent", depth_read, Blend::Alpha, true)?,
        ];
        let quads = [
            make("quads opaque", "vs_quad", "fs_opaque", depth_write, Blend::None, true)?,
            make("quads cutout", "vs_quad", "fs_cutout", depth_write, Blend::None, true)?,
            make("quads translucent", "vs_quad", "fs_translucent", depth_read, Blend::Alpha, true)?,
        ];
        Ok((faces, quads))
    }

    /// Recompiles the world shader; keeps the old pipelines when the new source has errors.
    pub fn reload_shaders(&mut self, gpu: &mut dyn Backend) {
        match world_shader().map_err(|e| e.0).and_then(|s| Self::pipelines(gpu, &s)) {
            Ok((f, q)) => {
                for p in self.face_pipes.into_iter().chain(self.quad_pipes) {
                    gpu.destroy_pipeline(p);
                }
                self.face_pipes = f;
                self.quad_pipes = q;
                log::info!("world shader reloaded");
            }
            Err(e) => log::error!("world shader: {e}"),
        }
    }

    pub fn remove(&mut self, pos: IVec3) {
        if let Some(m) = self.meshes.remove(&pos) {
            if let Some(a) = m.faces {
                self.faces.free(a);
            }
            if let Some(a) = m.quads {
                self.quads.free(a);
            }
        }
    }

    pub fn destroy(&mut self, gpu: &mut dyn Backend) {
        self.meshes.clear();
        for (b, _) in self.faces.pages.drain(..).chain(self.quads.pages.drain(..)) {
            gpu.destroy_buffer(b);
        }
        for b in self.globals.drain(..).chain([self.biomes, self.tex_info, self.anim, self.quad_index]) {
            gpu.destroy_buffer(b);
        }
        for (b, _) in self.origins.drain(..).chain(self.indirect.drain(..)) {
            gpu.destroy_buffer(b);
        }
        for p in self.face_pipes.into_iter().chain(self.quad_pipes) {
            gpu.destroy_pipeline(p);
        }
    }

    pub fn clear(&mut self) {
        let keys: Vec<IVec3> = self.meshes.keys().copied().collect();
        for k in keys {
            self.remove(k);
        }
    }

    pub fn upload(&mut self, gpu: &mut dyn Backend, pos: IVec3, mesh: &Mesh) {
        self.remove(pos);
        if mesh.is_empty() {
            return;
        }
        let face_counts = mesh.faces.each_ref().map(|v| {
            let mut n = [0u32; 6];
            for f in v {
                n[super::mesher::DIR_RANK[((f[0] >> 12) & 7) as usize] as usize] += 1;
            }
            n
        });
        let quad_counts = mesh.quads.each_ref().map(|v| v.len() as u32);
        let total_f: u32 = face_counts.iter().flatten().sum();
        let total_q: u32 = quad_counts.iter().sum();
        let faces = (total_f > 0).then(|| {
            let a = self.faces.alloc(gpu, total_f);
            let data: Vec<[u32; 4]> = mesh.faces.concat();
            gpu.write_buffer(self.faces.pages[a.page as usize].0, a.offset as u64 * 16, bytemuck::cast_slice(&data));
            a
        });
        let quads = (total_q > 0).then(|| {
            let a = self.quads.alloc(gpu, total_q);
            let data: Vec<[u32; 8]> = mesh.quads.concat();
            gpu.write_buffer(self.quads.pages[a.page as usize].0, a.offset as u64 * 32, bytemuck::cast_slice(&data));
            a
        });
        self.meshes.insert(pos, GpuMesh { faces, face_counts, quads, quad_counts });
    }

    fn ensure_host(gpu: &mut dyn Backend, slot: &mut Vec<(BufferId, u32)>, frame: usize, frames: usize, needed: u32, stride: u32, usage_bits: u32, label: &str) -> BufferId {
        while slot.len() < frames {
            slot.push((gpu.create_buffer(stride as u64 * 1024, usage_bits | usage::HOST, label), 1024));
        }
        if slot[frame].1 < needed {
            gpu.destroy_buffer(slot[frame].0);
            let cap = needed.next_power_of_two();
            slot[frame] = (gpu.create_buffer(stride as u64 * cap as u64, usage_bits | usage::HOST, label), cap);
        }
        slot[frame].0
    }

    pub fn draw(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, frame: &FrameInfo, cam: &Camera, p: &FrameParams) {
        let aspect = frame.width as f32 / frame.height.max(1) as f32;
        let view_proj = cam.proj(aspect) * cam.view();
        let cam_block = cam.pos.floor();
        let cam_cube: IVec3 = cam_block.as_ivec3() >> 4i32;
        let cam_frac = (cam.pos - (cam_cube * 16).as_dvec3()).as_vec3();
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            cam_frac: [cam_frac.x, cam_frac.y, cam_frac.z, 0.0],
            sky_color: p.sky_color.extend(1.0).to_array(),
            fog_color: p.fog_color.extend(1.0).to_array(),
            params: [p.daylight, p.fog_start, p.fog_end, p.time],
            ids: [self.block_index, p.ambient, 0, 0],
            look: [p.brightness, 0.0, 0.0, 0.0],
        };
        let gbuf = self.globals[frame.slot];
        gpu.mapped(gbuf)[..size_of::<Globals>()].copy_from_slice(bytemuck::bytes_of(&globals));

        let planes = frustum_planes(view_proj);
        let mut visible: Vec<(IVec3, f32)> = Vec::new();
        for (&pos, _) in &self.meshes {
            let rel = (pos - cam_cube).as_vec3() * 16.0 - cam_frac;
            let center = rel + Vec3::splat(8.0);
            if planes.iter().all(|pl| pl.dot(center.extend(1.0)) > -13.86) {
                visible.push((pos, center.length_squared()));
            }
        }
        visible.sort_by(|a, b| a.1.total_cmp(&b.1));

        // Draw records: (kind 0 faces / 1 quads, layer, page, args, origin)
        let mut draws: Vec<(u8, u8, u32, DrawIndexedArgs, [i32; 4])> = Vec::new();
        let mut faces_drawn = 0u64;
        for &(pos, _) in &visible {
            let m = &self.meshes[&pos];
            let rel = pos - cam_cube;
            let origin = [rel.x, rel.y, rel.z, 0];
            if let Some(a) = m.faces {
                // Directions by rank: +X +Y +Z -X -Y -Z. A face direction can be seen only from
                // the side its normal points to; contiguous visible runs become one draw.
                let lo = (pos - cam_cube).as_vec3() * 16.0 - cam_frac;
                let seen = [lo.x < 0.0, lo.y < 0.0, lo.z < 0.0, lo.x > -16.0, lo.y > -16.0, lo.z > -16.0];
                let mut off = a.offset;
                for layer in 0..3 {
                    let counts = m.face_counts[layer];
                    let mut run: Option<(u32, u32)> = None;
                    for rank in 0..6 {
                        let n = counts[rank];
                        if n > 0 && seen[rank] {
                            faces_drawn += n as u64;
                            run = Some(match run {
                                Some((s, len)) => (s, len + n),
                                None => (off, n),
                            });
                        } else if n > 0 {
                            if let Some((s, len)) = run.take() {
                                draws.push((0, layer as u8, a.page, DrawIndexedArgs { indices: len * 6, instances: 1, first_index: 0, vertex_offset: (s * 4) as i32, first_instance: 0 }, origin));
                            }
                        }
                        off += n;
                    }
                    if let Some((s, len)) = run {
                        draws.push((0, layer as u8, a.page, DrawIndexedArgs { indices: len * 6, instances: 1, first_index: 0, vertex_offset: (s * 4) as i32, first_instance: 0 }, origin));
                    }
                }
            }
            if let Some(a) = m.quads {
                let mut off = a.offset;
                for layer in 0..3 {
                    let n = m.quad_counts[layer];
                    if n > 0 {
                        draws.push((1, layer as u8, a.page, DrawIndexedArgs { indices: n * 6, instances: 1, first_index: 0, vertex_offset: (off * 4) as i32, first_instance: 0 }, origin));
                    }
                    off += n;
                }
            }
        }
        // Opaque/cutout front to back (early depth rejection), translucent back to front.
        let order = |d: &(u8, u8, u32, DrawIndexedArgs, [i32; 4])| (d.1 == 2, d.1, d.0, d.2);
        let mut sorted: Vec<usize> = (0..draws.len()).collect();
        sorted.sort_by_key(|&i| order(&draws[i]));
        let translucent_start = sorted.iter().position(|&i| draws[i].1 == 2).unwrap_or(sorted.len());
        sorted[translucent_start..].reverse();
        // Reversing flips kind/page grouping too; regroup translucent draws by (kind, page) while
        // keeping their distance order within each group.
        sorted[translucent_start..].sort_by_key(|&i| (draws[i].0, draws[i].2));

        let n = draws.len() as u32;
        self.drawn = n;
        self.faces_drawn = faces_drawn;
        if n == 0 {
            return;
        }
        let frames = gpu.frames_in_flight();
        let obuf = Self::ensure_host(gpu, &mut self.origins, frame.slot, frames, n, 16, usage::STORAGE, "origins");
        let ibuf = Self::ensure_host(gpu, &mut self.indirect, frame.slot, frames, n, 20, usage::INDIRECT, "indirect");
        {
            let mut args: Vec<DrawIndexedArgs> = Vec::with_capacity(n as usize);
            let mut origins: Vec<[i32; 4]> = Vec::with_capacity(n as usize);
            for (k, &i) in sorted.iter().enumerate() {
                let mut a = draws[i].3;
                a.first_instance = k as u32;
                args.push(a);
                origins.push(draws[i].4);
            }
            gpu.mapped(obuf)[..origins.len() * 16].copy_from_slice(bytemuck::cast_slice(&origins));
            gpu.mapped(ibuf)[..args.len() * 20].copy_from_slice(bytemuck::cast_slice(&args));
        }

        list.push(Cmd::IndexBuffer(self.quad_index));
        let mut k = 0usize;
        while k < sorted.len() {
            let (kind, layer, page) = (draws[sorted[k]].0, draws[sorted[k]].1, draws[sorted[k]].2);
            let mut end = k + 1;
            while end < sorted.len() {
                let d = &draws[sorted[end]];
                if (d.0, d.1, d.2) != (kind, layer, page) {
                    break;
                }
                end += 1;
            }
            let (pipe, buf) = if kind == 0 {
                (self.face_pipes[layer as usize], self.faces.pages[page as usize].0)
            } else {
                (self.quad_pipes[layer as usize], self.quads.pages[page as usize].0)
            };
            list.pipeline(pipe);
            list.bind(0, gbuf);
            list.bind(1, obuf);
            list.bind(2, buf);
            list.bind(3, self.biomes);
            list.bind(4, self.tex_info);
            list.bind(5, self.anim);
            list.push(Cmd::DrawIndexedIndirect { buffer: ibuf, offset: k as u64 * 20, count: (end - k) as u32 });
            k = end;
        }
    }
}

/// Left, right, bottom, top planes; the reverse-Z infinite projection has no useful far plane.
fn frustum_planes(m: Mat4) -> [Vec4; 4] {
    let r = |i: usize| m.row(i);
    let norm = |p: Vec4| p / p.truncate().length();
    [norm(r(3) + r(0)), norm(r(3) - r(0)), norm(r(3) + r(1)), norm(r(3) - r(1))]
}
