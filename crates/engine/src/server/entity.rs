//! Entities of one dimension: core state in a dense list, mod components as raw bytes, inventories
//! as an engine contract every mod can read and write.

use glam::{DVec3, IVec3};
use rustc_hash::{FxHashMap, FxHashSet};
use voxide_abi::{ItemStack, entity_flags as ef};

use crate::content::Content;
use crate::physics;

pub struct Entity {
    pub id: u64,
    pub kind: u32,
    pub pos: DVec3,
    pub vel: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub in_liquid: bool,
    pub fall: f32,
    pub moved: f32,
    pub health: f32,
    pub age: u64,
    pub hurt_cooldown: u32,
    /// AI steering requested this tick: direction, speed factor, jump.
    pub walk: Option<([f32; 2], f32, bool)>,
    pub removed: bool,
    pub item: ItemStack,
    pub pickup_delay: u32,
    pub player: bool,
    /// Hotbar slot the player holds.
    pub selected: u32,
    pub anchor: Option<IVec3>,
    pub sent: Option<(DVec3, f32, f32)>,
    /// Worn items as last sent to clients.
    pub worn: Vec<u32>,
}

impl Entity {
    pub fn new(id: u64, kind: u32, pos: DVec3, health: f32) -> Entity {
        Entity {
            id,
            kind,
            pos,
            vel: DVec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            in_liquid: false,
            fall: 0.0,
            moved: 0.0,
            health,
            age: 0,
            hurt_cooldown: 0,
            walk: None,
            removed: false,
            item: ItemStack::EMPTY,
            pickup_delay: 0,
            player: false,
            selected: 0,
            anchor: None,
            sent: None,
            worn: Vec::new(),
        }
    }

    pub fn state(&self, c: &Content) -> voxide_abi::EntityState {
        let k = &c.game.entities[self.kind as usize];
        voxide_abi::EntityState {
            kind: self.kind,
            flags: k.flags,
            pos: self.pos.to_array(),
            vel: self.vel.to_array(),
            yaw: self.yaw,
            pitch: self.pitch,
            on_ground: self.on_ground as u32,
            in_liquid: self.in_liquid as u32,
            health: self.health,
            max_health: k.max_health,
            age: self.age,
            fall: self.fall,
            moved: self.moved,
        }
    }
}

#[derive(Default)]
pub struct Entities {
    pub list: Vec<Entity>,
    index: FxHashMap<u64, usize>,
    pub comps: FxHashMap<(u64, u32), Box<[u8]>>,
    pub inventories: FxHashMap<u64, Vec<ItemStack>>,
    pub dirty_inv: FxHashSet<u64>,
    pub dirty_comp: FxHashSet<(u64, u32)>,
    pub anchors: FxHashMap<IVec3, u64>,
    pub spawned: Vec<u64>,
    pub despawned: Vec<u64>,
    pub hurt: Vec<(u64, f32)>,
}

pub const PLAYER_SLOTS: u32 = 40;
/// Player slots 36..40 are equipment: never filled by pickups, shown on the model.
pub const PLAYER_WORN: std::ops::Range<usize> = 36..40;

impl Entities {
    pub fn get(&self, id: u64) -> Option<&Entity> {
        self.index.get(&id).map(|&i| &self.list[i]).filter(|e| !e.removed)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Entity> {
        self.index.get(&id).map(|&i| &mut self.list[i]).filter(|e| !e.removed)
    }

    pub fn insert(&mut self, c: &Content, e: Entity) {
        let id = e.id;
        let kind = &c.game.entities[e.kind as usize];
        for &comp in &kind.components {
            self.comps.entry((id, comp)).or_insert_with(|| c.game.components[comp as usize].default.clone().into_boxed_slice());
        }
        if kind.slots > 0 {
            self.inventories.entry(id).or_insert_with(|| vec![ItemStack::EMPTY; kind.slots as usize]);
        }
        if e.player {
            self.inventories.entry(id).or_insert_with(|| vec![ItemStack::EMPTY; PLAYER_SLOTS as usize]);
        }
        if let Some(a) = e.anchor {
            self.anchors.insert(a, id);
        }
        self.index.insert(id, self.list.len());
        self.list.push(e);
        self.spawned.push(id);
    }

    pub fn remove(&mut self, id: u64) {
        if let Some(e) = self.get_mut(id) {
            e.removed = true;
            if let Some(a) = e.anchor {
                self.anchors.remove(&a);
            }
        }
    }

    /// Drops removed entities and their data. Keeps list order (it is the tick order).
    pub fn compact(&mut self) {
        if !self.list.iter().any(|e| e.removed) {
            return;
        }
        let gone: Vec<u64> = self.list.iter().filter(|e| e.removed).map(|e| e.id).collect();
        self.list.retain(|e| !e.removed);
        self.index = self.list.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
        for id in &gone {
            self.inventories.remove(id);
            self.comps.retain(|(e, _), _| e != id);
            self.dirty_inv.remove(id);
        }
        self.despawned.extend(gone);
    }

    /// Takes an entity out entirely (moving it to another dimension).
    pub fn take(&mut self, id: u64) -> Option<(Entity, Option<Vec<ItemStack>>, Vec<(u32, Box<[u8]>)>)> {
        let i = *self.index.get(&id)?;
        let e = self.list.remove(i);
        self.index = self.list.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
        let inv = self.inventories.remove(&id);
        let keys: Vec<(u64, u32)> = self.comps.keys().filter(|k| k.0 == id).copied().collect();
        let comps = keys.into_iter().filter_map(|k| self.comps.remove(&k).map(|v| (k.1, v))).collect();
        self.despawned.push(id);
        Some((e, inv, comps))
    }

    /// Canonical tick order after regions merge.
    pub fn sort_by_id(&mut self) {
        self.list.sort_by_key(|e| e.id);
        self.index = self.list.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
    }

    pub fn inv(&mut self, owner: u64) -> Option<&mut Vec<ItemStack>> {
        self.inventories.get_mut(&owner)
    }

    pub fn set_slot(&mut self, owner: u64, slot: u32, stack: ItemStack) {
        if let Some(inv) = self.inventories.get_mut(&owner) {
            if let Some(s) = inv.get_mut(slot as usize) {
                *s = if stack.count == 0 { ItemStack::EMPTY } else { stack };
                self.dirty_inv.insert(owner);
            }
        }
    }

    /// Merges into matching stacks, then empty slots. Returns the rest.
    pub fn insert_item(&mut self, c: &Content, owner: u64, mut stack: ItemStack) -> ItemStack {
        let max = c.max_stack(stack.item);
        let Some(inv) = self.inventories.get_mut(&owner) else { return stack };
        let limit = if inv.len() == PLAYER_SLOTS as usize { PLAYER_WORN.start } else { inv.len() };
        for s in inv[..limit].iter_mut() {
            if stack.count == 0 {
                break;
            }
            if s.item == stack.item && s.damage == stack.damage && s.count < max && !s.is_empty() {
                let n = (max - s.count).min(stack.count);
                s.count += n;
                stack.count -= n;
            }
        }
        for s in inv[..limit].iter_mut() {
            if stack.count == 0 {
                break;
            }
            if s.is_empty() {
                let n = max.min(stack.count);
                *s = ItemStack { count: n, ..stack };
                stack.count -= n;
            }
        }
        self.dirty_inv.insert(owner);
        if stack.count == 0 { ItemStack::EMPTY } else { stack }
    }

    pub fn near(&self, pos: DVec3, radius: f64, kind: u32) -> Vec<u64> {
        let r2 = radius * radius;
        self.list
            .iter()
            .filter(|e| !e.removed && (kind == u32::MAX || e.kind == kind) && (e.pos - pos).length_squared() <= r2)
            .map(|e| e.id)
            .collect()
    }
}

/// One physics step (1/20 s) for a non-player entity. `block` gives states, `None` = unloaded.
pub fn step(c: &Content, e: &mut Entity, gravity: f64, block: &impl Fn(IVec3) -> Option<u32>) -> Option<f32> {
    const DT: f64 = 0.05;
    let k = &c.game.entities[e.kind as usize];
    if e.anchor.is_some() {
        return None;
    }
    let feet = e.pos.floor().as_ivec3();
    let flags_at = |p: IVec3| block(p).map_or(0, |s| c.states[s as usize].flags);
    e.in_liquid = flags_at(feet) & voxide_abi::block_flags::LIQUID != 0;
    let start = e.pos;

    match e.walk.take() {
        Some((dir, speed, jump)) => {
            let target_x = dir[0] as f64 * k.speed as f64 * speed as f64;
            let target_z = dir[1] as f64 * k.speed as f64 * speed as f64;
            let accel = if e.on_ground || e.in_liquid { 0.4 } else { 0.08 };
            e.vel.x += (target_x - e.vel.x) * accel;
            e.vel.z += (target_z - e.vel.z) * accel;
            if jump {
                if e.on_ground {
                    e.vel.y = 8.4;
                } else if e.in_liquid {
                    e.vel.y = e.vel.y.max(2.5);
                }
            }
        }
        None => {
            let f = if e.on_ground { 0.55 } else if e.in_liquid { 0.8 } else { 0.98 };
            e.vel.x *= f;
            e.vel.z *= f;
        }
    }
    if !k.is(ef::NO_GRAVITY) {
        if e.in_liquid {
            e.vel.y -= gravity * DT * 0.2;
            e.vel.y *= 0.85;
            if k.is(ef::SWIMS) {
                e.vel.y += 1.2 * DT * 20.0 * 0.05;
            }
        } else {
            e.vel.y -= gravity * DT;
            e.vel.y *= 0.98;
        }
    }
    let step_h = if e.item.is_empty() && e.on_ground { 0.6 } else { 0.0 };
    let r = physics::move_box(c, block, e.pos, k.width as f64 / 2.0, k.height as f64, e.vel * DT, step_h);
    e.pos = r.pos;
    for i in 0..3 {
        if r.hit[i] {
            e.vel[i] = 0.0;
        }
    }
    let was_ground = e.on_ground;
    e.on_ground = r.on_ground;
    let dy = e.pos.y - start.y;
    e.moved = ((e.pos.x - start.x).powi(2) + (e.pos.z - start.z).powi(2)).sqrt() as f32;
    let mut land = None;
    if e.on_ground || e.in_liquid {
        if !was_ground && e.fall > 0.0 && !e.in_liquid {
            land = Some(e.fall);
        }
        e.fall = 0.0;
    } else if dy < 0.0 {
        e.fall += (-dy) as f32;
    }
    land
}
