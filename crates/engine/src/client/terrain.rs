//! The world the client draws, made here rather than sent: cubes in view are generated from the
//! seed exactly as the server would, the dimension's area edits are made on them and they are
//! lit here. The server sends only what it simulates around players and what differs from
//! generation (`ToClient::Saved`, then the cubes); those replace what was generated. Runs on its
//! own thread and hands the client world finished cubes, a batch per round.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use glam::{IVec2, IVec3};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::content::{Content, content};
use crate::server::area::{self, AreaFill};
use crate::world::Cube;
use crate::world::light::{CubeMap, Lighter};

pub enum In {
    Reset { dim: u32, seed: u64 },
    View { center: IVec3, radius: i32 },
    /// A cube the server sent: it wins over generation from now on.
    Cube { pos: IVec3, cube: Arc<Cube> },
    Blocks(Vec<(IVec3, u32)>),
    Unload(IVec3),
    Saved(Vec<IVec3>),
    Edits { specs: Vec<String>, all: bool },
    /// What came from the server in one frame, handled in one round (one batch out).
    Many(Vec<In>),
}

pub enum Out {
    /// `hidden`: the cubes around it never coming (`cube::around_mask` bits).
    Cube { pos: IVec3, cube: Arc<Cube>, hidden: u32 },
    Blocks(Vec<(IVec3, u32)>),
    Unload(IVec3),
}

/// The client's end: send what the server says and where the player is, take finished batches.
pub struct TerrainHandle {
    pub tx: Sender<In>,
    /// Batches with the number of resets they come after.
    pub rx: Receiver<(u64, Vec<Out>)>,
    /// Cubes to generate, generating or waiting for the one above: 0 when the view is complete.
    pub pending: Arc<AtomicU64>,
}

/// `gpu`: where jobs go while `gpu_on` (the GPU makes the terrain, `terrain_gpu`).
pub fn spawn(gpu: Sender<super::terrain_gpu::Job>, gpu_on: Arc<std::sync::atomic::AtomicBool>) -> TerrainHandle {
    let (tx, in_rx) = unbounded();
    let (out_tx, rx) = unbounded();
    let pending = Arc::new(AtomicU64::new(0));
    let p = pending.clone();
    std::thread::Builder::new()
        .name("terrain".into())
        .spawn(move || {
            let (gen_tx, gen_rx) = unbounded();
            let mut t = Terrain::new(p, gen_tx);
            t.gpu = Some((gpu, gpu_on));
            t.run(in_rx, gen_rx, out_tx);
        })
        .expect("terrain thread");
    TerrainHandle { tx, rx, pending }
}

/// Whether the visibility fill picks what is generated (`core:client_visibility`: 0 never, 1
/// always, 2 when the CPU makes the terrain). It saves a sixth of the cubes, but its front
/// moving out reaches cubes next to ones already meshed, which are meshed again: with the GPU
/// making terrain that costs more than it saves (view 32: 6 s instead of 4, view 64: about the
/// same), with the CPU it saves (view 64: 47 s instead of 57).
static GPU_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn visibility_on() -> bool {
    let c = content();
    match c.game.config_names.get("core:client_visibility").map(|&id| crate::config::get(id) as i32) {
        Some(0) => false,
        Some(1) => true,
        _ => !GPU_ON.load(Ordering::Relaxed),
    }
}

/// Threads for lighting what arrived: the background pool's queue is long with generation and
/// meshing while loading, and the terrain thread waits for this.
pub fn pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(crate::workers()).thread_name(|i| format!("terrain light {i}")).build().expect("terrain light pool"))
}

/// The terrain as the visibility fill sees it.
struct Seen<'a> {
    cubes: &'a FxHashMap<IVec3, Arc<Cube>>,
    conn: &'a mut FxHashMap<IVec3, u16>,
}

impl crate::server::visibility::Cubes for Seen<'_> {
    fn has(&self, pos: IVec3) -> bool {
        self.cubes.contains_key(&pos)
    }
    fn connectivity(&mut self, c: &Content, pos: IVec3) -> Option<u16> {
        let cube = self.cubes.get(&pos)?;
        Some(*self.conn.entry(pos).or_insert_with(|| crate::server::dim::see_through(c, cube)))
    }
}

struct Map<'a>(&'a mut FxHashMap<IVec3, Arc<Cube>>);

/// The cubes, to read only.
struct Read<'a>(&'a FxHashMap<IVec3, Arc<Cube>>);

impl CubeMap for Read<'_> {
    fn cube(&self, c: IVec3) -> Option<&Cube> {
        self.0.get(&c).map(|a| &**a)
    }
    fn cube_mut(&mut self, _: IVec3) -> Option<&mut Cube> {
        None
    }
}

/// A run of cubes down a column being lit on a worker: its own cubes, over the ones already
/// in (read only: light toward them spreads out later, `outward`).
struct Run<'a> {
    cubes: &'a FxHashMap<IVec3, Arc<Cube>>,
    /// Its cubes by height below `top` (none where one was left out).
    own: Vec<Option<(IVec3, Cube)>>,
    top: IVec3,
}

impl Run<'_> {
    fn slot(&self, c: IVec3) -> Option<usize> {
        let i = self.top.y - c.y;
        (c.x == self.top.x && c.z == self.top.z && i >= 0 && (i as usize) < self.own.len() && self.own[i as usize].is_some()).then_some(i as usize)
    }
}

impl CubeMap for Run<'_> {
    fn cube(&self, c: IVec3) -> Option<&Cube> {
        match self.slot(c) {
            Some(i) => self.own[i].as_ref().map(|o| &o.1),
            None => self.cubes.get(&c).map(|a| &**a),
        }
    }
    fn cube_mut(&mut self, c: IVec3) -> Option<&mut Cube> {
        let i = self.slot(c)?;
        self.own[i].as_mut().map(|o| &mut o.1)
    }
    fn writable(&self, c: IVec3) -> bool {
        self.slot(c).is_some()
    }
}

/// Lights a run down a column (top first) against `cubes`. Each cube came lit on its own as if
/// open sky were above it (`lit`, unless an edit came since): right when the cube above lets
/// full sky through, and without its sky light when none comes through (block light does not
/// depend on it); otherwise lit again.
fn light_run(c: &Content, def: &crate::content::DimensionDef, cubes: &FxHashMap<IVec3, Arc<Cube>>, hi: i32, run: Vec<(IVec3, Cube, bool)>) -> Vec<(IVec3, Cube)> {
    // Sky coming down from a cube: all full, none, or some (`None`).
    let from_above = |a: &Cube| {
        let sky = |i: usize| a.light(crate::world::cube::index(i % 16, 0, i / 16)) & 0xFF;
        match a.uniform_light() {
            Some(l) => Some(l & 0xFF),
            None => Some(sky(0)).filter(|&v| (1..256).all(|i| sky(i) == v)),
        }
        .and_then(|v| match v {
            0 => Some(false),
            15 => Some(true),
            _ => None,
        })
    };
    let top = run[0].0;
    let mut map = Run { cubes, own: Vec::with_capacity((top.y - run[run.len() - 1].0.y + 1) as usize), top };
    for (pos, mut cube, lit) in run {
        let open_above = pos.y + 1 >= def.sky_cube_y || pos.y + 1 > def.max_cube_y || pos.y >= hi;
        let full = match map.cube(pos + IVec3::Y) {
            _ if !def.sky_light => Some(true),
            Some(a) => from_above(a),
            None => Some(open_above),
        };
        let prelit = lit && full.is_some();
        if prelit && full == Some(false) {
            cube.mask_light(!0xFF);
        }
        map.own.resize_with((top.y - pos.y) as usize, || None);
        map.own.push(Some((pos, cube)));
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        if prelit {
            lighter.light_prelit(pos);
        } else {
            lighter.light_new(pos, open_above);
        }
    }
    map.own.into_iter().flatten().collect()
}

impl CubeMap for Map<'_> {
    fn cube(&self, c: IVec3) -> Option<&Cube> {
        self.0.get(&c).map(|a| &**a)
    }
    fn cube_mut(&mut self, c: IVec3) -> Option<&mut Cube> {
        self.0.get_mut(&c).map(Arc::make_mut)
    }
}

/// A generated cube: the round it was asked in (`Terrain::epoch`), where, what.
pub type Made = (u64, IVec3, Cube);

struct Terrain {
    dim: u32,
    seed: u64,
    /// Bumped on every reset: cubes asked for before it are dropped when they come.
    epoch: u64,
    active: bool,
    cubes: FxHashMap<IVec3, Arc<Cube>>,
    /// Sent by the server: never replaced by generation.
    server: FxHashSet<IVec3>,
    /// The server says they differ from generation and will send them: not generated.
    saved: FxHashSet<IVec3>,
    edits: Arc<Vec<AreaFill>>,
    center: IVec3,
    radius: i32,
    /// Lowest and highest cube loaded (`lo..=hi`), for the current view.
    lo: i32,
    hi: i32,
    /// Columns by distance from the center, and how far the scan for missing cubes got.
    offsets: Vec<IVec2>,
    cursor: usize,
    /// Columns with every cube loaded, asked for or saved, for the current `lo..=hi`.
    done: FxHashSet<IVec2>,
    generating: FxHashSet<IVec3>,
    /// Generated, waiting for the cube above (sky light comes from it); whether its light from
    /// the worker still holds (an edit that came meanwhile changed its blocks).
    waiting: FxHashMap<IVec3, (Cube, bool)>,
    gen_tx: Sender<Made>,
    out: Vec<Out>,
    pending: Arc<AtomicU64>,
    gpu: Option<(Sender<super::terrain_gpu::Job>, Arc<std::sync::atomic::AtomicBool>)>,
    /// The features of this dimension whose heights the CPU gives the GPU; `None`: the GPU
    /// cannot make its terrain.
    gpu_custom: Option<Arc<Vec<usize>>>,
    vis: crate::server::visibility::Visibility,
    /// Which faces of each cube see each other, for `vis`; cubes changed since go into
    /// `edited`, and the fill looks at them again.
    conn: FxHashMap<IVec3, u16>,
    edited: Vec<IVec3>,
    /// Cubes not handed to the client world yet (waiting for their neighbors, or just put
    /// in); the others in `cubes` it has. Kept this way round: it is the small set.
    held: FxHashSet<IVec3>,
    /// What the fill wants, farthest first, and when it was gathered.
    wanted: Vec<IVec3>,
    wanted_at: Option<std::time::Instant>,
    /// `visibility_on()` for this round.
    vis_on: bool,
}

impl Terrain {
    fn new(pending: Arc<AtomicU64>, gen_tx: Sender<Made>) -> Terrain {
        Terrain {
            dim: 0,
            seed: 0,
            epoch: 0,
            active: false,
            cubes: FxHashMap::default(),
            server: FxHashSet::default(),
            saved: FxHashSet::default(),
            edits: Arc::new(Vec::new()),
            center: IVec3::ZERO,
            radius: 0,
            lo: 0,
            hi: -1,
            offsets: Vec::new(),
            cursor: 0,
            done: FxHashSet::default(),
            generating: FxHashSet::default(),
            waiting: FxHashMap::default(),
            gen_tx,
            out: Vec::new(),
            pending,
            gpu: None,
            gpu_custom: None,
            vis: crate::server::visibility::Visibility::new(),
            conn: FxHashMap::default(),
            edited: Vec::new(),
            held: FxHashSet::default(),
            wanted: Vec::new(),
            wanted_at: None,
            vis_on: false,
        }
    }

    fn run(&mut self, in_rx: Receiver<In>, gen_rx: Receiver<Made>, out_tx: Sender<(u64, Vec<Out>)>) {
        loop {
            select! {
                recv(in_rx) -> m => match m {
                    Ok(m) => self.handle(m),
                    Err(_) => return,
                },
                recv(gen_rx) -> m => {
                    if let Ok(m) = m {
                        self.made(m);
                    }
                }
                default(Duration::from_millis(5)) => {}
            }
            let busy = std::time::Instant::now();
            self.vis_on = visibility_on();
            // Everything waiting, in one round.
            while let Ok(m) = in_rx.try_recv() {
                self.handle(m);
            }
            while let Ok(m) = gen_rx.try_recv() {
                self.made(m);
            }
            self.insert_waiting();
            self.request();
            if !self.out.is_empty() && out_tx.send((self.epoch, std::mem::take(&mut self.out))).is_err() {
                return;
            }
            crate::diag::TERRAIN_BUSY_NS.add(busy.elapsed().as_nanos() as u64);
            let scanning = if self.vis_on { !self.wanted.is_empty() } else { self.cursor < self.offsets.len() };
            let left = self.generating.len() + self.waiting.len() + scanning as usize;
            self.pending.store(left as u64, Ordering::Relaxed);
        }
    }

    fn handle(&mut self, m: In) {
        let c = content();
        match m {
            In::Many(list) => {
                for m in list {
                    self.handle(m);
                }
            }
            In::Reset { dim, seed } => {
                // The client world was cleared with it.
                let gpu = self.gpu.take();
                *self = Terrain { epoch: self.epoch + 1, offsets: std::mem::take(&mut self.offsets), gpu, ..Terrain::new(self.pending.clone(), self.gen_tx.clone()) };
                (self.dim, self.seed, self.active) = (dim, seed, true);
                self.gpu_custom = super::terrain_gpu::describe(c, dim, seed).map(|(_, custom)| Arc::new(custom));
                self.radius = 0;
            }
            In::View { center, radius } => self.set_view(center, radius),
            In::Cube { pos, cube } => {
                self.server.insert(pos);
                self.waiting.remove(&pos);
                if self.cubes.insert(pos, cube).is_some() {
                    self.conn.remove(&pos);
                    self.edited.push(pos);
                } else {
                    self.held.insert(pos);
                }
                // Its light is the server's; light from cubes made here joins it.
                let changed = self.light_sent(pos);
                self.arrived(pos);
                self.emit_all(changed);
            }
            In::Blocks(changes) => {
                for &(p, s) in &changes {
                    let cp = crate::world::cube_of(p);
                    if let Some(cube) = self.cubes.get_mut(&cp) {
                        Arc::make_mut(cube).set(crate::world::local_index(p), s);
                        if self.conn.remove(&cp).is_some() {
                            self.edited.push(cp);
                        }
                    }
                }
                self.out.push(Out::Blocks(changes));
            }
            In::Unload(pos) => {
                self.server.remove(&pos);
            }
            In::Saved(list) => {
                for p in list {
                    self.saved.insert(p);
                }
            }
            In::Edits { specs, all } => {
                let parsed: Vec<AreaFill> = specs.iter().filter_map(|s| AreaFill::parse(c, s)).collect();
                if all {
                    self.edits = Arc::new(parsed);
                    return;
                }
                let mut dirty = FxHashSet::default();
                for fill in &parsed {
                    area::fill_cubes(c, self.dim, fill, &mut self.cubes, &mut dirty);
                }
                Arc::make_mut(&mut self.edits).extend(parsed);
                for &p in &dirty {
                    if self.conn.remove(&p).is_some() {
                        self.edited.push(p);
                    }
                }
                self.emit_all(dirty);
            }
        }
    }

    fn in_view(&self, p: IVec3) -> bool {
        let d = p - self.center;
        d.x * d.x + d.z * d.z <= self.radius * self.radius && p.y >= self.lo && p.y <= self.hi
    }

    fn set_view(&mut self, center: IVec3, radius: i32) {
        if !self.active || (center == self.center && radius == self.radius) {
            return;
        }
        let c = content();
        let def = &c.dims[self.dim as usize];
        if radius != self.radius || self.offsets.is_empty() {
            self.offsets = (-radius..=radius).flat_map(|z| (-radius..=radius).map(move |x| IVec2::new(x, z))).filter(|o| o.length_squared() <= radius * radius).collect();
            self.offsets.sort_by_key(|o| o.length_squared());
        }
        // As far down as sideways; flying higher, down to the ground and a little into it.
        let air = (1..=2 * radius).take_while(|&d| self.cubes.get(&(center - IVec3::Y * d)).is_some_and(|c| c.uniform_state() == Some(crate::content::AIR))).count() as i32;
        let down = radius.max(air + 8);
        let lo = (center.y - down).max(def.min_cube_y);
        let top = center.y + radius;
        // Above the view the column goes up to open sky, so light coming down is right.
        let hi = if def.sky_light && def.sky_cube_y != i32::MAX { top.max((def.sky_cube_y - 1).min(top + 12)) } else { top }.min(def.max_cube_y);
        if (lo, hi) != (self.lo, self.hi) {
            self.done.clear();
        }
        (self.center, self.radius, self.lo, self.hi) = (center, radius, lo, hi);
        self.cursor = 0;
        // What left the view goes (a cube more around it, against flicker at the edge).
        let gone: Vec<IVec3> = self
            .cubes
            .keys()
            .copied()
            .filter(|&p| {
                let d = p - center;
                d.x * d.x + d.z * d.z > (radius + 2) * (radius + 2) || p.y < lo - 2 || p.y > hi + 2
            })
            .collect();
        for p in gone {
            self.cubes.remove(&p);
            self.server.remove(&p);
            self.conn.remove(&p);
            if !self.held.remove(&p) {
                self.out.push(Out::Unload(p));
            }
        }
        self.saved.retain(|&p| {
            let d = p - center;
            d.x * d.x + d.z * d.z <= (radius + 2) * (radius + 2)
        });
        self.done.retain(|col| (*col - IVec2::new(center.x, center.z)).length_squared() <= radius * radius);
        self.emit_held();
    }

    /// Asks for the nearest missing cubes, each column from the top down: what the visibility
    /// fill reached (`core:client_visibility`), or every cube in view.
    fn request(&mut self) {
        if !self.active || self.radius == 0 {
            return;
        }
        // The GPU answers a few frames later whatever the count: it needs far more asked at once
        // to stay busy than the workers do.
        let gpu = self.gpu_custom.is_some() && self.gpu.as_ref().is_some_and(|g| g.1.load(Ordering::Relaxed));
        GPU_ON.store(gpu, Ordering::Relaxed);
        let budget = if gpu { 65536 } else { crate::background().current_num_threads() * 64 };
        if self.vis_on {
            self.request_visible(budget);
            return;
        }
        while self.generating.len() + self.waiting.len() < budget && self.cursor < self.offsets.len() {
            let col = IVec2::new(self.center.x, self.center.z) + self.offsets[self.cursor];
            if self.done.contains(&col) {
                self.cursor += 1;
                continue;
            }
            let mut complete = true;
            let mut list = Vec::new();
            for y in (self.lo..=self.hi).rev() {
                let pos = IVec3::new(col.x, y, col.y);
                if !self.missing(pos) {
                    continue;
                }
                if self.generating.len() + self.waiting.len() + list.len() >= budget {
                    complete = false;
                    break;
                }
                list.push(pos);
            }
            self.ask_column(list);
            if !complete {
                return;
            }
            self.done.insert(col);
            self.cursor += 1;
        }
    }

    /// Neither here, nor coming from the server or the workers.
    fn missing(&self, pos: IVec3) -> bool {
        !(self.cubes.contains_key(&pos) || self.saved.contains(&pos) || self.generating.contains(&pos) || self.waiting.contains_key(&pos))
    }

    /// The visibility fill (`server::visibility`, the one the server used) goes on from what
    /// arrived; the cubes it reached and the ones above them (sky light comes from above) are
    /// asked for, nearest columns first.
    fn request_visible(&mut self, budget: usize) {
        let c = content();
        let (center, r, lo, hi) = (self.center, self.radius, self.lo, self.hi);
        let range = (center, r, r, center.y - lo);
        let in_view = |p: IVec3| {
            let d = p - center;
            d.x * d.x + d.z * d.z <= r * r && p.y >= lo && p.y <= center.y + r
        };
        let edited = std::mem::take(&mut self.edited);
        let mut seen = Seen { cubes: &self.cubes, conn: &mut self.conn };
        let (arrived, left) = self.vis.update(&mut seen, c, range, &in_view, 2, &edited);
        let stale = self.wanted_at.is_none_or(|t| t.elapsed() > Duration::from_millis(20));
        if (!arrived.is_empty() || !left.is_empty() || self.wanted.is_empty() || !edited.is_empty()) && stale {
            self.wanted_at = Some(std::time::Instant::now());
            let mut list: Vec<IVec3> = self.vis.waiting.keys().copied().filter(|&p| in_view(p)).collect();
            // Above the view the columns go up to open sky.
            for o in &self.offsets {
                for y in center.y + r + 1..=hi {
                    list.push(IVec3::new(center.x + o.x, y, center.z + o.y));
                }
            }
            let mut have: FxHashSet<IVec3> = list.iter().copied().collect();
            let mut above = Vec::new();
            for &p in &list {
                let mut q = p + IVec3::Y;
                while q.y <= hi && !self.cubes.contains_key(&q) && have.insert(q) {
                    above.push(q);
                    q += IVec3::Y;
                }
            }
            list.extend(above);
            list.retain(|&p| self.missing(p));
            list.sort_unstable_by_key(|p| std::cmp::Reverse(((p.x - center.x).pow(2) + (p.z - center.z).pow(2)) * 64 - p.y));
            self.wanted = list;
        }
        while self.generating.len() + self.waiting.len() < budget {
            let Some(pos) = self.wanted.pop() else { break };
            if self.missing(pos) {
                self.ask(pos);
            }
        }
    }

    /// Has cube `pos` made: on the GPU when it can, else on a worker.
    fn ask(&mut self, pos: IVec3) {
        self.ask_column(vec![pos]);
    }

    /// Has the cubes `list` of one cube column made, together: what the GPU needs from mods'
    /// code is the same for all of them.
    fn ask_column(&mut self, list: Vec<IVec3>) {
        let c = content();
        let (tx, epoch, dim, seed, edits) = (self.gen_tx.clone(), self.epoch, self.dim, self.seed, self.edits.clone());
        let def = &c.dims[dim as usize];
        let gpu = self.gpu.as_ref().filter(|g| g.1.load(Ordering::Relaxed)).map(|g| g.0.clone());
        let (on_gpu, on_cpu): (Vec<IVec3>, Vec<IVec3>) = list.into_iter().partition(|p| gpu.is_some() && self.gpu_custom.is_some() && p.y >= def.min_cube_y && p.y <= def.max_cube_y);
        self.generating.extend(on_gpu.iter().chain(&on_cpu));
        if let (Some(gpu), Some(custom), Some(&first)) = (gpu, self.gpu_custom.clone(), on_gpu.first()) {
            let (tx, edits) = (tx.clone(), edits.clone());
            let send = move |heights: Arc<Vec<i32>>| {
                for pos in on_gpu {
                    let _ = gpu.send(super::terrain_gpu::Job { epoch, dim, seed, pos, custom: heights.clone(), edits: edits.clone(), done: tx.clone() });
                }
            };
            if custom.is_empty() {
                send(Arc::new(Vec::new()));
            } else {
                // Mods' features run their code: on a worker, not here.
                crate::background().spawn(move || send(super::terrain_gpu::custom_heights(c, dim, seed, first.x, first.z, &custom)));
            }
        }
        for pos in on_cpu {
            let (tx, edits) = (tx.clone(), edits.clone());
            crate::background().spawn(move || {
                let mut cube = crate::server::dim::generate(c, dim, seed, pos);
                area::catch_up(c, &edits, pos, &mut cube);
                crate::world::light::prelight(c, &mut cube, c.dims[dim as usize].sky_light);
                let _ = tx.send((epoch, pos, cube));
            });
        }
    }

    fn made(&mut self, (epoch, pos, mut cube): Made) {
        if epoch != self.epoch || !self.generating.remove(&pos) {
            return;
        }
        if self.server.contains(&pos) || self.saved.contains(&pos) || !self.in_view(pos) {
            return;
        }
        // Edits that came while it was being made.
        let lit = !area::catch_up(content(), &self.edits, pos, &mut cube);
        self.waiting.insert(pos, (cube, lit));
    }

    /// Inserts generated cubes whose cube above is there (or never coming). Each run down a
    /// column is lit on a worker, top down, against what is already here: light from the cubes
    /// around comes in, its own spreads inside. Then, here, what it gives the cubes around
    /// (the ones put in alongside included) spreads out to them.
    fn insert_waiting(&mut self) {
        let tops: Vec<IVec3> = self
            .waiting
            .keys()
            .copied()
            .filter(|&p| {
                let above = p + IVec3::Y;
                !self.waiting.contains_key(&above) && !self.generating.contains(&above)
            })
            .collect();
        let mut runs = Vec::new();
        for top in tops {
            let mut run = Vec::new();
            let mut p = top;
            while let Some((cube, lit)) = self.waiting.remove(&p) {
                if !self.server.contains(&p) && self.in_view(p) {
                    run.push((p, cube, lit));
                }
                p -= IVec3::Y;
            }
            if !run.is_empty() {
                runs.push(run);
            }
        }
        if runs.is_empty() {
            return;
        }
        use rayon::prelude::*;
        let c: &'static Content = content();
        let def = &c.dims[self.dim as usize];
        let (cubes, hi) = (&self.cubes, self.hi);
        let lit: Vec<Vec<(IVec3, Cube)>> = pool().install(|| runs.into_par_iter().map(|run| light_run(c, def, cubes, hi, run)).collect());
        let mut new = Vec::new();
        for (p, cube) in lit.into_iter().flatten() {
            self.cubes.insert(p, Arc::new(cube));
            self.held.insert(p);
            new.push(p);
        }
        let read = Read(&self.cubes);
        let seeds: Vec<(IVec3, u8, u8)> = pool().install(|| {
            new.par_iter()
                .fold(Vec::new, |mut out, &p| {
                    crate::world::light::outward(&read, c, def.sky_light, p, &mut out);
                    out
                })
                .flatten()
                .collect()
        });
        let mut map = Map(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        for ch in 0..4u8 {
            lighter.spread(ch as usize, seeds.iter().filter(|s| s.1 == ch).map(|s| (s.0, s.2)));
        }
        let changed = std::mem::take(&mut lighter.changed);
        crate::diag::TERRAIN_JOINED.add(new.len() as u64);
        // What may go now: these, held ones around them, and handed over ones whose light
        // changed. Looking at the 26 around each is done on every core.
        let this = &*self;
        let around: Vec<IVec3> = pool().install(|| {
            new.par_iter()
                .flat_map_iter(|&p| (0..27).filter(|&i| i != 13).map(move |i| p + IVec3::new(i % 3 - 1, i / 9 - 1, i / 3 % 3 - 1)))
                .filter(|n| this.held.contains(n))
                .collect()
        });
        let mut look: FxHashSet<IVec3> = new.into_iter().chain(around).chain(changed).collect();
        look.retain(|p| self.cubes.contains_key(p));
        let this = &*self;
        let ready: Vec<(IVec3, Option<u32>)> = pool().install(|| look.par_iter().map(|&p| (p, this.readiness(p))).collect());
        for (p, r) in ready {
            match r {
                Some(hidden) => self.hand_over(p, hidden),
                None => {
                    self.held.insert(p);
                }
            }
        }
    }

    /// Joins the light of a cube the server sent (complete already) with its neighbors; the
    /// other cubes whose light changed.
    fn light_sent(&mut self, pos: IVec3) -> FxHashSet<IVec3> {
        let c: &Content = content();
        let mut map = Map(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, c.dims[self.dim as usize].sky_light);
        lighter.light_prelit(pos);
        let mut changed = std::mem::take(&mut lighter.changed);
        changed.remove(&pos);
        changed
    }

    fn emit_all(&mut self, set: FxHashSet<IVec3>) {
        for p in set {
            self.emit(p);
        }
    }

    /// Hands a cube to the client world once the 26 around it are here (or never coming): its
    /// light still changes with each of them that comes, and the client meshes it once. The
    /// ones never coming go with it.
    fn emit(&mut self, pos: IVec3) {
        if !self.cubes.contains_key(&pos) {
            return;
        }
        match self.readiness(pos) {
            Some(hidden) => self.hand_over(pos, hidden),
            None => {
                self.held.insert(pos);
            }
        }
    }

    /// The cubes around `pos` never coming (`cube::around_mask` bits), or `None` while one in
    /// view is still to come and it was not handed over yet.
    fn readiness(&self, pos: IVec3) -> Option<u32> {
        let def = &content().dims[self.dim as usize];
        let mut hidden = 0u32;
        for i in 0..27u32 {
            let n = pos + IVec3::new(i as i32 % 3 - 1, i as i32 / 9 - 1, i as i32 / 3 % 3 - 1);
            if i == 13 || n.y < def.min_cube_y || n.y > def.max_cube_y || self.cubes.contains_key(&n) {
                continue;
            }
            if self.in_view(n) && (!self.vis_on || self.vis.seen.contains(&n)) {
                if self.held.contains(&pos) {
                    return None;
                }
            } else {
                hidden |= 1 << i;
            }
        }
        Some(hidden)
    }

    fn hand_over(&mut self, pos: IVec3, hidden: u32) {
        self.held.remove(&pos);
        self.out.push(Out::Cube { pos, cube: self.cubes[&pos].clone(), hidden });
    }

    /// A cube just put in: handed over if it can be, and so are held ones around it that
    /// waited only for it.
    fn arrived(&mut self, pos: IVec3) {
        self.emit(pos);
        for i in 0..27 {
            let n = pos + IVec3::new(i % 3 - 1, i / 9 - 1, i / 3 % 3 - 1);
            if i != 13 && self.held.contains(&n) {
                self.emit(n);
            }
        }
    }

    /// Held cubes that may go now (what they waited for left the view).
    fn emit_held(&mut self) {
        let around: Vec<IVec3> = self.held.iter().copied().collect();
        for p in around {
            self.emit(p);
        }
    }
}
