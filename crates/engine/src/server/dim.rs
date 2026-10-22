use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::{DVec3, IVec3};
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
    let mut cube = Cube::from_states(blocks.as_slice().try_into().unwrap());
    if biomes.iter().any(|&b| b != 0) {
        cube.biomes = Some(Box::new(biomes));
    }
    cube
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

    pub fn insert(&mut self, c: &Content, pos: IVec3, cube: Cube) {
        self.generating.remove(&pos);
        if self.cubes.contains_key(&pos) {
            return;
        }
        self.grid.cube_loaded(c, pos, &cube);
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
        if (k.is(ef::INVULNERABLE) && cause != damage::VOID) || e.hurt_cooldown > 0 || e.health <= 0.0 {
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
        let mut landings: Vec<(u64, f32)> = Vec::new();
        let mut burns: Vec<u64> = Vec::new();
        let cubes = &self.cubes;
        let block = |p: IVec3| cubes.get(&cube_of(p)).map(|cb| cb.get(local_index(p)));
        for e in self.entities.list.iter_mut() {
            if e.removed {
                continue;
            }
            e.age += 1;
            e.hurt_cooldown = e.hurt_cooldown.saturating_sub(1);
            if e.player {
                continue;
            }
            let k = &c.game.entities[e.kind as usize];
            if let Some(fall) = step(c, e, gravity, &block) {
                if k.is(ef::FALL_DAMAGE) && fall > 3.0 {
                    landings.push((e.id, fall - 3.0));
                }
            }
            if k.is(ef::BURNS_IN_SUN) && day > 0.5 && e.age % 20 == 0 && !e.in_liquid {
                let head = (e.pos + DVec3::Y * k.eye as f64).floor().as_ivec3();
                let sky = cubes.get(&cube_of(head)).map_or(0, |cb| cb.light(local_index(head)) & 0xFF);
                if sky >= 15 {
                    burns.push(e.id);
                }
            }
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
        }
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
            self.entities.list.iter().filter(|e| e.player && !e.removed && e.health > 0.0).map(|e| (e.id, e.pos)).collect();
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
