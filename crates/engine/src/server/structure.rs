//! Moving structures: blocks that move together. A structure is a small world of its own (a
//! `Dim` in its own coordinates, `framed`) owned by an entity whose position places it: block
//! (0, 0, 0) is centered on the entity. Whatever the world does with blocks works on it as it
//! is (placement, support, callbacks, loot, events); what leaves it (dropped items, sounds,
//! particles) goes to the world it moves in, carried over by `Dim::hand_over`.

use std::sync::{Arc, OnceLock};

use glam::{DVec3, IVec3};

use super::dim::{Dim, DimEvent};
use crate::content::{AIR, Content};
use crate::world::{Cube, cube_of, local_index};

/// Where block (0, 0, 0) of a structure starts, for its entity at `pos`.
pub fn origin(pos: DVec3) -> DVec3 {
    pos - DVec3::new(0.5, 0.0, 0.5)
}

/// The blocks a kind is built of (its `blocks` component), parsed once per kind.
pub fn template(c: &Content, kind: u32) -> Option<&'static [(IVec3, u32)]> {
    static ALL: OnceLock<Vec<Option<Vec<(IVec3, u32)>>>> = OnceLock::new();
    let all = ALL.get_or_init(|| {
        let comp = c.components.id(weft_abi::comp::BLOCKS);
        (0..c.game.entities.len() as u32)
            .map(|k| {
                let text = comp.and_then(|id| c.components.text(crate::components::ENTITY, k, id))?;
                let mut out = Vec::new();
                for part in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                    let w: Vec<&str> = part.split_whitespace().collect();
                    let n: Vec<i32> = w.iter().skip(1).filter_map(|v| v.parse().ok()).collect();
                    let state = w.first().and_then(|b| c.state_by_name(b));
                    let (Some(state), [x, y, z, rest @ ..]) = (state, &n[..]) else {
                        log::error!("{}: bad `blocks` part '{part}' (block x y z [x2 y2 z2])", c.game.entities[k as usize].name);
                        continue;
                    };
                    let (a, b) = (IVec3::new(*x, *y, *z), match rest {
                        [x2, y2, z2] => IVec3::new(*x2, *y2, *z2),
                        _ => IVec3::new(*x, *y, *z),
                    });
                    let (lo, hi) = (a.min(b), a.max(b));
                    for y in lo.y..=hi.y {
                        for z in lo.z..=hi.z {
                            for x in lo.x..=hi.x {
                                out.push((IVec3::new(x, y, z), state));
                            }
                        }
                    }
                }
                Some(out)
            })
            .collect()
    });
    all.get(kind as usize)?.as_deref()
}

impl crate::physics::Blocks for Dim {
    fn block(&self, p: IVec3) -> u32 {
        self.get(p).unwrap_or(AIR)
    }
}

impl Dim {
    /// A structure's own space in dimension `dim`, holding `blocks`.
    pub fn new_structure(c: &Content, dim: u32, seed: u64, next_id: Arc<std::sync::atomic::AtomicU64>, blocks: &[(IVec3, u32)]) -> Dim {
        let mut d = Dim::new(dim, seed, next_id);
        d.framed = true;
        for &(p, state) in blocks {
            d.put(c, p, state);
        }
        d.relight();
        d.edited.clear();
        d
    }

    /// Writes a block of a structure without block updates (building it, loading it).
    fn put(&mut self, c: &Content, p: IVec3, state: u32) {
        let cp = cube_of(p);
        self.structure_cube(c, cp);
        if let Some(cube) = self.cubes.get_mut(&cp) {
            Arc::make_mut(cube).set(local_index(p), state);
        }
        self.grow_bounds(p, state);
        self.relight.insert(cp);
    }

    /// Makes cube `cp` of a structure exist (air) so blocks can go there.
    pub fn structure_cube(&mut self, c: &Content, cp: IVec3) {
        if !self.cubes.contains_key(&cp) {
            let cube = Cube::uniform(AIR, crate::world::light::SKY_FULL as u32);
            self.grid.cube_loaded(c, cp, &cube);
            self.add_cube(cp, Arc::new(cube));
            self.settled.insert(cp);
        }
    }

    pub fn grow_bounds(&mut self, p: IVec3, state: u32) {
        if state == AIR {
            // Shrinking needs a look over everything (only when the edge itself went).
            if self.bounds.is_some_and(|(lo, hi)| p.cmpeq(lo).any() || p.cmpeq(hi).any()) {
                self.bounds = self.scan_bounds();
            }
        } else {
            self.bounds = Some(self.bounds.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
        }
    }

    fn scan_bounds(&self) -> Option<(IVec3, IVec3)> {
        let mut out: Option<(IVec3, IVec3)> = None;
        for (&cp, cube) in &self.cubes {
            if cube.uniform_state() == Some(AIR) {
                continue;
            }
            for i in 0..crate::world::cube::VOLUME {
                if cube.get(i) != AIR {
                    let p = cp * 16 + IVec3::new((i & 15) as i32, (i >> 8) as i32, ((i >> 4) & 15) as i32);
                    out = Some(out.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
                }
            }
        }
        out
    }

    /// Structures light each of their cubes as open to the sky above (they move under the sky
    /// of whatever world they are in).
    pub fn relight(&mut self) {
        let c = crate::content::content();
        for cp in std::mem::take(&mut self.relight) {
            if let Some(cube) = self.cubes.get_mut(&cp) {
                let cube = Arc::make_mut(cube);
                cube.fill_light(0);
                crate::world::light::prelight(c, cube, true);
                self.resend.push((cp, Arc::new(cube.clone())));
            }
        }
    }

    /// Box around a structure's blocks in world space for its entity at `pos`.
    pub fn structure_box(&self, pos: DVec3) -> Option<crate::physics::Aabb> {
        let (lo, hi) = self.bounds?;
        let o = origin(pos);
        Some(crate::physics::Aabb { min: o + lo.as_dvec3(), max: o + (hi + IVec3::ONE).as_dvec3() })
    }

    /// Takes over what left structure `id` this tick: entities (dropped items, anything spawned
    /// in it; block entities stay) and sounds and particles, moved into world coordinates.
    pub fn hand_over(&mut self, c: &Content, id: u64) {
        let Some(pos) = self.entities.get(id).map(|e| e.pos) else { return };
        let o = origin(pos);
        let Some(s) = self.structures.get_mut(&id) else { return };
        let free: Vec<u64> = s.entities.list.iter().filter(|e| !e.removed && e.anchor.is_none()).map(|e| e.id).collect();
        let mut moved = Vec::new();
        for eid in free {
            if let Some((mut e, inv, comps)) = s.entities.take(eid) {
                e.pos += o;
                moved.push((e, inv, comps));
            }
        }
        s.entities.despawned.clear();
        let mut events = std::mem::take(&mut s.events);
        for ev in &mut events {
            match ev {
                DimEvent::Sound { pos, .. } => *pos += o,
                DimEvent::Particles { burst, .. } => burst.pos = (DVec3::from_array(burst.pos) + o).to_array(),
                _ => {}
            }
        }
        for (e, inv, comps) in moved {
            self.entities.insert_with(c, e, inv, comps);
        }
        self.events.extend(events);
    }

    /// Runs `f` on the space blocks at `frame` are in: this region (0) or the structure of
    /// entity `frame`, with where that space's block (0, 0, 0) starts in the world. What leaves
    /// the structure comes over after.
    pub fn with_space<R>(&mut self, c: &Content, frame: u64, f: impl FnOnce(&mut Dim, DVec3) -> R) -> Option<R> {
        if frame == 0 {
            return Some(f(self, DVec3::ZERO));
        }
        let o = origin(self.entities.get(frame)?.pos);
        let mut s = self.structures.remove(&frame)?;
        let out = f(&mut s, o);
        self.structures.insert(frame, s);
        self.hand_over(c, frame);
        Some(out)
    }

    /// The blocks of a structure for saving: (position, state name), air left out.
    pub fn structure_blocks(&self, c: &Content) -> Vec<(IVec3, String)> {
        let mut out = Vec::new();
        for (&cp, cube) in &self.cubes {
            for i in 0..crate::world::cube::VOLUME {
                let s = cube.get(i);
                if s != AIR {
                    out.push((cp * 16 + IVec3::new((i & 15) as i32, (i >> 8) as i32, ((i >> 4) & 15) as i32), c.state_name(s)));
                }
            }
        }
        out.sort_by_key(|b| (b.0.x, b.0.y, b.0.z));
        out
    }
}
