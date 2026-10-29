//! Terrain made on the GPU (`terrain_gpu.wgsl`): the dimension's terrain description and a
//! batch of cube positions go up, blocks, biomes and columns come back a couple of frames later,
//! and the rest of generation (the dimension's generator, trees, ores, structures, light) is
//! finished on the CPU workers. Integer math only on both sides, so a cube is the same bits
//! whoever made it (`core:terrain_check` compares every one).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender};
use glam::IVec3;
use weft_abi::TerrainColumn;
use weft_noise::fixed::{Fields, seed32};
use weft_render::{Backend, BufferId, Cmd, CommandList, ComputeDesc, PipelineId, Shader, usage};

use crate::content::{Content, content};
use crate::server::area::{self, AreaFill};
use crate::server::terrain::{Height, Mask};
use crate::world::Cube;

/// Words per cube coming back: blocks and biomes two per word, column heights, column biomes.
const OUT_WORDS: usize = 2560;
/// Per mod feature and cube: heights at the 256 columns and the 25 corners caves measure from.
const CUSTOM_WORDS: usize = 281;
/// Cubes per frame at most.
const BATCH: usize = 4096;

pub static GPU_CUBES: AtomicU64 = AtomicU64::new(0);
pub static GPU_MISMATCHES: AtomicU64 = AtomicU64::new(0);

/// A cube for the GPU, with the heights of mods' own features (their code runs here).
pub struct Job {
    pub epoch: u64,
    pub dim: u32,
    pub seed: u64,
    pub pos: IVec3,
    pub custom: Arc<Vec<i32>>,
    pub edits: Arc<Vec<AreaFill>>,
    pub done: Sender<super::terrain::Made>,
}

/// What `terrain_gpu.wgsl` reads for dimension `dim`, and the indices of the features that run
/// mods' code (their heights go up with each cube); `None` when the GPU cannot make it.
pub fn describe(c: &Content, dim: u32, seed: u64) -> Option<(Vec<i32>, Vec<usize>)> {
    let def = &c.dims[dim as usize];
    let t = def.terrain.as_ref().filter(|_| def.builtin.is_none())?;
    if t.features.len() > 32 || c.states.len() > 65536 {
        return None;
    }
    let mut w = vec![0i32; 29];
    w[0] = t.stone as i32;
    w[1] = t.base;
    if let Some((level, water)) = t.sea {
        (w[2], w[3], w[4]) = (1, level, water as i32);
    }
    if let Some(caves) = &t.caves {
        (w[5], w[6], w[7], w[8]) = (1, caves.lava as i32, caves.lava_from, caves.lava_to);
    }
    (w[9], w[10], w[11]) = (t.features.len() as i32, t.rules.len() as i32, t.cave_biomes.len() as i32);
    for (i, salt) in [0x11, 0x22, 0x33, 0x55, 0x66, 0x77, 0x88, 0x99, 0xC7A7, 0xC1, 0xC2, 0xC3, 0xC4, 0x1A7A].into_iter().enumerate() {
        w[12 + i] = seed32(seed, salt) as i32;
    }
    // Features, their masks after them.
    w[26] = w.len() as i32;
    let at = w.len();
    w.resize(at + t.features.len() * 8, 0);
    let mut custom = Vec::new();
    for (i, f) in t.features.iter().enumerate() {
        let r = at + i * 8;
        match f.height {
            Height::Preset(kind, p) => {
                w[r] = kind as i32;
                w[r + 1..r + 5].copy_from_slice(&p);
                w[r + 5] = -1;
            }
            Height::Custom(..) => {
                w[r + 5] = custom.len() as i32;
                custom.push(i);
            }
        }
        if let Some(m) = &f.mask {
            let start = w.len();
            if postfix(m, seed, &mut w, 0)? > 8 {
                return None;
            }
            w[r + 6] = start as i32;
            w[r + 7] = ((w.len() - start) / 6) as i32;
        }
    }
    w[27] = w.len() as i32;
    let opt = |v: Option<u32>| v.map_or(-1, |v| v as i32);
    for r in &t.rules {
        let mut rule = [0i32; 24];
        rule[0] = r.biome as i32;
        (rule[1], rule[2]) = r.temperature;
        (rule[3], rule[4]) = r.humidity;
        (rule[5], rule[6]) = r.continent;
        (rule[7], rule[8]) = r.height;
        (rule[9], rule[10], rule[11]) = match r.feature {
            None => (-1, 0, 0),
            Some((i, lo, hi)) => (if i < 32 { i as i32 } else { -2 }, lo, hi),
        };
        rule[12] = opt(r.top);
        rule[13] = opt(r.filler);
        rule[14] = r.depth;
        (rule[15], rule[16]) = r.under.map_or((-1, 0), |(b, n)| (b as i32, n));
        (rule[17], rule[18]) = r.deep_top.map_or((-1, 0), |(b, y)| (b as i32, y));
        rule[19] = opt(r.ice);
        (rule[22], rule[23]) = r.bare_above.map_or((0, 0), |y| (1, y));
        w.extend_from_slice(&rule);
    }
    w[28] = w.len() as i32;
    for (k, b) in t.cave_biomes.iter().enumerate() {
        w.extend_from_slice(&[b.biome as i32, b.depth, b.size, b.threshold, seed32(seed, 0xCB10 ^ k as u64) as i32]);
    }
    Some((w, custom))
}

/// Writes a mask as postfix steps of six words; returns the stack depth it needs.
fn postfix(m: &Mask, seed: u64, w: &mut Vec<i32>, depth: usize) -> Option<usize> {
    Some(match m {
        Mask::Noise { salt, scale, octaves, from, to } => {
            w.extend_from_slice(&[1, seed32(seed, *salt) as i32, *scale, *octaves as i32, *from, *to]);
            depth + 1
        }
        Mask::Continent(a, b) => {
            w.extend_from_slice(&[2, 0, 0, 0, *a, *b]);
            depth + 1
        }
        Mask::Temperature(a, b) => {
            w.extend_from_slice(&[3, 0, 0, 0, *a, *b]);
            depth + 1
        }
        Mask::Humidity(a, b) => {
            w.extend_from_slice(&[4, 0, 0, 0, *a, *b]);
            depth + 1
        }
        Mask::All(v) => {
            let mut most = depth;
            for (i, m) in v.iter().enumerate() {
                most = most.max(postfix(m, seed, w, depth + i)?);
            }
            w.extend_from_slice(&[5, v.len() as i32, 0, 0, 0, 0]);
            most.max(depth + 1)
        }
        Mask::Not(m) => {
            let most = postfix(m, seed, w, depth)?;
            w.extend_from_slice(&[6, 0, 0, 0, 0, 0]);
            most
        }
    })
}

/// Heights of the mod features `custom` at the columns and corners of cube column (`x`, `z`),
/// for its jobs.
pub fn custom_heights(c: &Content, dim: u32, seed: u64, x: i32, z: i32, custom: &[usize]) -> Arc<Vec<i32>> {
    let def = &c.dims[dim as usize];
    def.custom_cache.get(seed, x, z, || {
        let Some(t) = def.terrain.as_ref() else { return Vec::new() };
        let f = Fields { seed };
        let (ox, oz) = (x * 16, z * 16);
        let mut out = Vec::with_capacity(custom.len() * CUSTOM_WORDS);
        for &i in custom {
            for ci in 0..CUSTOM_WORDS as i32 {
                let (x, z) = if ci < 256 { (ox + ci % 16, oz + ci / 16) } else { (ox + (ci - 256) % 5 * 4, oz + (ci - 256) / 5 * 4) };
                out.push(t.features[i].height_at(&f, x * 16, z * 16));
            }
        }
        out
    })
}

#[derive(Default)]
struct Slot {
    desc: Option<(BufferId, usize)>,
    desc_key: Option<(u32, u64)>,
    jobs: Option<(BufferId, usize)>,
    /// Two, so the next batch goes into one while what came back in the other is read.
    out: [Option<(BufferId, usize)>; 2],
    /// Which `out` the batch in flight writes.
    flight_out: usize,
    flight: Vec<Job>,
    /// What came back, while workers still read it.
    reading: [Option<Arc<Readback>>; 2],
}

/// A frame slot's output buffer as workers read it, straight from where the GPU wrote it.
struct Readback(*const u32, usize);

// The buffer is neither written nor freed while anyone holds this (`Slot::reading`).
unsafe impl Send for Readback {}
unsafe impl Sync for Readback {}

impl Readback {
    fn cube(&self, i: usize) -> &[u32] {
        assert!((i + 1) * OUT_WORDS <= self.1);
        unsafe { std::slice::from_raw_parts(self.0.add(i * OUT_WORDS), OUT_WORDS) }
    }
}

/// The main thread's end: dispatches batches into the frame and hands what came back to the
/// workers.
pub struct GpuTerrain {
    pipeline: Option<PipelineId>,
    rx: Receiver<Job>,
    queue: VecDeque<Job>,
    slots: Vec<Slot>,
    /// (dim, seed) and its description and mod features.
    desc: Option<((u32, u64), Vec<i32>, usize)>,
    /// Set once the pipeline works: the terrain thread sends jobs here instead of to the CPU.
    pub on: Arc<AtomicBool>,
}

impl GpuTerrain {
    pub fn new(gpu: &mut dyn Backend, rx: Receiver<Job>, on: Arc<AtomicBool>) -> GpuTerrain {
        let pipeline = Shader::from_wgsl("terrain_gpu.wgsl", include_str!("terrain_gpu.wgsl"))
            .map_err(|e| e.0)
            .and_then(|s| gpu.create_compute(&ComputeDesc { label: "terrain", shader: &s, entry: "generate" }).map_err(|e| e.0));
        let pipeline = match pipeline {
            Ok(p) => Some(p),
            Err(e) => {
                log::error!("terrain on the GPU unavailable, generating on the CPU: {e}");
                None
            }
        };
        on.store(pipeline.is_some() && enabled(), Ordering::Relaxed);
        GpuTerrain { pipeline, rx, queue: VecDeque::new(), slots: Vec::new(), desc: None, on }
    }

    /// Frees its buffers, once the workers reading them are done.
    pub fn destroy(&mut self, gpu: &mut dyn Backend) {
        for s in self.slots.drain(..) {
            while s.reading.iter().flatten().any(|r| Arc::strong_count(r) > 1) {
                std::thread::yield_now();
            }
            for (b, _) in [s.desc, s.jobs, s.out[0], s.out[1]].into_iter().flatten() {
                gpu.destroy_buffer(b);
            }
        }
        if let Some(p) = self.pipeline.take() {
            gpu.destroy_pipeline(p);
        }
    }

    /// Takes in what the GPU finished in this slot's last frame and records the next batch.
    pub fn frame(&mut self, gpu: &mut dyn Backend, list: &mut CommandList, slot: usize) {
        let Some(pipeline) = self.pipeline else { return };
        self.on.store(enabled(), Ordering::Relaxed);
        while self.slots.len() < gpu.frames_in_flight() {
            self.slots.push(Slot::default());
        }
        let s = &mut self.slots[slot];
        if !s.flight.is_empty() {
            let (out, _) = s.out[s.flight_out].unwrap();
            let words: &[u32] = bytemuck::cast_slice(&gpu.mapped(out)[..s.flight.len() * OUT_WORDS * 4]);
            let back = Arc::new(Readback(words.as_ptr(), words.len()));
            // A column's cubes come together: what they share is worked out first, once.
            let mut columns: rustc_hash::FxHashMap<(i32, i32), Vec<(usize, Job)>> = Default::default();
            for (i, job) in s.flight.drain(..).enumerate() {
                columns.entry((job.pos.x, job.pos.z)).or_default().push((i, job));
            }
            // Unpacked on threads of their own (the other pools are busy while loading): the
            // slot waits for it.
            for ((x, z), jobs) in columns {
                let back = back.clone();
                unpacking().spawn(move || {
                    let made: Vec<(Job, Vec<u32>)> = jobs.into_iter().map(|(i, job)| (job, back.cube(i).to_vec())).collect();
                    drop(back);
                    crate::server::dim::warm_scatters(content(), made[0].0.dim, made[0].0.seed, x, z);
                    for (job, words) in made {
                        crate::background().spawn(move || finish(job, unpack(&words)));
                    }
                });
            }
            s.reading[s.flight_out] = Some(back);
        }
        for r in &mut s.reading {
            if r.as_ref().is_some_and(|r| Arc::strong_count(r) == 1) {
                *r = None;
            }
        }
        let Some(next) = (0..2).find(|&k| s.reading[k].is_none()) else { return };
        self.queue.extend(self.rx.try_iter());
        let Some(first) = self.queue.front() else { return };
        let key = (first.dim, first.seed);
        if self.desc.as_ref().is_none_or(|d| d.0 != key) {
            let Some((words, custom)) = describe(content(), key.0, key.1) else {
                // The terrain thread asked for a dimension the GPU cannot make: done by the CPU.
                for job in self.queue.drain(..) {
                    crate::background().spawn(move || cpu(job));
                }
                return;
            };
            self.desc = Some((key, words, custom.len()));
        }
        let (_, words, custom) = self.desc.as_ref().unwrap();
        let mut batch = Vec::new();
        while batch.len() < BATCH && self.queue.front().is_some_and(|j| (j.dim, j.seed) == key) {
            batch.push(self.queue.pop_front().unwrap());
        }
        let stride = 4 + custom * CUSTOM_WORDS;
        let mut jobs = vec![0i32; batch.len() * stride];
        for (i, j) in batch.iter().enumerate() {
            let r = i * stride;
            jobs[r..r + 3].copy_from_slice(&j.pos.to_array());
            if j.custom.len() == custom * CUSTOM_WORDS {
                jobs[r + 4..r + stride].copy_from_slice(&j.custom);
            }
        }
        let desc = sized(gpu, &mut s.desc, words.len() * 4, usage::STORAGE | usage::HOST, "terrain desc");
        if s.desc_key != Some(key) {
            gpu.mapped(desc)[..words.len() * 4].copy_from_slice(bytemuck::cast_slice(words));
            s.desc_key = Some(key);
        }
        let jobs_buf = sized(gpu, &mut s.jobs, BATCH * stride * 4, usage::STORAGE | usage::HOST, "terrain jobs");
        gpu.mapped(jobs_buf)[..jobs.len() * 4].copy_from_slice(bytemuck::cast_slice(&jobs));
        let out = sized(gpu, &mut s.out[next], BATCH * OUT_WORDS * 4, usage::STORAGE | usage::READBACK, "terrain out");
        s.flight_out = next;
        list.push(Cmd::SetPipeline(pipeline));
        list.bind(0, desc);
        list.bind(1, jobs_buf);
        list.bind(2, out);
        list.push_constants(&[batch.len() as u32, *custom as u32]);
        list.push(Cmd::Dispatch { x: batch.len() as u32, y: 1, z: 1 });
        crate::diag::TERRAIN_GPU.add(batch.len() as u64);
        list.push(Cmd::Barrier);
        s.flight = batch;
    }
}

fn unpacking() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(4).thread_name(|i| format!("terrain unpack {i}")).start_handler(|_| crate::lower_priority()).build().expect("terrain unpack pool"))
}

fn enabled() -> bool {
    let c = content();
    c.game.config_names.get("core:terrain_gpu").is_none_or(|&id| crate::config::get(id) != 0.0)
}

fn checking() -> bool {
    let c = content();
    c.game.config_names.get("core:terrain_check").is_some_and(|&id| crate::config::get(id) != 0.0)
}

/// A buffer of at least `bytes`, made again when too small.
fn sized(gpu: &mut dyn Backend, slot: &mut Option<(BufferId, usize)>, bytes: usize, usage: u32, label: &str) -> BufferId {
    match *slot {
        Some((b, size)) if size >= bytes => b,
        old => {
            if let Some((b, _)) = old {
                gpu.destroy_buffer(b);
            }
            let b = gpu.create_buffer(bytes.max(4) as u64, usage, label);
            *slot = Some((b, bytes));
            b
        }
    }
}

/// The CPU's way, for jobs the GPU cannot take.
fn cpu(job: Job) {
    let c = content();
    let mut cube = crate::server::dim::generate(c, job.dim, job.seed, job.pos);
    done(c, job, &mut cube);
}

/// Unpacks what the GPU made and finishes the cube like the CPU would.
/// A cube as the GPU made it: blocks, biomes, columns.
type Unpacked = (Vec<u32>, [u16; 256], [TerrainColumn; 256]);

fn unpack(w: &[u32]) -> Unpacked {
    let blocks: Vec<u32> = (0..4096).map(|i| (w[i / 2] >> ((i & 1) * 16)) & 0xFFFF).collect();
    let half = |at: usize, i: usize| ((w[at + i / 2] >> ((i & 1) * 16)) & 0xFFFF) as u16;
    let biomes: [u16; 256] = std::array::from_fn(|i| half(2048, i));
    let cols: [TerrainColumn; 256] = std::array::from_fn(|i| TerrainColumn { height: w[2176 + i] as i32, biome: half(2432, i) as u32 });
    (blocks, biomes, cols)
}

fn finish(job: Job, (blocks, biomes, cols): Unpacked) {
    let t = std::time::Instant::now();
    let c = content();
    let mut cube = crate::server::dim::finish_generated(c, job.dim, job.seed, job.pos, blocks, biomes, Some(Arc::new(cols)), false);
    GPU_CUBES.fetch_add(1, Ordering::Relaxed);
    if checking() {
        let want = crate::server::dim::generate(c, job.dim, job.seed, job.pos);
        let differs = (0..4096).find(|&i| cube.get(i) != want.get(i));
        let biome_differs = (0..256).find(|&i| cube.biome(i % 16, i / 16) != want.biome(i % 16, i / 16));
        // Light too, against the way it was done before (lit as it will be below).
        let mut lit = cube.clone();
        area::catch_up(c, &job.edits, job.pos, &mut lit);
        let mut old = lit.clone();
        crate::world::light::prelight(c, &mut lit, c.dims[job.dim as usize].sky_light);
        crate::world::light::prelight_reference(c, &mut old, c.dims[job.dim as usize].sky_light);
        let light_differs = (0..4096).find(|&i| lit.light(i) != old.light(i));
        if let Some(i) = light_differs {
            if GPU_MISMATCHES.fetch_add(1, Ordering::Relaxed) < 8 {
                log::warn!("prelight differs at cube {} block {i}: {:#x} instead of {:#x}", job.pos, lit.light(i), old.light(i));
            }
        }
        if differs.is_some() || biome_differs.is_some() {
            if GPU_MISMATCHES.fetch_add(1, Ordering::Relaxed) < 8 {
                match differs {
                    Some(i) => log::warn!("GPU terrain differs at cube {} block {i}: {} instead of {}", job.pos, cube.get(i), want.get(i)),
                    None => log::warn!("GPU terrain differs at cube {} in biomes", job.pos),
                }
            }
        }
    }
    done(c, job, &mut cube);
    crate::diag::TERRAIN_FINISHED.add(1);
    crate::diag::TERRAIN_FINISH_NS.add(t.elapsed().as_nanos() as u64);
}

/// Edits made since it was asked for, then its light on its own (as generation workers do).
fn done(c: &Content, job: Job, cube: &mut Cube) {
    area::catch_up(c, &job.edits, job.pos, cube);
    crate::world::light::prelight(c, cube, c.dims[job.dim as usize].sky_light);
    let _ = job.done.send((job.epoch, job.pos, std::mem::replace(cube, Cube::uniform(0, 0))));
}
