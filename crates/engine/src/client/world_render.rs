//! Draws meshed cubes. All face records live in a few big storage-buffer pages; each frame the CPU
//! culls cubes and writes one indirect draw per (cube, layer) plus the cube origin it uses.
//! Cubes of moving structures are meshed in the structure's own coordinates and drawn moved
//! by its pose (an offset per structure, `origin.w` in the shader): a block placed on one
//! remeshes only its cube.

use glam::{DVec3, IVec3, Mat4, Vec3, Vec4};
use rustc_hash::FxHashMap;
use weft_render::*;

use super::mesher::Mesh;
use super::textures::GameTextures;
use crate::content::Content;
pub use crate::gpu_layout::Globals;

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

#[derive(Clone, Copy)]
pub struct Camera {
    pub pos: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    /// Walking sway in view space: offset, roll and extra pitch (vanilla's view bobbing).
    pub bob: (Vec3, f32, f32),
}

impl Camera {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    pub fn view(&self) -> Mat4 {
        let (offset, roll, pitch) = self.bob;
        Mat4::from_translation(offset) * Mat4::from_rotation_z(roll) * Mat4::from_rotation_x(pitch) * glam::camera::rh::view::look_to_mat4(Vec3::ZERO, self.forward(), Vec3::Y)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        // Y-up clip space: naga already flips Y for Vulkan.
        glam::camera::rh::proj::directx::perspective_infinite_reverse(self.fov.to_radians(), aspect, 0.05)
    }
}

/// One indirect draw: kind (0 faces, 1 quads), layer, page, arguments, cube origin (w: offset
/// index), cube center relative to the camera.
type DrawRecord = (u8, u8, u32, DrawIndexedArgs, [i32; 4], Vec3);

/// A mesh's place: the structure (entity) it belongs to, 0 for the world, and its cube.
pub type MeshKey = (u64, IVec3);

pub struct WorldRenderer {
    face_pipes: [PipelineId; 3],
    quad_pipes: [PipelineId; 3],
    faces: Pool,
    quads: Pool,
    meshes: FxHashMap<MeshKey, GpuMesh>,
    globals: Vec<BufferId>,
    origins: Vec<(BufferId, u32)>,
    /// Per frame: camera-relative positions of the structures drawn (index 0: none, the world).
    offsets: Vec<(BufferId, u32)>,
    indirect: Vec<(BufferId, u32)>,
    /// `draw`'s lists, kept between frames.
    scratch: (Vec<(MeshKey, f32, Vec3, i32)>, Vec<DrawRecord>, Vec<usize>),
    /// Of the last `draw`: where its translucent draws start in `scratch.2`, and the buffers
    /// they use (globals, origins, indirect, offsets, the frame's scene).
    translucent: usize,
    buffers: Option<(BufferId, BufferId, BufferId, BufferId, BufferId)>,
    /// Biome table, then per block column of the loaded area its blended colors: tiles of
    /// 16x16 blocks addressed by column position modulo `tint_tiles`.
    biomes: BufferId,
    biome_table: Vec<u32>,
    tint_tiles: u32,
    tex_info: BufferId,
    anim: BufferId,
    /// Start of the overlay table in `anim`.
    overlays: u32,
    /// Block shader slot per texture id (`shader_table`).
    tex_shader: BufferId,
    /// `GameTextures::glow_buffer`, bound as it is.
    glow: BufferId,
    /// `GameTextures::generation` the tables were built from.
    tex_generation: u64,
    /// (0 1 2)(0 2 3) for as many quads as one draw can hold; every face and quad draw shares it
    /// so each corner is shaded once instead of once per triangle.
    quad_index: BufferId,
    pub drawn: u32,
    pub faces_drawn: u64,
}

/// Block shaders: `fn shade(p: BlockPixel) -> vec4<f32>` in `shaders/block/<name>.wgsl`, for
/// blocks with a `shader` component.
pub const HOOK: crate::shaders::Hook = crate::shaders::Hook {
    kind: "block",
    entry: "shade",
    dispatch: "block_shade",
    params: "p: BlockPixel",
    args: "p",
    ret: "vec4<f32>",
    fallback: "return vec4(p.albedo.rgb * p.light, p.albedo.a);",
};

pub fn world_shader(assets: &crate::assets::Assets) -> Result<(Shader, crate::shaders::Slots), ShaderError> {
    let snippets = crate::shaders::snippets(assets, &HOOK);
    let (s, mut slots) = crate::packs::compose(assets, "world.wgsl", &[(&HOOK, &snippets)])?;
    Ok((s, slots.pop().unwrap_or_default()))
}

/// Block shader slot per texture id: a block's `shader` goes with its textures.
fn shader_table(gpu: &mut dyn Backend, c: &Content, textures: &GameTextures, slots: &crate::shaders::Slots) -> BufferId {
    let mut table = vec![0u32; textures.count().max(1)];
    if let Some(comp) = c.components.id(weft_abi::comp::SHADER) {
        for (i, s) in c.states.iter().enumerate() {
            let Some(name) = c.components.text(crate::components::BLOCK, i as u32, comp) else { continue };
            let slot = slots.slot(name);
            if slot == 0 {
                log::warn!("block {}: no block shader {name}", c.block_of(i as u32).name);
                continue;
            }
            for t in s.textures.iter().chain(s.boxes.iter().flat_map(|b| b.textures.iter())) {
                if let Some(e) = table.get_mut(*t as usize) {
                    *e = slot;
                }
            }
        }
    }
    let buf = gpu.create_buffer((table.len() * 4) as u64, usage::STORAGE, "texture shaders");
    gpu.write_buffer(buf, 0, bytemuck::cast_slice(&table));
    buf
}

impl WorldRenderer {
    pub fn new(gpu: &mut dyn Backend, c: &Content, textures: &GameTextures, assets: &crate::assets::Assets) -> Result<WorldRenderer, String> {
        let (shader, slots) = world_shader(assets).map_err(|e| e.0)?;
        let (face_pipes, quad_pipes) = Self::pipelines(gpu, &shader)?;
        let tex_shader = shader_table(gpu, c, textures, &slots);

        let (tex_info, anim, overlays) = Self::texture_info(gpu, textures);

        // Tint kind colors per biome, then the blended columns (`set_tint_tiles`).
        let biome_data: Vec<u32> = if c.biomes.is_empty() {
            c.tint_kinds.iter().map(|k| k.1).collect()
        } else {
            c.biomes.iter().flat_map(|b| b.tints.iter().copied()).collect()
        };
        let biomes = gpu.create_buffer((biome_data.len() * 4) as u64, usage::STORAGE, "biomes");
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
            offsets: Vec::new(),
            globals,
            origins: Vec::new(),
            indirect: Vec::new(),
            scratch: Default::default(),
            translucent: 0,
            buffers: None,
            biomes,
            biome_table: biome_data,
            tint_tiles: 0,
            tex_info,
            anim,
            overlays,
            glow: textures.glow_buffer(),
            tex_shader,
            tex_generation: textures.generation(),
            quad_index,
            drawn: 0,
            faces_drawn: 0,
        })
    }

    /// The block texture tables (`GameTextures::world_tables`) on the GPU.
    fn texture_info(gpu: &mut dyn Backend, textures: &GameTextures) -> (BufferId, BufferId, u32) {
        let (info, frames, overlays) = textures.world_tables();
        let tex_info = gpu.create_buffer((info.len().max(1) * 4) as u64, usage::STORAGE, "texture info");
        gpu.write_buffer(tex_info, 0, bytemuck::cast_slice(&info));
        let anim = gpu.create_buffer((frames.len() * 4) as u64, usage::STORAGE, "texture frames");
        gpu.write_buffer(anim, 0, bytemuck::cast_slice(&frames));
        (tex_info, anim, overlays)
    }

    /// Builds the texture tables again when textures were loaded since (cheap: a few words per
    /// block texture).
    pub fn set_textures(&mut self, gpu: &mut dyn Backend, textures: &GameTextures) {
        self.glow = textures.glow_buffer();
        if self.tex_generation == textures.generation() {
            return;
        }
        gpu.destroy_buffer(self.tex_info);
        gpu.destroy_buffer(self.anim);
        (self.tex_info, self.anim, self.overlays) = Self::texture_info(gpu, textures);
        self.tex_generation = textures.generation();
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
    pub fn reload_shaders(&mut self, gpu: &mut dyn Backend, textures: &GameTextures, assets: &crate::assets::Assets) {
        match world_shader(assets).map_err(|e| e.0).and_then(|(s, slots)| Ok((Self::pipelines(gpu, &s)?, slots))) {
            Ok(((f, q), slots)) => {
                for p in self.face_pipes.into_iter().chain(self.quad_pipes) {
                    gpu.destroy_pipeline(p);
                }
                self.face_pipes = f;
                self.quad_pipes = q;
                gpu.destroy_buffer(self.tex_shader);
                self.tex_shader = shader_table(gpu, crate::content::content(), textures, &slots);
                log::info!("world shader reloaded");
            }
            Err(e) => log::error!("world shader: {e}"),
        }
    }

    /// Sizes the blended color area to `tiles` x `tiles` columns; true when it was reallocated
    /// and every column has to be uploaded again.
    pub fn set_tint_tiles(&mut self, gpu: &mut dyn Backend, tiles: u32) -> bool {
        if tiles == self.tint_tiles {
            return false;
        }
        gpu.destroy_buffer(self.biomes);
        let size = (self.biome_table.len() + (tiles * tiles) as usize * 256 * crate::content::content().tint_kinds.len()) * 4;
        self.biomes = gpu.create_buffer(size as u64, usage::STORAGE, "biomes");
        gpu.write_buffer(self.biomes, 0, bytemuck::cast_slice(&self.biome_table));
        // Zero marks a block without blended colors yet.
        gpu.write_buffer(self.biomes, self.biome_table.len() as u64 * 4, &vec![0u8; size - self.biome_table.len() * 4]);
        self.tint_tiles = tiles;
        true
    }

    pub fn upload_tints(&mut self, gpu: &mut dyn Backend, col: glam::IVec2, tints: &super::world::ColumnTints) {
        let t = self.tint_tiles as i32;
        let slot = (col.x.rem_euclid(t) + col.y.rem_euclid(t) * t) as usize;
        gpu.write_buffer(self.biomes, ((self.biome_table.len() + slot * tints.len()) * 4) as u64, bytemuck::cast_slice(tints));
    }

    pub fn remove(&mut self, key: MeshKey) {
        if let Some(m) = self.meshes.remove(&key) {
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
        for b in self.globals.drain(..).chain([self.biomes, self.tex_info, self.anim, self.quad_index, self.tex_shader]) {
            gpu.destroy_buffer(b);
        }
        for (b, _) in self.origins.drain(..).chain(self.indirect.drain(..)).chain(self.offsets.drain(..)) {
            gpu.destroy_buffer(b);
        }
        for p in self.face_pipes.into_iter().chain(self.quad_pipes) {
            gpu.destroy_pipeline(p);
        }
    }

    /// Every cube of structure `frame` goes (it left).
    pub fn remove_frame(&mut self, frame: u64) {
        let keys: Vec<MeshKey> = self.meshes.keys().copied().filter(|k| k.0 == frame).collect();
        for k in keys {
            self.remove(k);
        }
    }

    pub fn upload(&mut self, gpu: &mut dyn Backend, textures: &GameTextures, key: MeshKey, mesh: &Mesh) {
        self.remove(key);
        if mesh.is_empty() {
            return;
        }
        // The textures it shows are loaded before it is drawn.
        let mut used: Vec<u32> = mesh.faces.iter().flatten().map(|f| f[1] & 0xFFFF).chain(mesh.quads.iter().flatten().map(|q| q[4] & 0xFFFF)).collect();
        used.sort_unstable();
        used.dedup();
        textures.ensure_blocks(gpu, used);
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
        self.meshes.insert(key, GpuMesh { faces, face_counts, quads, quad_counts });
    }

    /// `frames`: structures to draw and where their block (0, 0, 0) is in the world.
    pub fn draw(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, frame: &FrameInfo, cam: &Camera, scene: BufferId, frames: &[(u64, DVec3)]) {
        let aspect = frame.width as f32 / frame.height.max(1) as f32;
        let view_proj = cam.proj(aspect) * cam.view();
        let cam_block = cam.pos.floor();
        let cam_cube: IVec3 = cam_block.as_ivec3() >> 4i32;
        let cam_frac = (cam.pos - (cam_cube * 16).as_dvec3()).as_vec3();
        let globals = Globals {
            cam_frac: [cam_frac.x, cam_frac.y, cam_frac.z, 0.0],
            ids: [0, 0, (self.biome_table.len() / crate::content::content().tint_kinds.len()) as u32, self.tint_tiles],
            cam_cube: cam_cube.extend(0).to_array(),
            extra: [self.overlays, 0, crate::content::content().tint_kinds.len() as u32, 0],
        };
        let gbuf = self.globals[frame.slot];
        gpu.mapped(gbuf)[..size_of::<Globals>()].copy_from_slice(bytemuck::bytes_of(&globals));

        let planes = frustum_planes(view_proj);
        let (mut visible, mut draws, mut sorted) = std::mem::take(&mut self.scratch);
        visible.clear();
        draws.clear();
        sorted.clear();
        // Camera-relative offset of each structure drawn (k + 1 its index; 0 the world).
        let frame_offsets: Vec<Vec3> = frames.iter().map(|f| (f.1 - (cam_cube * 16).as_dvec3()).as_vec3()).collect();
        for (&key, _) in &self.meshes {
            let (lo, k) = if key.0 == 0 {
                ((key.1 - cam_cube).as_vec3() * 16.0 - cam_frac, 0)
            } else {
                let Some(k) = frames.iter().position(|f| f.0 == key.0) else { continue };
                (frame_offsets[k] + key.1.as_vec3() * 16.0 - cam_frac, k as i32 + 1)
            };
            let center = lo + Vec3::splat(8.0);
            if planes.iter().all(|pl| pl.dot(center.extend(1.0)) > -13.86) {
                visible.push((key, center.length_squared(), lo, k));
            }
        }
        visible.sort_by(|a, b| a.1.total_cmp(&b.1));

        let mut faces_drawn = 0u64;
        for &(key, _, lo, k) in &visible {
            let m = &self.meshes[&key];
            let rel = if k == 0 { key.1 - cam_cube } else { key.1 };
            let origin = [rel.x, rel.y, rel.z, k];
            let center = lo + Vec3::splat(8.0);
            if let Some(a) = m.faces {
                // Directions by rank: +X +Y +Z -X -Y -Z. A face direction can be seen only from
                // the side its normal points to; contiguous visible runs become one draw.
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
                                draws.push((0, layer as u8, a.page, DrawIndexedArgs { indices: len * 6, instances: 1, first_index: 0, vertex_offset: (s * 4) as i32, first_instance: 0 }, origin, center));
                            }
                        }
                        off += n;
                    }
                    if let Some((s, len)) = run {
                        draws.push((0, layer as u8, a.page, DrawIndexedArgs { indices: len * 6, instances: 1, first_index: 0, vertex_offset: (s * 4) as i32, first_instance: 0 }, origin, center));
                    }
                }
            }
            if let Some(a) = m.quads {
                let mut off = a.offset;
                for layer in 0..3 {
                    let n = m.quad_counts[layer];
                    if n > 0 {
                        draws.push((1, layer as u8, a.page, DrawIndexedArgs { indices: n * 6, instances: 1, first_index: 0, vertex_offset: (off * 4) as i32, first_instance: 0 }, origin, center));
                    }
                    off += n;
                }
            }
        }
        // Opaque/cutout front to back (early depth rejection), translucent back to front across
        // faces and quads (ice in front of water must come after it). In one cube, seen from
        // above, the quads (liquid surfaces) go first, from below the faces.
        let order = |d: &DrawRecord| (d.1 == 2, d.1, d.0, d.2);
        sorted.extend(0..draws.len());
        sorted.sort_by_key(|&i| order(&draws[i]));
        let translucent_start = sorted.iter().position(|&i| draws[i].1 == 2).unwrap_or(sorted.len());
        let center = |d: &DrawRecord| d.5;
        sorted[translucent_start..].sort_by(|&a, &b| {
            let (ca, cb) = (center(&draws[a]), center(&draws[b]));
            let rank = |d: &DrawRecord, c: Vec3| if c.y < 0.0 { d.0 == 0 } else { d.0 == 1 };
            cb.length_squared().total_cmp(&ca.length_squared()).then(rank(&draws[a], ca).cmp(&rank(&draws[b], cb)))
        });
        self.translucent = translucent_start;

        let n = draws.len() as u32;
        self.drawn = n;
        self.faces_drawn = faces_drawn;
        if n == 0 {
            self.scratch = (visible, draws, sorted);
            self.buffers = None;
            return;
        }
        let obuf = crate::gpu_layout::frame_buffer(gpu, &mut self.origins, frame.slot, n, 16, usage::STORAGE, "origins");
        let fbuf = crate::gpu_layout::frame_buffer(gpu, &mut self.offsets, frame.slot, frames.len() as u32 + 1, 16, usage::STORAGE, "frame offsets");
        let offs = gpu.mapped(fbuf);
        offs[..16].fill(0);
        for (k, o) in frame_offsets.iter().enumerate() {
            offs[(k + 1) * 16..(k + 1) * 16 + 12].copy_from_slice(bytemuck::cast_slice(&o.to_array()));
        }
        let ibuf = crate::gpu_layout::frame_buffer(gpu, &mut self.indirect, frame.slot, n, 20, usage::INDIRECT, "indirect");
        // Straight into the mapped buffers.
        for (out, &i) in gpu.mapped(obuf)[..n as usize * 16].chunks_exact_mut(16).zip(&sorted) {
            out.copy_from_slice(bytemuck::bytes_of(&draws[i].4));
        }
        for (k, (out, &i)) in gpu.mapped(ibuf)[..n as usize * 20].chunks_exact_mut(20).zip(&sorted).enumerate() {
            let mut a = draws[i].3;
            a.first_instance = k as u32;
            out.copy_from_slice(bytemuck::bytes_of(&a));
        }

        self.buffers = Some((gbuf, obuf, ibuf, fbuf, scene));
        self.scratch = (visible, draws, sorted);
        self.emit(list, 0..self.translucent);
    }

    /// The translucent part of the last `draw`, after everything that may stand behind it.
    pub fn draw_translucent(&self, list: &mut CommandList) {
        let n = self.scratch.2.len();
        if self.buffers.is_some() && self.translucent < n {
            self.emit(list, self.translucent..n);
        }
    }

    fn emit(&self, list: &mut CommandList, range: std::ops::Range<usize>) {
        let Some((gbuf, obuf, ibuf, fbuf, scene)) = self.buffers else { return };
        let (_, draws, sorted) = &self.scratch;
        if range.is_empty() {
            return;
        }
        list.push(Cmd::IndexBuffer(self.quad_index));
        let mut k = range.start;
        while k < range.end {
            let (kind, layer, page) = (draws[sorted[k]].0, draws[sorted[k]].1, draws[sorted[k]].2);
            let mut end = k + 1;
            while end < range.end {
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
            list.bind(6, fbuf);
            list.bind(7, self.glow);
            list.bind(8, self.tex_shader);
            list.bind(9, scene);
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
