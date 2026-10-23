//! A dimension split into independently simulated regions, like Folia. A region is a connected
//! set of loaded sections (4x4x4 cubes, 26-neighbourhood). Players far apart end up in different
//! regions that tick in parallel; when they approach each other the regions merge, when they part
//! the region splits. Anything built from loaded blocks (multiblocks, cables) lies in adjacent
//! sections and therefore always inside one region, so cross-region access never happens.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use glam::{DVec3, IVec3};
use rustc_hash::{FxHashMap, FxHashSet};
use voxide_abi as abi;

use super::dim::{Dim, section_of, section_of_cube};
use crate::content::Content;
use crate::host;
use crate::world::{Cube, cube_of, hash_pos};

pub struct DimSet {
    pub id: u32,
    pub regions: Vec<Dim>,
    pub generating: FxHashSet<IVec3>,
    pub time: u64,
    seed: u64,
    next_id: Arc<AtomicU64>,
    owner: FxHashMap<IVec3, usize>,
    pub dirty: bool,
}

impl DimSet {
    pub fn new(id: u32, seed: u64, next_id: Arc<AtomicU64>) -> DimSet {
        DimSet { id, regions: Vec::new(), generating: FxHashSet::default(), time: 1000, seed, next_id, owner: FxHashMap::default(), dirty: false }
    }

    fn new_region(&mut self, section: IVec3) -> usize {
        let mut d = Dim::new(self.id, self.seed, self.next_id.clone());
        d.set_rng(hash_pos(self.seed ^ self.time, section));
        d.time = self.time;
        self.regions.push(d);
        self.regions.len() - 1
    }

    /// Region owning `section`, or the neighbouring one, or a new region. Touching several regions
    /// merges them on the spot: data that is about to connect must already share a region, so
    /// lighting and anything else reading neighbours never meets a border. Splits can wait.
    fn region_for(&mut self, c: &Content, section: IVec3) -> usize {
        if let Some(&r) = self.owner.get(&section) {
            return r;
        }
        let mut near: Vec<usize> = Vec::new();
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(&r) = self.owner.get(&(section + IVec3::new(dx, dy, dz))) {
                        near.push(r);
                    }
                }
            }
        }
        near.sort_unstable();
        near.dedup();
        let r = match near.len() {
            0 => self.new_region(section),
            1 => near[0],
            _ => self.merge(c, &near),
        };
        self.owner.insert(section, r);
        r
    }

    /// Folds regions into the largest of them; returns its index afterwards.
    fn merge(&mut self, c: &Content, list: &[usize]) -> usize {
        let target = *list.iter().max_by_key(|&&i| self.regions[i].cubes.len()).unwrap();
        for &i in list.iter().rev() {
            if i != target {
                let parcel = self.regions[i].take_sections(|_| true);
                self.regions[target].absorb(c, parcel);
                self.regions.remove(i);
            }
        }
        log::debug!("dimension {}: {} regions merged", self.id, list.len());
        self.rebuild_owner();
        target - list.iter().filter(|&&i| i < target).count()
    }

    pub fn region_index(&self, pos: IVec3) -> Option<usize> {
        self.owner.get(&section_of_cube(cube_of(pos))).copied()
    }

    pub fn region_at(&mut self, pos: IVec3) -> Option<&mut Dim> {
        let r = self.region_index(pos)?;
        self.regions.get_mut(r)
    }

    /// Region for a position, created if needed (teleports, spawning players).
    pub fn ensure_region(&mut self, c: &Content, pos: DVec3) -> &mut Dim {
        let r = self.region_for(c, section_of(pos));
        &mut self.regions[r]
    }

    pub fn entity_region(&mut self, id: u64) -> Option<&mut Dim> {
        self.regions.iter_mut().find(|r| r.entities.get(id).is_some())
    }

    pub fn entity_region_ref(&self, id: u64) -> Option<&Dim> {
        self.regions.iter().find(|r| r.entities.get(id).is_some())
    }

    pub fn has(&self, pos: IVec3) -> bool {
        self.owner.get(&section_of_cube(pos)).is_some_and(|&r| self.regions[r].has(pos))
    }

    pub fn edit_version(&self) -> u64 {
        self.regions.iter().map(|r| r.edits).sum::<u64>() ^ (self.regions.len() as u64) << 52
    }

    /// Changes whenever a cube was loaded or became settled, or regions were reshaped.
    pub fn settled_version(&self) -> u64 {
        let settled: u64 = self.regions.iter().map(|r| r.settled_gen).sum();
        let cubes: u64 = self.regions.iter().map(|r| r.cubes.len() as u64).sum();
        settled ^ cubes << 24 ^ (self.regions.len() as u64) << 52
    }

    pub fn settled(&self, pos: IVec3) -> bool {
        self.owner.get(&section_of_cube(pos)).is_some_and(|&r| self.regions[r].settled.contains(&pos))
    }

    pub fn snapshot(&self, pos: IVec3) -> Option<Arc<Cube>> {
        let r = *self.owner.get(&section_of_cube(pos))?;
        self.regions[r].snapshot(pos)
    }

    pub fn get(&self, p: IVec3) -> Option<u32> {
        let r = *self.owner.get(&section_of_cube(cube_of(p)))?;
        self.regions[r].get(p)
    }

    pub fn insert(&mut self, c: &Content, pos: IVec3, cube: Cube, entities: &[u8], conn: u16, prelit: bool) {
        self.generating.remove(&pos);
        let r = self.region_for(c, section_of_cube(pos));
        let d = &mut self.regions[r];
        d.insert(c, pos, cube, prelit);
        d.conn.insert(pos, conn);
        for (e, inv, comps) in super::persist::read(c, entities) {
            let id = e.id;
            if d.entities.get(id).is_some() {
                continue;
            }
            d.entities.insert(c, e);
            if let Some(inv) = inv {
                d.entities.inventories.insert(id, inv);
            }
            for (k, v) in comps {
                d.entities.comps.insert((id, k), v);
            }
        }
    }

    pub fn connectivity(&mut self, c: &Content, pos: IVec3) -> Option<u16> {
        let r = *self.owner.get(&section_of_cube(pos))?;
        self.regions[r].connectivity(c, pos)
    }

    pub fn settle_down(&mut self, pos: IVec3) {
        if let Some(&r) = self.owner.get(&section_of_cube(pos)) {
            self.regions[r].settle_down(pos);
        }
    }

    /// What a viewer in cube `center` could see: a flood fill from that cube through loaded
    /// cubes, leaving each one only by faces its blocks connect to the face it was entered by.
    /// Solid ground and sealed caves stop the fill, so they are never loaded unless they come
    /// within `near` cubes. Returns every reached cube (loaded or not) and the unloaded ones.
    pub fn visible(&mut self, c: &Content, center: IVec3, in_range: impl Fn(IVec3) -> bool, near: i32) -> (FxHashSet<IVec3>, Vec<IVec3>) {
        let mut seen: FxHashSet<IVec3> = FxHashSet::default();
        let mut missing = Vec::new();
        for dy in -near..=near {
            for dz in -near..=near {
                for dx in -near..=near {
                    let p = center + IVec3::new(dx, dy, dz);
                    if in_range(p) && seen.insert(p) && !self.has(p) {
                        missing.push(p);
                    }
                }
            }
        }
        // Faces already left through, per cube: entering again only explores new exits.
        let mut explored: FxHashMap<IVec3, u8> = FxHashMap::default();
        let mut queue: std::collections::VecDeque<(IVec3, usize)> = std::collections::VecDeque::new();
        if self.has(center) {
            queue.push_back((center, 6));
        }
        while let Some((pos, from)) = queue.pop_front() {
            let Some(conn) = self.connectivity(c, pos) else { continue };
            let exits = if from == 6 { 0x3F } else { crate::world::cube::exits(conn, from) };
            let done = explored.entry(pos).or_default();
            let new = exits & !*done;
            *done |= new;
            for f in 0..6 {
                if new & (1 << f) == 0 {
                    continue;
                }
                let n = pos + crate::world::DIRS[f];
                if !in_range(n) {
                    continue;
                }
                let first = seen.insert(n);
                if !self.has(n) {
                    if first {
                        missing.push(n);
                    }
                    continue;
                }
                // Entered through the face opposite to `f`.
                queue.push_back((n, f ^ 1));
            }
        }
        (seen, missing)
    }

    pub fn spawn_point(&self, c: &Content) -> DVec3 {
        let def = &c.dims[self.id as usize];
        let mut pos = [0, 64, 0];
        if let Some(cb) = def.spawn {
            let mut args = abi::SpawnArgs { seed: self.seed, pos };
            host::invoke(def.owner, cb, &mut args);
            pos = args.pos;
        }
        DVec3::new(pos[0] as f64 + 0.5, pos[1] as f64 + 1.0, pos[2] as f64 + 0.5)
    }

    pub fn set_time(&mut self, t: u64) {
        self.time = t;
        for r in &mut self.regions {
            r.time = t;
        }
    }

    /// Recomputes regions from the sections in use: merges touching regions, splits separated
    /// parts. Data moves in parcels; each region keeps the part it holds most of.
    pub fn regionize(&mut self, c: &Content) {
        self.dirty = false;
        self.regions.retain(|r| !r.cubes.is_empty() || !r.entities.list.is_empty());
        let per_region: Vec<FxHashSet<IVec3>> = self.regions.iter().map(|r| r.sections()).collect();
        let all: Vec<IVec3> = per_region.iter().flatten().copied().collect::<FxHashSet<_>>().into_iter().collect();
        let index: FxHashMap<IVec3, usize> = all.iter().enumerate().map(|(i, &s)| (s, i)).collect();
        let mut parent: Vec<usize> = (0..all.len()).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for (i, &s) in all.iter().enumerate() {
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if let Some(&j) = index.get(&(s + IVec3::new(dx, dy, dz))) {
                            let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                            if a != b {
                                parent[a.max(b)] = a.min(b);
                            }
                        }
                    }
                }
            }
        }
        // Canonical component ids: the smallest section in each, so the result does not depend on
        // hash order.
        let mut root_min: FxHashMap<usize, IVec3> = FxHashMap::default();
        for (i, &s) in all.iter().enumerate() {
            let r = find(&mut parent, i);
            let e = root_min.entry(r).or_insert(s);
            if (s.x, s.y, s.z) < (e.x, e.y, e.z) {
                *e = s;
            }
        }
        let comp_of = |s: IVec3, parent: &mut Vec<usize>| root_min[&find(parent, index[&s])];

        // Primary component of each region: where most of its sections are.
        let mut primary: Vec<Option<IVec3>> = Vec::new();
        for secs in &per_region {
            let mut counts: FxHashMap<IVec3, usize> = FxHashMap::default();
            for &s in secs {
                *counts.entry(comp_of(s, &mut parent)).or_default() += 1;
            }
            primary.push(counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then((b.0.x, b.0.y, b.0.z).cmp(&(a.0.x, a.0.y, a.0.z)))).map(|x| x.0));
        }
        let sec_comp: FxHashMap<IVec3, IVec3> = all.iter().map(|&s| (s, comp_of(s, &mut parent))).collect();
        let changed = per_region.iter().enumerate().any(|(i, secs)| secs.iter().any(|s| Some(sec_comp[s]) != primary[i]))
            || {
                let mut seen = FxHashSet::default();
                primary.iter().flatten().any(|p| !seen.insert(*p))
            };
        if !changed {
            self.rebuild_owner();
            return;
        }

        // Split: every region gives away what is not in its primary component.
        let mut parcels: Vec<(IVec3, super::dim::Parcel)> = Vec::new();
        for (i, r) in self.regions.iter_mut().enumerate() {
            let mine = primary[i];
            let foreign: FxHashSet<IVec3> = per_region[i].iter().filter(|s| Some(sec_comp[*s]) != mine).map(|s| sec_comp[s]).collect();
            for comp in foreign {
                let sc = &sec_comp;
                parcels.push((comp, r.take_sections(|s| sc.get(&s) == Some(&comp))));
            }
        }
        // Merge: regions sharing a primary component fold into the first one.
        let mut by_comp: FxHashMap<IVec3, usize> = FxHashMap::default();
        let mut order: Vec<usize> = (0..self.regions.len()).collect();
        order.sort_by_key(|&i| primary[i].map(|p| (p.x, p.y, p.z)));
        let mut removed = vec![false; self.regions.len()];
        for i in order {
            let Some(p) = primary[i] else {
                removed[i] = true;
                continue;
            };
            match by_comp.get(&p) {
                Some(&target) => {
                    let all_parcel = self.regions[i].take_sections(|_| true);
                    self.regions[target].absorb(c, all_parcel);
                    removed[i] = true;
                    log::debug!("dimension {}: regions merged", self.id);
                }
                None => {
                    by_comp.insert(p, i);
                }
            }
        }
        for (comp, parcel) in parcels {
            let target = match by_comp.get(&comp) {
                Some(&t) => t,
                None => {
                    let t = self.new_region(comp);
                    removed.push(false);
                    by_comp.insert(comp, t);
                    log::debug!("dimension {}: region split", self.id);
                    t
                }
            };
            self.regions[target].absorb(c, parcel);
        }
        let mut i = 0;
        self.regions.retain(|_| {
            let keep = !removed[i];
            i += 1;
            keep
        });
        self.rebuild_owner();
    }

    fn rebuild_owner(&mut self) {
        self.owner.clear();
        for (i, r) in self.regions.iter().enumerate() {
            for s in r.sections() {
                self.owner.insert(s, i);
            }
        }
    }
}
