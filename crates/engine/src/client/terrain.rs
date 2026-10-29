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
use crate::world::light::{CubeMap, Lighter, SKY_FULL};

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
    Cube { pos: IVec3, cube: Arc<Cube>, hidden: u8 },
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

pub fn spawn() -> TerrainHandle {
    let (tx, in_rx) = unbounded();
    let (out_tx, rx) = unbounded();
    let pending = Arc::new(AtomicU64::new(0));
    let p = pending.clone();
    std::thread::Builder::new()
        .name("terrain".into())
        .spawn(move || {
            let (gen_tx, gen_rx) = unbounded();
            let mut t = Terrain::new(p, gen_tx);
            t.run(in_rx, gen_rx, out_tx);
        })
        .expect("terrain thread");
    TerrainHandle { tx, rx, pending }
}

struct Map<'a>(&'a mut FxHashMap<IVec3, Arc<Cube>>);

impl CubeMap for Map<'_> {
    fn cube(&self, c: IVec3) -> Option<&Cube> {
        self.0.get(&c).map(|a| &**a)
    }
    fn cube_mut(&mut self, c: IVec3) -> Option<&mut Cube> {
        self.0.get_mut(&c).map(Arc::make_mut)
    }
}

/// A generated cube: the round it was asked in (`Terrain::epoch`), where, what.
type Made = (u64, IVec3, Cube);

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
            let left = self.generating.len() + self.waiting.len() + (self.cursor < self.offsets.len()) as usize;
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
                *self = Terrain { epoch: self.epoch + 1, offsets: std::mem::take(&mut self.offsets), ..Terrain::new(self.pending.clone(), self.gen_tx.clone()) };
                (self.dim, self.seed, self.active) = (dim, seed, true);
                self.radius = 0;
            }
            In::View { center, radius } => self.set_view(center, radius),
            In::Cube { pos, cube } => {
                self.server.insert(pos);
                self.waiting.remove(&pos);
                self.cubes.insert(pos, cube);
                // Its light is the server's; light from cubes made here joins it.
                let changed = self.light(pos, true, true);
                self.emit(pos);
                self.emit_all(changed);
            }
            In::Blocks(changes) => {
                for &(p, s) in &changes {
                    if let Some(cube) = self.cubes.get_mut(&crate::world::cube_of(p)) {
                        Arc::make_mut(cube).set(crate::world::local_index(p), s);
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
            self.out.push(Out::Unload(p));
        }
        self.saved.retain(|&p| {
            let d = p - center;
            d.x * d.x + d.z * d.z <= (radius + 2) * (radius + 2)
        });
        self.done.retain(|col| (*col - IVec2::new(center.x, center.z)).length_squared() <= radius * radius);
    }

    /// Asks the workers for the nearest missing cubes, each column from the top down.
    fn request(&mut self) {
        if !self.active || self.radius == 0 {
            return;
        }
        let budget = crate::background().current_num_threads() * 64;
        let c = content();
        while self.generating.len() + self.waiting.len() < budget && self.cursor < self.offsets.len() {
            let col = IVec2::new(self.center.x, self.center.z) + self.offsets[self.cursor];
            if self.done.contains(&col) {
                self.cursor += 1;
                continue;
            }
            let mut complete = true;
            for y in (self.lo..=self.hi).rev() {
                let pos = IVec3::new(col.x, y, col.y);
                if self.cubes.contains_key(&pos) || self.saved.contains(&pos) || self.generating.contains(&pos) || self.waiting.contains_key(&pos) {
                    continue;
                }
                if self.generating.len() + self.waiting.len() >= budget {
                    complete = false;
                    break;
                }
                self.generating.insert(pos);
                let (tx, epoch, dim, seed, edits) = (self.gen_tx.clone(), self.epoch, self.dim, self.seed, self.edits.clone());
                crate::background().spawn(move || {
                    let mut cube = crate::server::dim::generate(c, dim, seed, pos);
                    area::catch_up(c, &edits, pos, &mut cube);
                    crate::world::light::prelight(c, &mut cube, c.dims[dim as usize].sky_light);
                    let _ = tx.send((epoch, pos, cube));
                });
            }
            if !complete {
                return;
            }
            self.done.insert(col);
            self.cursor += 1;
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

    /// Inserts generated cubes whose cube above is there (or never coming), top down.
    fn insert_waiting(&mut self) {
        let mut list: Vec<IVec3> = self.waiting.keys().copied().collect();
        list.sort_unstable_by_key(|p| (-p.y, p.x, p.z));
        for pos in list {
            let above = pos + IVec3::Y;
            if self.waiting.contains_key(&above) || self.generating.contains(&above) {
                continue;
            }
            let (cube, lit) = self.waiting.remove(&pos).unwrap();
            if self.server.contains(&pos) || !self.in_view(pos) {
                continue;
            }
            self.cubes.insert(pos, Arc::new(cube));
            let changed = self.light(pos, false, lit);
            self.emit(pos);
            self.emit_all(changed);
        }
    }

    /// Joins the light of a cube just put in with its neighbors; the other cubes whose light
    /// changed. `whole`: its light is complete already (from the server); `lit`: it was lit on
    /// its own as if open sky were above.
    fn light(&mut self, pos: IVec3, whole: bool, lit: bool) -> FxHashSet<IVec3> {
        let c: &Content = content();
        let def = &c.dims[self.dim as usize];
        let open_above = pos.y + 1 >= def.sky_cube_y || pos.y + 1 > def.max_cube_y || pos.y >= self.hi;
        let full = SKY_FULL as u32;
        let full_above = |a: &Cube| match a.uniform_light() {
            Some(l) => l & 0xFF == full,
            None => (0..256).all(|i| a.light(crate::world::cube::index(i % 16, 0, i / 16)) & 0xFF == full),
        };
        let cube = &self.cubes[&pos];
        // Prelit as if open sky were above: right when the cube above lets full sky through.
        let clear = cube.palette().iter().all(|&s| c.states[s as usize].opacity == 0);
        let prelit = whole
            || (lit
                && !clear
                && (!def.sky_light
                    || match self.cubes.get(&(pos + IVec3::Y)) {
                        Some(a) => full_above(a),
                        None => open_above,
                    }));
        let mut map = Map(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        if prelit {
            lighter.light_prelit(pos);
        } else {
            lighter.light_new(pos, open_above);
        }
        let mut changed = std::mem::take(&mut lighter.changed);
        changed.remove(&pos);
        changed
    }

    fn emit_all(&mut self, set: FxHashSet<IVec3>) {
        for p in set {
            self.emit(p);
        }
    }

    /// Hands a cube to the client world: faces toward cubes that will never be here are shut.
    fn emit(&mut self, pos: IVec3) {
        let Some(cube) = self.cubes.get(&pos) else { return };
        let def = &content().dims[self.dim as usize];
        let hidden = crate::world::DIRS.iter().enumerate().fold(0u8, |m, (i, d)| {
            let n = pos + *d;
            let outside_dim = n.y < def.min_cube_y || n.y > def.max_cube_y;
            if outside_dim || self.cubes.contains_key(&n) || self.in_view(n) { m } else { m | 1 << i }
        });
        self.out.push(Out::Cube { pos, cube: cube.clone(), hidden });
    }
}
