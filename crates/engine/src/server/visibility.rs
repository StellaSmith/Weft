//! What one player can see, kept up to date as cubes arrive instead of being recomputed: a flood
//! fill from the player's cube through loaded cubes, leaving each one only by faces its blocks
//! connect to the face it was entered by. Unloaded cubes the fill reaches wait, with the faces it
//! reached them through, and the fill goes on from them once they are loaded.

use glam::IVec3;
use rustc_hash::{FxHashMap, FxHashSet};

use super::regions::DimSet;
use crate::content::Content;
use crate::world::DIRS;

/// Entered from nowhere: every face is an exit (the player's own cube).
const START: u8 = 1 << 6;

/// (dim, centre cube, radius, height, block edits): anything else changing starts over.
pub type Key = (u32, IVec3, i32, i32, u64);

pub struct Visibility {
    pub key: Key,
    /// Every reached cube, loaded or not.
    pub seen: FxHashSet<IVec3>,
    /// Faces already left through, per cube: entering again only explores new exits.
    explored: FxHashMap<IVec3, u8>,
    /// Reached but unloaded cubes: the faces they were entered through (`START` for the centre,
    /// 0 for cubes wanted only because they are near).
    pub waiting: FxHashMap<IVec3, u8>,
}

impl Visibility {
    /// Starts over from `center`; cubes within `near` are always wanted.
    pub fn new(key: Key, dim: &mut DimSet, c: &Content, center: IVec3, in_range: &impl Fn(IVec3) -> bool, near: i32) -> Visibility {
        let mut v = Visibility { key, seen: FxHashSet::default(), explored: FxHashMap::default(), waiting: FxHashMap::default() };
        for dy in -near..=near {
            for dz in -near..=near {
                for dx in -near..=near {
                    let p = center + IVec3::new(dx, dy, dz);
                    if in_range(p) && v.seen.insert(p) && !dim.has(p) {
                        v.waiting.insert(p, 0);
                    }
                }
            }
        }
        v.seen.insert(center);
        let mut queue = Vec::new();
        if dim.has(center) {
            queue.push((center, 6));
        } else {
            *v.waiting.entry(center).or_default() |= START;
        }
        v.fill(dim, c, in_range, queue);
        v
    }

    /// Goes on from waiting cubes that got loaded; returns them.
    pub fn update(&mut self, dim: &mut DimSet, c: &Content, in_range: &impl Fn(IVec3) -> bool) -> Vec<IVec3> {
        let arrived: Vec<(IVec3, u8)> = self.waiting.iter().filter(|(p, _)| dim.has(**p)).map(|(p, m)| (*p, *m)).collect();
        let mut queue = Vec::new();
        for &(p, mask) in &arrived {
            self.waiting.remove(&p);
            for from in 0..7 {
                if mask & (1 << from) != 0 {
                    queue.push((p, from));
                }
            }
        }
        self.fill(dim, c, in_range, queue);
        arrived.into_iter().map(|a| a.0).collect()
    }

    fn fill(&mut self, dim: &mut DimSet, c: &Content, in_range: &impl Fn(IVec3) -> bool, mut queue: Vec<(IVec3, usize)>) {
        while let Some((pos, from)) = queue.pop() {
            let Some(conn) = dim.connectivity(c, pos) else { continue };
            let exits = if from == 6 { 0x3F } else { crate::world::cube::exits(conn, from) };
            let done = self.explored.entry(pos).or_default();
            let new = exits & !*done;
            *done |= new;
            for f in 0..6 {
                if new & (1 << f) == 0 {
                    continue;
                }
                let n = pos + DIRS[f];
                if !in_range(n) {
                    continue;
                }
                self.seen.insert(n);
                // Entered through the face opposite to `f`.
                if dim.has(n) {
                    queue.push((n, f ^ 1));
                } else {
                    *self.waiting.entry(n).or_default() |= 1 << (f ^ 1);
                }
            }
        }
    }
}
