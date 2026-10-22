use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::Arc;

use glam::{DVec3, IVec3};
use rustc_hash::{FxHashMap, FxHashSet};
use voxide_abi::{self as abi, block_flags as bf};

use crate::content::{AIR, Content};
use crate::host::{self, CallCtx, WorldAccess};
use crate::world::cube::VOLUME;
use crate::world::light::{CubeMap, Lighter};
use crate::world::{Cube, DIRS, Rng, cube_of, local_index};

const MAX_NEIGHBOR_UPDATES: usize = 1 << 16;
const RANDOM_TICKS_PER_CUBE: u32 = 3;

pub struct Dim {
    pub id: u32,
    pub cubes: FxHashMap<IVec3, Arc<Cube>>,
    pub generating: FxHashSet<IVec3>,
    pub time: u64,
    rng: Rng,
    scheduled: BinaryHeap<Reverse<(u64, i32, i32, i32)>>,
    neighbor_queue: VecDeque<(IVec3, IVec3)>,
    /// Block changes this tick, sent as deltas.
    pub changes: Vec<(IVec3, u32)>,
    /// Cubes whose light changed this tick, sent whole.
    pub resend: Vec<(IVec3, Arc<Cube>)>,
    light_dirty: FxHashSet<IVec3>,
}

struct Cubes<'a>(&'a mut FxHashMap<IVec3, Arc<Cube>>);

impl CubeMap for Cubes<'_> {
    fn cube(&self, c: IVec3) -> Option<&Cube> {
        self.0.get(&c).map(|a| &**a)
    }
    fn cube_mut(&mut self, c: IVec3) -> Option<&mut Cube> {
        self.0.get_mut(&c).map(Arc::make_mut)
    }
}

pub fn generate(c: &Content, dim: u32, seed: u64, pos: IVec3) -> Cube {
    let def = &c.dims[dim as usize];
    if pos.y < def.min_cube_y || pos.y > def.max_cube_y {
        return Cube::uniform(AIR, 0);
    }
    let Some(gen_cb) = def.generator else { return Cube::uniform(AIR, 0) };
    let mut blocks = vec![0u32; VOLUME];
    let mut biomes = [0u8; 256];
    let mut args = abi::GenArgs {
        seed,
        cube: pos.to_array(),
        blocks: blocks.as_mut_ptr(),
        biomes: biomes.as_mut_ptr(),
    };
    host::invoke(def.owner, gen_cb, &mut args);
    let max = c.states.len() as u32;
    for b in blocks.iter_mut() {
        if *b >= max {
            *b = AIR;
        }
    }
    let mut cube = Cube::from_states(blocks.as_slice().try_into().unwrap());
    if biomes.iter().any(|&b| b != 0) {
        cube.biomes = Some(Box::new(biomes));
    }
    cube
}

impl Dim {
    pub fn new(id: u32, seed: u64) -> Dim {
        Dim {
            id,
            cubes: FxHashMap::default(),
            generating: FxHashSet::default(),
            time: 1000,
            rng: Rng(seed ^ (id as u64).wrapping_mul(0x9E37_79B9)),
            scheduled: BinaryHeap::new(),
            neighbor_queue: VecDeque::new(),
            changes: Vec::new(),
            resend: Vec::new(),
            light_dirty: FxHashSet::default(),
        }
    }

    pub fn has(&self, pos: IVec3) -> bool {
        self.cubes.contains_key(&pos)
    }

    pub fn snapshot(&self, pos: IVec3) -> Option<Arc<Cube>> {
        self.cubes.get(&pos).cloned()
    }

    pub fn insert(&mut self, c: &Content, pos: IVec3, cube: Cube) {
        self.generating.remove(&pos);
        if self.cubes.contains_key(&pos) {
            return;
        }
        self.cubes.insert(pos, Arc::new(cube));
        let def = &c.dims[self.id as usize];
        let open_above = pos.y + 1 >= def.sky_cube_y || pos.y + 1 > def.max_cube_y;
        let mut map = Cubes(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        lighter.light_new(pos, open_above);
        let changed = std::mem::take(&mut lighter.changed);
        changed.into_iter().filter(|&p| p != pos).for_each(|p| {
            self.light_dirty.insert(p);
        });
    }

    pub fn retain_cubes(&mut self, _c: &Content, keep: impl Fn(IVec3) -> bool) {
        self.cubes.retain(|&pos, _| keep(pos));
    }

    pub fn get(&self, p: IVec3) -> Option<u32> {
        self.cubes.get(&cube_of(p)).map(|cube| cube.get(local_index(p)))
    }

    /// Writes a block, relights, records the change for clients and queues neighbour updates.
    pub fn set(&mut self, c: &Content, p: IVec3, state: u32) -> Option<u32> {
        let cp = cube_of(p);
        let cube = Arc::make_mut(self.cubes.get_mut(&cp)?);
        let old = cube.set(local_index(p), state);
        if old == state {
            return Some(old);
        }
        self.changes.push((p, state));
        let def = &c.dims[self.id as usize];
        let mut map = Cubes(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        lighter.block_changed(p, old);
        let changed = std::mem::take(&mut lighter.changed);
        self.light_dirty.extend(changed);
        for d in DIRS {
            self.neighbor_queue.push_back((p + d, p));
        }
        Some(old)
    }

    pub fn spawn_point(&self, c: &Content) -> DVec3 {
        let def = &c.dims[self.id as usize];
        let mut pos = [0, 64, 0];
        if let Some(cb) = def.spawn {
            let mut args = abi::SpawnArgs { seed: self.rng.0, pos };
            host::invoke(def.owner, cb, &mut args);
            pos = args.pos;
        }
        DVec3::new(pos[0] as f64 + 0.5, pos[1] as f64 + 1.0, pos[2] as f64 + 0.5)
    }

    fn call_block<A>(&mut self, c: &'static Content, state: u32, pick: impl Fn(&crate::content::BlockCallbacks) -> Option<abi::Callback>, args: impl FnOnce(*mut abi::WorldCtx) -> A) -> Option<A> {
        let block = c.block_of(state);
        let cb = pick(&block.cb)?;
        let owner = block.owner;
        let mut access = Access { dim: self, c };
        let mut ctx = CallCtx { world: &mut access };
        let mut a = args(ctx.raw());
        host::invoke(owner, cb, &mut a);
        Some(a)
    }

    pub fn tick(&mut self, c: &'static Content) {
        self.time += 1;

        while let Some(&Reverse((at, x, y, z))) = self.scheduled.peek() {
            if at > self.time {
                break;
            }
            self.scheduled.pop();
            let p = IVec3::new(x, y, z);
            if let Some(state) = self.get(p) {
                self.call_block(c, state, |b| b.on_scheduled, |w| abi::BlockEventArgs {
                    world: w,
                    pos: p.to_array(),
                    state,
                    other: [0; 3],
                    entity: 0,
                });
            }
        }

        // Sorted so the random stream is consumed in the same order on every run.
        let mut ticking: Vec<IVec3> = self
            .cubes
            .iter()
            .filter(|(_, cube)| cube.palette().iter().any(|&s| c.states[s as usize].flags & bf::RANDOM_TICKS != 0))
            .map(|(&p, _)| p)
            .collect();
        ticking.sort_by_key(|p| (p.x, p.y, p.z));
        for cp in ticking {
            for _ in 0..RANDOM_TICKS_PER_CUBE {
                let r = self.rng.next();
                let i = (r & 4095) as usize;
                let Some(cube) = self.cubes.get(&cp) else { break };
                let state = cube.get(i);
                if c.states[state as usize].flags & bf::RANDOM_TICKS == 0 {
                    continue;
                }
                let p = cp * 16 + IVec3::new((i & 15) as i32, (i >> 8) as i32, ((i >> 4) & 15) as i32);
                self.call_block(c, state, |b| b.on_random_tick, |w| abi::BlockEventArgs {
                    world: w,
                    pos: p.to_array(),
                    state,
                    other: [0; 3],
                    entity: 0,
                });
            }
        }
        self.flush_neighbors(c);
        self.finish_tick();
    }

    pub fn flush_neighbors(&mut self, c: &'static Content) {
        let mut budget = MAX_NEIGHBOR_UPDATES;
        while let Some((p, from)) = self.neighbor_queue.pop_front() {
            if budget == 0 {
                log::warn!("neighbour update storm in dimension {}, dropping {} updates", self.id, self.neighbor_queue.len());
                self.neighbor_queue.clear();
                break;
            }
            budget -= 1;
            let Some(state) = self.get(p) else { continue };
            self.call_block(c, state, |b| b.on_neighbor, |w| abi::BlockEventArgs {
                world: w,
                pos: p.to_array(),
                state,
                other: from.to_array(),
                entity: 0,
            });
        }
    }

    fn finish_tick(&mut self) {
        for cp in std::mem::take(&mut self.light_dirty) {
            if let Some(cube) = self.cubes.get(&cp) {
                self.resend.push((cp, cube.clone()));
            }
        }
        if !self.resend.is_empty() {
            let resent: FxHashSet<IVec3> = self.resend.iter().map(|r| r.0).collect();
            self.changes.retain(|(p, _)| !resent.contains(&cube_of(*p)));
        }
    }

    pub fn player_break(&mut self, c: &'static Content, p: IVec3, player: u64) {
        let Some(state) = self.get(p) else { return };
        if state == AIR || c.states[state as usize].hardness < 0.0 {
            return;
        }
        self.set(c, p, AIR);
        self.call_block(c, state, |b| b.on_broken, |w| abi::BlockEventArgs {
            world: w,
            pos: p.to_array(),
            state,
            other: [0; 3],
            entity: player,
        });
        self.flush_neighbors(c);
        self.finish_tick();
    }

    pub fn player_place(&mut self, c: &'static Content, p: IVec3, face: u8, state: u32, look: [f32; 3], player: u64) {
        let Some(cur) = self.get(p) else { return };
        if c.states[cur as usize].flags & bf::REPLACEABLE == 0 {
            return;
        }
        let mut chosen = state;
        let first = c.block_of(state).first_state;
        if let Some(cb) = c.block_of(state).cb.placement {
            let mut args = abi::PlaceArgs { pos: p.to_array(), face: face as u32, look, state: first };
            host::invoke(c.block_of(state).owner, cb, &mut args);
            if args.state >= first && args.state < first + c.block_of(state).states {
                chosen = args.state;
            }
        }
        self.set(c, p, chosen);
        self.call_block(c, chosen, |b| b.on_placed, |w| abi::BlockEventArgs {
            world: w,
            pos: p.to_array(),
            state: chosen,
            other: [0; 3],
            entity: player,
        });
        self.flush_neighbors(c);
        self.finish_tick();
    }

    /// True if the block consumed the click.
    pub fn player_use(&mut self, c: &'static Content, p: IVec3, face: u8, player: u64) -> bool {
        let Some(state) = self.get(p) else { return false };
        let r = self.call_block(c, state, |b| b.on_use, |w| abi::UseArgs {
            world: w,
            pos: p.to_array(),
            state,
            face: face as u32,
            player,
            handled: 0,
        });
        self.flush_neighbors(c);
        self.finish_tick();
        r.is_some_and(|a| a.handled != 0)
    }
}

struct Access<'a> {
    dim: &'a mut Dim,
    c: &'static Content,
}

impl WorldAccess for Access<'_> {
    fn get(&mut self, pos: [i32; 3]) -> u32 {
        self.dim.get(IVec3::from_array(pos)).unwrap_or(AIR)
    }

    fn set(&mut self, pos: [i32; 3], state: u32) -> bool {
        self.dim.set(self.c, IVec3::from_array(pos), state).is_some()
    }

    fn schedule(&mut self, pos: [i32; 3], delay: u32) {
        let at = self.dim.time + delay.max(1) as u64;
        self.dim.scheduled.push(Reverse((at, pos[0], pos[1], pos[2])));
    }

    fn time(&mut self) -> u64 {
        self.dim.time
    }

    fn random(&mut self) -> u64 {
        self.dim.rng.next()
    }

    fn drop_item(&mut self, pos: [f32; 3], item: u32, count: u32, _damage: u32) {
        log::debug!("drop {count}x item {item} at {pos:?}");
    }
}
