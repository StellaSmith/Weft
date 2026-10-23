use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::{DVec3, IVec3};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use voxide_abi::{self as abi, ItemStack, block_flags as bf, damage, entity_flags as ef};

use super::entity::{Entities, Entity, step};
use crate::content::{AIR, Content};
use crate::host::{self, CallCtx};
use crate::world::cube::VOLUME;
use crate::world::light::{CubeMap, Lighter};
use crate::world::{Cube, DIRS, Rng, cube_of, local_index};

const MAX_NEIGHBOR_UPDATES: usize = 1 << 16;
const RANDOM_TICKS_PER_CUBE: u32 = 3;
/// Random ticks only happen this many cubes around a player (like vanilla's ticking distance):
/// far terrain has nobody to watch grass spread, and scanning every loaded cube each tick costs
/// more than all the ticks themselves.
const RANDOM_TICK_RADIUS: i32 = 8;
const ITEM_LIFETIME: u64 = 20 * 60 * 5;

pub enum DimEvent {
    OpenMenu { player: u64, menu: u32, owner: u64 },
    Died { player: u64 },
}

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
    /// Cubes changed since the last save. Untouched cubes are never written (regenerating is cheaper).
    pub modified: FxHashSet<IVec3>,
    pub entities: Entities,
    pub events: Vec<DimEvent>,
    next_id: Arc<AtomicU64>,
    pub grid: super::energy::Grid,
    /// Cubes whose sky light is final: open sky above, or the cube above is settled. Only these
    /// are sent, so clients never see light being filled in afterwards.
    pub settled: FxHashSet<IVec3>,
    /// Bumped whenever a cube becomes settled, so senders know when to look again.
    pub settled_gen: u64,
    /// Bumped by block changes, which may open or close lines of sight.
    pub edits: u64,
    /// Face connectivity of loaded cubes (see `Cube::connectivity`); missing = recompute.
    pub conn: FxHashMap<IVec3, u16>,
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
    let mut args = abi::GenArgs { seed, cube: pos.to_array(), blocks: blocks.as_mut_ptr(), biomes: biomes.as_mut_ptr() };
    host::invoke(def.owner, gen_cb, &mut args);
    let max = c.states.len() as u32;
    for b in blocks.iter_mut() {
        if *b >= max {
            *b = AIR;
        }
    }
    place_ores(c, def, seed, pos, &mut blocks);
    let mut cube = Cube::from_states(blocks.as_slice().try_into().unwrap());
    if biomes.iter().any(|&b| b != 0) {
        cube.biomes = Some(Box::new(biomes));
    }
    cube
}

fn hash(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    let mut h = seed ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// Veins start in this and the 26 neighbouring cubes and wander a block per step, so the result
/// does not depend on which cube is generated first.
fn place_ores(c: &Content, def: &crate::content::DimensionDef, seed: u64, pos: IVec3, blocks: &mut [u32]) {
    let o = pos * 16;
    for ore in &def.ores {
        let reach = ore.size as i32;
        if o.y > ore.max_y.saturating_add(reach) || o.y + 15 < ore.min_y.saturating_sub(reach) {
            continue;
        }
        for nz in pos.z - 1..=pos.z + 1 {
            for ny in pos.y - 1..=pos.y + 1 {
                for nx in pos.x - 1..=pos.x + 1 {
                    for v in 0..ore.veins {
                        let h = hash(seed ^ ore.salt, nx as i64, ny as i64 * 31 + v as i64, nz as i64);
                        let mut p = IVec3::new(nx * 16 + (h % 16) as i32, ny * 16 + ((h >> 4) % 16) as i32, nz * 16 + ((h >> 8) % 16) as i32);
                        if p.y > ore.max_y || p.y < ore.min_y {
                            continue;
                        }
                        for k in 0..ore.size {
                            let hk = hash(h, k as i64, 0, 0);
                            p += IVec3::new((hk % 3) as i32 - 1, ((hk >> 2) % 3) as i32 - 1, ((hk >> 4) % 3) as i32 - 1);
                            let l = p - o;
                            if l.cmpge(IVec3::ZERO).all() && l.cmplt(IVec3::splat(16)).all() {
                                let i = (l.y * 256 + l.z * 16 + l.x) as usize;
                                if ore.replace.binary_search(&c.states[blocks[i] as usize].block).is_ok() {
                                    blocks[i] = ore.state;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Face connectivity through blocks that do not hide what is behind them.
pub fn see_through(c: &Content, cube: &Cube) -> u16 {
    cube.connectivity(|s| c.states[s as usize].flags & bf::OPAQUE == 0)
}

/// Seconds to break `state` holding `tool` (item id, 0 = hand), and whether it drops anything.
pub fn break_time(c: &Content, state: u32, tool: u32) -> (f32, bool) {
    let s = &c.states[state as usize];
    let b = c.block_of(state);
    if s.hardness < 0.0 {
        return (f32::INFINITY, false);
    }
    let item = c.items.get(tool as usize);
    let matches = !b.tool.is_empty() && item.is_some_and(|i| i.tool == b.tool);
    let level = if matches { item.map_or(0, |i| i.tool_level + 1) } else { 0 };
    // harvest_level N > 0 needs a matching tool of level N-1 or better.
    let harvest = b.harvest_level == 0 || level >= b.harvest_level;
    let speed = if matches { item.map_or(1.0, |i| i.tool_speed) } else { 1.0 };
    let base = s.hardness * if harvest { 1.5 } else { 5.0 };
    (base / speed, harvest)
}

impl Dim {
    pub fn new(id: u32, seed: u64, next_id: Arc<AtomicU64>) -> Dim {
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
            modified: FxHashSet::default(),
            entities: Entities::default(),
            events: Vec::new(),
            next_id,
            grid: Default::default(),
            settled: FxHashSet::default(),
            settled_gen: 0,
            edits: 0,
            conn: FxHashMap::default(),
        }
    }

    pub fn new_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn has(&self, pos: IVec3) -> bool {
        self.cubes.contains_key(&pos)
    }

    pub fn snapshot(&self, pos: IVec3) -> Option<Arc<Cube>> {
        self.cubes.get(&pos).cloned()
    }

    /// `prelit`: the cube carries light from `light::prelight`.
    pub fn insert(&mut self, c: &Content, pos: IVec3, cube: Cube, prelit: bool) {
        self.generating.remove(&pos);
        if self.cubes.contains_key(&pos) {
            return;
        }
        self.grid.cube_loaded(c, pos, &cube);
        let def = &c.dims[self.id as usize];
        let open_above = pos.y + 1 >= def.sky_cube_y || pos.y + 1 > def.max_cube_y;
        // The worker's light assumed open sky above; true when the cube above lets full sky
        // through its whole bottom (or there is nothing above).
        let full_above = |above: &Cube| match above.uniform_light() {
            Some(l) => l & 0xFF == crate::world::light::SKY_FULL as u32,
            None => (0..256).all(|i| above.light(crate::world::cube::index(i % 16, 0, i / 16)) & 0xFF == crate::world::light::SKY_FULL as u32),
        };
        // Clear cubes have a cheaper path of their own in `light_new`.
        let clear = cube.palette().iter().all(|&s| c.states[s as usize].opacity == 0);
        let prelit = prelit
            && !clear
            && (!def.sky_light
                || match self.cubes.get(&(pos + IVec3::Y)) {
                    Some(a) => full_above(a),
                    None => open_above,
                });
        self.cubes.insert(pos, Arc::new(cube));
        let mut map = Cubes(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        if prelit {
            lighter.light_prelit(pos);
        } else {
            lighter.light_new(pos, open_above);
        }
        let changed = std::mem::take(&mut lighter.changed);
        changed.into_iter().filter(|&p| p != pos).for_each(|p| {
            self.light_dirty.insert(p);
        });
        if open_above || !def.sky_light || self.settled.contains(&(pos + IVec3::Y)) {
            let mut p = pos;
            while self.cubes.contains_key(&p) && self.settled.insert(p) {
                self.settled_gen += 1;
                p -= IVec3::Y;
            }
        }
    }

    /// Drops cubes `keep` rejects; returns the ones that must be saved (modified or holding
    /// entities) together with the entities that lived in them, which leave the world.
    pub fn retain_cubes(&mut self, keep: impl Fn(IVec3) -> bool) -> Vec<(IVec3, Arc<Cube>, Vec<u64>)> {
        let mut out = Vec::new();
        let gone: Vec<IVec3> = self.cubes.keys().copied().filter(|&p| !keep(p)).collect();
        for pos in gone {
            let inside: Vec<u64> = self
                .entities
                .list
                .iter()
                .filter(|e| !e.removed && !e.player && cube_of(e.pos.floor().as_ivec3()) == pos)
                .map(|e| e.id)
                .collect();
            let cube = self.cubes.remove(&pos).unwrap();
            self.grid.cube_unloaded(pos);
            self.settled.remove(&pos);
            self.conn.remove(&pos);
            if self.modified.remove(&pos) || !inside.is_empty() {
                out.push((pos, cube, inside));
            }
        }
        out
    }

    pub fn get(&self, p: IVec3) -> Option<u32> {
        self.cubes.get(&cube_of(p)).map(|cube| cube.get(local_index(p)))
    }

    pub fn light(&self, p: IVec3) -> u32 {
        self.cubes.get(&cube_of(p)).map_or(0, |cube| cube.light(local_index(p)))
    }

    /// 0..1 strength of the sun.
    pub fn daylight(&self, c: &Content) -> f32 {
        let def = &c.dims[self.id as usize];
        if !def.sky_light {
            return 0.0;
        }
        let (t, len) = if def.day_length == 0 {
            (def.fixed_time as f64, 24000.0)
        } else {
            ((self.time % def.day_length as u64) as f64, def.day_length as f64)
        };
        ((t / len * std::f64::consts::TAU).sin() * 2.0 + 0.6).clamp(0.0, 1.0) as f32
    }

    /// Writes a block, relights, records the change for clients, queues neighbour updates and keeps
    /// block entities in sync with their block.
    pub fn set(&mut self, c: &'static Content, p: IVec3, state: u32) -> Option<u32> {
        let cp = cube_of(p);
        let cube = Arc::make_mut(self.cubes.get_mut(&cp)?);
        let old = cube.set(local_index(p), state);
        if old == state {
            return Some(old);
        }
        self.changes.push((p, state));
        self.modified.insert(cp);
        self.conn.remove(&cp);
        self.edits += 1;
        let def = &c.dims[self.id as usize];
        let mut map = Cubes(&mut self.cubes);
        let mut lighter = Lighter::new(&mut map, c, def.sky_light);
        lighter.block_changed(p, old);
        let changed = std::mem::take(&mut lighter.changed);
        self.light_dirty.extend(changed);
        for d in DIRS {
            self.neighbor_queue.push_back((p + d, p));
        }
        self.grid.block_changed(c, p, old, state);
        let (ob, nb) = (c.states[old as usize].block, c.states[state as usize].block);
        if ob != nb {
            if let Some(&id) = self.entities.anchors.get(&p) {
                if let Some(items) = self.entities.inventories.get(&id).cloned() {
                    for s in items.into_iter().filter(|s| !s.is_empty()) {
                        self.spawn_item(c, p.as_dvec3() + DVec3::splat(0.5), s);
                    }
                }
                self.entities.remove(id);
            }
            if let Some(&kind) = c.game.block_entities.get(&nb) {
                let id = self.new_id();
                let mut e = Entity::new(id, kind, p.as_dvec3() + DVec3::splat(0.5), 1.0);
                e.anchor = Some(p);
                self.entities.insert(c, e);
            }
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

    pub fn spawn(&mut self, c: &Content, kind: u32, pos: DVec3) -> u64 {
        let id = self.new_id();
        let k = &c.game.entities[kind as usize];
        let mut e = Entity::new(id, kind, pos, k.max_health);
        e.yaw = (self.rng.float() * std::f32::consts::TAU) - std::f32::consts::PI;
        self.entities.insert(c, e);
        id
    }

    pub fn spawn_item(&mut self, c: &Content, pos: DVec3, stack: ItemStack) -> u64 {
        if stack.is_empty() {
            return 0;
        }
        let id = self.new_id();
        let mut e = Entity::new(id, c.game.item_kind, pos, 1.0);
        e.item = stack;
        e.pickup_delay = 10;
        let (a, b) = (self.rng.float() - 0.5, self.rng.float() - 0.5);
        e.vel = DVec3::new(a as f64 * 2.0, 3.0, b as f64 * 2.0);
        self.entities.insert(c, e);
        id
    }

    /// Calls a mod callback with this dimension as its world.
    pub fn call<A>(&mut self, owner: abi::ModId, cb: abi::Callback, make: impl FnOnce(*mut abi::WorldCtx) -> A) -> A {
        let mut ctx = CallCtx { dim: self as *mut Dim };
        let mut a = make(ctx.raw());
        host::invoke(owner, cb, &mut a);
        a
    }

    fn call_block<A>(&mut self, c: &'static Content, state: u32, pick: impl Fn(&crate::content::BlockCallbacks) -> Option<abi::Callback>, args: impl FnOnce(*mut abi::WorldCtx) -> A) -> Option<A> {
        let block = c.block_of(state);
        let cb = pick(&block.cb)?;
        Some(self.call(block.owner, cb, args))
    }

    pub fn damage(&mut self, c: &'static Content, id: u64, amount: f32, source: u64, cause: u32) -> bool {
        let Some(e) = self.entities.get(id) else { return false };
        let k = &c.game.entities[e.kind as usize];
        if ((k.is(ef::INVULNERABLE) || e.mode.invulnerable()) && cause != damage::VOID) || e.hurt_cooldown > 0 || e.health <= 0.0 {
            return false;
        }
        let mut amount = amount;
        for &(owner, cb) in &k.on_hurt {
            let a = self.call(owner, cb, |w| abi::EntityEventArgs { world: w, entity: id, other: source, amount, cause, handled: 0 });
            amount = a.amount;
        }
        if amount <= 0.0 {
            return false;
        }
        let knock = self.entities.get(source).map(|s| s.pos);
        let Some(e) = self.entities.get_mut(id) else { return false };
        e.health -= amount;
        e.hurt_cooldown = 10;
        if let Some(from) = knock {
            let d = (e.pos - from).with_y(0.0).normalize_or_zero();
            e.vel += d * 6.0 + DVec3::Y * 5.0;
        }
        let (health, player) = (e.health, e.player);
        self.entities.hurt.push((id, health));
        if health <= 0.0 {
            for &(owner, cb) in &k.on_death {
                self.call(owner, cb, |w| abi::EntityEventArgs { world: w, entity: id, other: source, amount, cause, handled: 0 });
            }
            if player {
                if let Some(items) = self.entities.inventories.get(&id).cloned() {
                    let pos = self.entities.get(id).map_or(DVec3::ZERO, |e| e.pos);
                    for (i, s) in items.into_iter().enumerate() {
                        if !s.is_empty() {
                            self.spawn_item(c, pos + DVec3::Y, s);
                            self.entities.set_slot(id, i as u32, ItemStack::EMPTY);
                        }
                    }
                }
                self.events.push(DimEvent::Died { player: id });
            } else {
                self.entities.remove(id);
            }
        }
        true
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
                self.call_block(c, state, |b| b.on_scheduled, |w| abi::BlockEventArgs { world: w, pos: p.to_array(), state, other: [0; 3], entity: 0 });
            }
        }

        // Sorted so the random stream is consumed in the same order on every run.
        let mut ticking: Vec<IVec3> = Vec::new();
        for e in self.entities.list.iter().filter(|e| e.player && !e.removed) {
            let center = cube_of(e.pos.floor().as_ivec3());
            let r = RANDOM_TICK_RADIUS;
            for dy in -r..=r {
                for dz in -r..=r {
                    for dx in -r..=r {
                        let p = center + IVec3::new(dx, dy, dz);
                        let ticks = self.cubes.get(&p).is_some_and(|cube| cube.palette().iter().any(|&s| c.states[s as usize].flags & bf::RANDOM_TICKS != 0));
                        if ticks {
                            ticking.push(p);
                        }
                    }
                }
            }
        }
        ticking.sort_by_key(|p| (p.x, p.y, p.z));
        ticking.dedup();
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
                self.call_block(c, state, |b| b.on_random_tick, |w| abi::BlockEventArgs { world: w, pos: p.to_array(), state, other: [0; 3], entity: 0 });
            }
        }

        self.tick_entities(c);
        self.grid.tick(&mut self.entities, c);
        self.flush_neighbors(c);
        self.entities.compact();
        self.finish_tick();
    }

    fn tick_entities(&mut self, c: &'static Content) {
        // Mod logic: one call per kind and callback with every entity of that kind.
        let mut by_kind: FxHashMap<u32, Vec<u64>> = FxHashMap::default();
        for e in &self.entities.list {
            if !e.removed && (!e.player || e.health > 0.0) {
                by_kind.entry(e.kind).or_default().push(e.id);
            }
        }
        let mut kinds: Vec<u32> = by_kind.keys().copied().collect();
        kinds.sort();
        for kind in kinds {
            let ids = &by_kind[&kind];
            for &(owner, cb) in &c.game.entities[kind as usize].on_tick {
                let ids_ptr = ids.as_ptr();
                let count = ids.len();
                self.call(owner, cb, |w| abi::EntityTickArgs { world: w, kind, ids: ids_ptr, count });
            }
        }

        let gravity = c.dims[self.id as usize].gravity as f64;
        let day = self.daylight(c);
        let players: Vec<DVec3> = self.entities.list.iter().filter(|e| e.player && !e.removed).map(|e| e.pos).collect();
        let cubes = &self.cubes;
        let block = |p: IVec3| cubes.get(&cube_of(p)).map(|cb| cb.get(local_index(p)));
        // Physics only reads blocks and writes its own entity, so entities move in parallel;
        // damage is applied afterwards in list order to stay deterministic.
        let hits: Vec<(u64, Option<f32>, bool)> = self
            .entities
            .list
            .par_iter_mut()
            .with_min_len(64)
            .filter_map(|e| {
                if e.removed {
                    return None;
                }
                e.age += 1;
                e.hurt_cooldown = e.hurt_cooldown.saturating_sub(1);
                if e.player {
                    return None;
                }
                let k = &c.game.entities[e.kind as usize];
                let mut land = None;
                if let Some(fall) = step(c, e, gravity, &block) {
                    if k.is(ef::FALL_DAMAGE) && fall > 3.0 {
                        land = Some(fall - 3.0);
                    }
                }
                // Sunlight sets them on fire for 8 seconds; burning hurts once a second, water puts it out.
                if k.is(ef::BURNS_IN_SUN) && day > 0.5 && e.age % 20 == 0 && !e.in_liquid {
                    let head = (e.pos + DVec3::Y * k.eye as f64).floor().as_ivec3();
                    let sky = cubes.get(&cube_of(head)).map_or(0, |cb| cb.light(local_index(head)) & 0xFF);
                    if sky >= 15 {
                        e.fire = e.fire.max(160);
                    }
                }
                if e.in_liquid {
                    e.fire = 0;
                }
                e.fire = e.fire.saturating_sub(1);
                let burn = e.fire > 0 && e.fire % 20 == 0;
                if k.category == 1 && e.age % 100 == 0 {
                    let near = players.iter().any(|p| (*p - e.pos).length_squared() < 128.0 * 128.0);
                    if !near {
                        e.removed = true;
                    }
                }
                if !e.item.is_empty() {
                    e.pickup_delay = e.pickup_delay.saturating_sub(1);
                    if e.age > ITEM_LIFETIME {
                        e.removed = true;
                    }
                }
                (land.is_some() || burn).then_some((e.id, land, burn))
            })
            .collect();
        let landings: Vec<(u64, f32)> = hits.iter().filter_map(|h| Some((h.0, h.1?))).collect();
        let burns: Vec<u64> = hits.iter().filter(|h| h.2).map(|h| h.0).collect();
        for (id, amount) in landings {
            self.damage(c, id, amount, 0, damage::FALL);
        }
        for id in burns {
            self.damage(c, id, 1.0, 0, damage::FIRE);
        }
        self.items(c);
    }

    /// Item entities: merge neighbours, get picked up by players.
    fn items(&mut self, c: &'static Content) {
        let players: Vec<(u64, DVec3)> =
            self.entities.list.iter().filter(|e| e.player && !e.removed && e.health > 0.0 && e.mode != crate::world::GameMode::Spectator).map(|e| (e.id, e.pos)).collect();
        let items: Vec<(u64, DVec3)> = self.entities.list.iter().filter(|e| !e.removed && !e.item.is_empty()).map(|e| (e.id, e.pos)).collect();
        for (id, pos) in items {
            let Some(e) = self.entities.get(id) else { continue };
            if e.pickup_delay > 0 {
                continue;
            }
            let stack = e.item;
            if let Some(&(pid, _)) = players.iter().find(|(_, pp)| (*pp + DVec3::Y * 0.5 - pos).length_squared() < 2.25) {
                let rest = self.entities.insert_item(c, pid, stack);
                if let Some(e) = self.entities.get_mut(id) {
                    if rest.is_empty() {
                        e.removed = true;
                    } else {
                        e.item = rest;
                    }
                }
                continue;
            }
            if self.time % 10 == 0 {
                let max = c.max_stack(stack.item);
                let other = self.entities.list.iter().position(|o| {
                    o.id != id
                        && !o.removed
                        && o.item.item == stack.item
                        && o.item.damage == stack.damage
                        && o.item.count + stack.count <= max
                        && (o.pos - pos).length_squared() < 1.0
                });
                if let Some(oi) = other {
                    self.entities.list[oi].item.count += stack.count;
                    if let Some(e) = self.entities.get_mut(id) {
                        e.removed = true;
                    }
                }
            }
        }
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
            self.call_block(c, state, |b| b.on_neighbor, |w| abi::BlockEventArgs { world: w, pos: p.to_array(), state, other: from.to_array(), entity: 0 });
        }
    }

    pub fn finish_tick(&mut self) {
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

    /// The player finished breaking a block holding `tool`.
    pub fn player_break(&mut self, c: &'static Content, p: IVec3, player: u64, tool: u32) -> bool {
        let Some(state) = self.get(p) else { return false };
        if state == AIR || c.states[state as usize].hardness < 0.0 {
            return false;
        }
        let (_, harvest) = break_time(c, state, tool);
        let harvest = harvest && !self.entities.get(player).is_some_and(|e| e.mode == crate::world::GameMode::Creative);
        self.set(c, p, AIR);
        let block = c.block_of(state);
        if harvest {
            if let Some(item) = block.drop {
                self.spawn_item(c, p.as_dvec3() + DVec3::splat(0.5), ItemStack::new(item, block.drop_count.max(1)));
            }
        }
        self.call_block(c, state, |b| b.on_broken, |w| abi::BlockEventArgs { world: w, pos: p.to_array(), state, other: [0; 3], entity: player });
        self.flush_neighbors(c);
        self.finish_tick();
        true
    }

    pub fn place(&mut self, c: &'static Content, p: IVec3, face: u8, state: u32, look: [f32; 3], player: u64) -> bool {
        let Some(cur) = self.get(p) else { return false };
        if c.states[cur as usize].flags & bf::REPLACEABLE == 0 {
            return false;
        }
        let def = c.block_of(state);
        let mut chosen = def.first_state;
        if let Some(cb) = def.cb.placement {
            let mut args = abi::PlaceArgs { pos: p.to_array(), face: face as u32, look, state: def.first_state };
            host::invoke(def.owner, cb, &mut args);
            if args.state >= def.first_state && args.state < def.first_state + def.states {
                chosen = args.state;
            }
        }
        self.set(c, p, chosen);
        self.call_block(c, chosen, |b| b.on_placed, |w| abi::BlockEventArgs { world: w, pos: p.to_array(), state: chosen, other: [0; 3], entity: player });
        self.flush_neighbors(c);
        self.finish_tick();
        true
    }

    /// True if the block consumed the click.
    pub fn player_use(&mut self, c: &'static Content, p: IVec3, face: u8, player: u64) -> bool {
        let Some(state) = self.get(p) else { return false };
        let r = self.call_block(c, state, |b| b.on_use, |w| abi::UseArgs { world: w, pos: p.to_array(), state, face: face as u32, player, handled: 0 });
        self.flush_neighbors(c);
        self.finish_tick();
        r.is_some_and(|a| a.handled != 0)
    }

    pub fn schedule(&mut self, p: IVec3, delay: u32) {
        let at = self.time + delay.max(1) as u64;
        self.scheduled.push(Reverse((at, p.x, p.y, p.z)));
    }

    pub fn random(&mut self) -> u64 {
        self.rng.next()
    }
}

/// Section = 4x4x4 cubes, the unit regions are made of.
#[inline]
pub fn section_of_cube(cube: IVec3) -> IVec3 {
    cube >> 2
}

#[inline]
pub fn section_of(p: DVec3) -> IVec3 {
    section_of_cube(cube_of(p.floor().as_ivec3()))
}

type Taken = (Entity, Option<Vec<ItemStack>>, Vec<(u32, Box<[u8]>)>);

/// Everything stored for a set of sections, moved between regions when they merge or split.
#[derive(Default)]
pub struct Parcel {
    cubes: Vec<(IVec3, Arc<Cube>, bool, bool)>,
    entities: Vec<Taken>,
    scheduled: Vec<Reverse<(u64, i32, i32, i32)>>,
}

impl Dim {
    /// Sections this region occupies: loaded cubes plus where its entities stand.
    pub fn sections(&self) -> FxHashSet<IVec3> {
        let mut s: FxHashSet<IVec3> = self.cubes.keys().map(|&c| section_of_cube(c)).collect();
        s.extend(self.entities.list.iter().filter(|e| !e.removed).map(|e| section_of(e.pos)));
        s
    }

    /// Moves out cubes, entities and pending ticks located in sections where `take` is true.
    pub fn take_sections(&mut self, take: impl Fn(IVec3) -> bool) -> Parcel {
        let mut p = Parcel::default();
        let cubes: Vec<IVec3> = self.cubes.keys().copied().filter(|&c| take(section_of_cube(c))).collect();
        for pos in cubes {
            let cube = self.cubes.remove(&pos).unwrap();
            self.grid.cube_unloaded(pos);
            self.conn.remove(&pos);
            p.cubes.push((pos, cube, self.modified.remove(&pos), self.settled.remove(&pos)));
        }
        let ids: Vec<u64> = self.entities.list.iter().filter(|e| take(section_of(e.pos))).map(|e| e.id).collect();
        for id in ids {
            if let Some(t) = self.entities.take(id) {
                p.entities.push(t);
            }
        }
        let (moved, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.scheduled)
            .into_iter()
            .partition(|Reverse((_, x, y, z))| take(section_of_cube(cube_of(IVec3::new(*x, *y, *z)))));
        self.scheduled = kept.into_iter().collect();
        p.scheduled = moved;
        p
    }

    pub fn absorb(&mut self, c: &Content, p: Parcel) {
        for (pos, cube, modified, settled) in p.cubes {
            self.grid.cube_loaded(c, pos, &cube);
            self.cubes.insert(pos, cube);
            if modified {
                self.modified.insert(pos);
            }
            if settled {
                self.settled.insert(pos);
                self.settled_gen += 1;
            }
        }
        for (e, inv, comps) in p.entities {
            let id = e.id;
            self.entities.insert(c, e);
            if let Some(inv) = inv {
                self.entities.inventories.insert(id, inv);
            }
            for (k, v) in comps {
                self.entities.comps.insert((id, k), v);
            }
        }
        self.entities.sort_by_id();
        self.scheduled.extend(p.scheduled);
    }

    pub fn connectivity(&mut self, c: &Content, pos: IVec3) -> Option<u16> {
        if let Some(&v) = self.conn.get(&pos) {
            return Some(v);
        }
        let v = see_through(c, self.cubes.get(&pos)?);
        self.conn.insert(pos, v);
        Some(v)
    }

    /// Marks `pos` and the loaded cubes below it settled: nothing above will ever bring sky
    /// light (it is not going to be loaded), so their light is final as it is.
    pub fn settle_down(&mut self, pos: IVec3) {
        let mut p = pos;
        while self.cubes.contains_key(&p) && self.settled.insert(p) {
            self.settled_gen += 1;
            p -= IVec3::Y;
        }
    }

    pub fn set_rng(&mut self, seed: u64) {
        self.rng = Rng(seed);
    }
}
