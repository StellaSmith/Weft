//! What one player can see, kept up to date as cubes arrive instead of being recomputed: a flood
//! fill from the player's cube through loaded cubes, leaving each one only by faces its blocks
//! connect to the face it was entered by. Unloaded cubes the fill reaches wait, with the faces it
//! reached them through, and the fill goes on from them once they are loaded.
//!
//! Nothing starts over while the player moves or blocks change (at a view of 64 cubes a fresh
//! fill took over 100 ms of a tick): moving drops what left the range and goes on through the
//! faces that led out of it, and an edited cube is explored again from the faces it was entered
//! by. What was seen from where the player stood before stays seen while in range.

use glam::IVec3;
use rustc_hash::{FxHashMap, FxHashSet};

/// The world a fill goes through: the server's regions, or the client's own terrain.
pub trait Cubes {
    fn has(&self, pos: IVec3) -> bool;
    /// Which faces of the loaded cube at `pos` see each other (`cube::exits`).
    fn connectivity(&mut self, c: &Content, pos: IVec3) -> Option<u16>;
}

impl Cubes for super::regions::DimSet {
    fn has(&self, pos: IVec3) -> bool {
        super::regions::DimSet::has(self, pos)
    }
    fn connectivity(&mut self, c: &Content, pos: IVec3) -> Option<u16> {
        super::regions::DimSet::connectivity(self, c, pos)
    }
}
use crate::content::Content;
use crate::world::DIRS;

/// Entered from nowhere: every face is an exit (the player's own cube).
const START: u8 = 1 << 6;

/// The range a fill was made for: center cube, radius, cubes above and below.
pub type Range = (IVec3, i32, i32, i32);

/// A loaded cube the fill went through: faces it was entered by (`START` too) and faces it was
/// left by.
#[derive(Clone, Copy, Default)]
struct Explored {
    entered: u8,
    exits: u8,
}

pub struct Visibility {
    range: Option<Range>,
    /// Every reached cube in range, loaded or not.
    pub seen: FxHashSet<IVec3>,
    explored: FxHashMap<IVec3, Explored>,
    /// Exits that led out of range: (cube, face), taken once the range moves over them.
    frontier: FxHashSet<(IVec3, u8)>,
    /// Reached but unloaded cubes: the faces they were entered through (`START` for the center,
    /// 0 for cubes wanted only because they are near).
    pub waiting: FxHashMap<IVec3, u8>,
}

impl Visibility {
    pub fn new() -> Visibility {
        Visibility {
            range: None,
            seen: FxHashSet::default(),
            explored: FxHashMap::default(),
            frontier: FxHashSet::default(),
            waiting: FxHashMap::default(),
        }
    }

    /// Follows the player's `range` (`in_range` tells what is in it; cubes within `near` of
    /// its center are always wanted), re-explores `edited` cubes and goes on from waiting cubes
    /// that got loaded. Returns the loaded cubes the fill entered for the first time and the
    /// cubes that left the range.
    pub fn update(&mut self, dim: &mut impl Cubes, c: &Content, range: Range, in_range: &impl Fn(IVec3) -> bool, near: i32, edited: &[IVec3]) -> (Vec<IVec3>, Vec<IVec3>) {
        let mut queue = Vec::new();
        let mut left = Vec::new();
        let center = range.0;
        if self.range != Some(range) {
            self.range = Some(range);
            left = self.seen.iter().copied().filter(|&p| !in_range(p)).collect();
            for p in &left {
                self.seen.remove(p);
                self.waiting.remove(p);
                self.explored.remove(p);
                // Exits into it from cubes still in range lead out of range now.
                for (f, d) in DIRS.iter().enumerate() {
                    let n = *p - *d;
                    if let Some(e) = self.explored.get_mut(&n).filter(|e| e.exits & (1 << f) != 0) {
                        e.exits &= !(1 << f);
                        self.frontier.insert((n, f as u8));
                    }
                }
            }
            self.frontier.retain(|&(p, _)| in_range(p));
            let open: Vec<(IVec3, u8)> = self.frontier.iter().copied().filter(|&(p, f)| in_range(p + DIRS[f as usize])).collect();
            for (p, f) in open {
                self.frontier.remove(&(p, f));
                if let Some(e) = self.explored.get_mut(&p) {
                    e.exits |= 1 << f;
                }
                self.reach(dim, p + DIRS[f as usize], f ^ 1, &mut queue);
            }
            for dy in -near..=near {
                for dz in -near..=near {
                    for dx in -near..=near {
                        let p = center + IVec3::new(dx, dy, dz);
                        if in_range(p) && self.seen.insert(p) && !dim.has(p) {
                            self.waiting.insert(p, 0);
                        }
                    }
                }
            }
            self.seen.insert(center);
            if dim.has(center) {
                queue.push((center, 6));
            } else {
                *self.waiting.entry(center).or_default() |= START;
            }
        }
        for &p in edited {
            if let Some(e) = self.explored.get_mut(&p) {
                e.exits = 0;
                for from in 0..7 {
                    if e.entered & (1 << from) != 0 {
                        queue.push((p, from));
                    }
                }
            }
        }
        let arrived: Vec<(IVec3, u8)> = self.waiting.iter().filter(|(p, _)| dim.has(**p)).map(|(p, m)| (*p, *m)).collect();
        for &(p, mask) in &arrived {
            self.waiting.remove(&p);
            for from in 0..7 {
                if mask & (1 << from) != 0 {
                    queue.push((p, from));
                }
            }
        }
        // Near cubes reached by no face arrive too, without going on from them.
        let mut fresh = self.fill(dim, c, in_range, queue);
        fresh.extend(arrived.iter().filter(|a| a.1 == 0).map(|a| a.0));
        (fresh, left)
    }

    /// Reaches `n` through its face `from`.
    fn reach(&mut self, dim: &mut impl Cubes, n: IVec3, from: u8, queue: &mut Vec<(IVec3, u8)>) {
        self.seen.insert(n);
        if dim.has(n) {
            queue.push((n, from));
        } else {
            *self.waiting.entry(n).or_default() |= 1 << from;
        }
    }

    /// Returns the cubes entered for the first time.
    fn fill(&mut self, dim: &mut impl Cubes, c: &Content, in_range: &impl Fn(IVec3) -> bool, mut queue: Vec<(IVec3, u8)>) -> Vec<IVec3> {
        let mut fresh = Vec::new();
        while let Some((pos, from)) = queue.pop() {
            let Some(conn) = dim.connectivity(c, pos) else { continue };
            let exits = if from == 6 { 0x3F } else { crate::world::cube::exits(conn, from as usize) };
            let e = self.explored.entry(pos).or_insert_with(|| {
                fresh.push(pos);
                Explored::default()
            });
            e.entered |= 1 << from;
            let new = exits & !e.exits;
            e.exits |= new;
            for f in 0..6u8 {
                if new & (1 << f) == 0 {
                    continue;
                }
                let n = pos + DIRS[f as usize];
                if !in_range(n) {
                    self.explored.get_mut(&pos).unwrap().exits &= !(1 << f);
                    self.frontier.insert((pos, f));
                    continue;
                }
                // Entered through the face opposite to `f`.
                self.reach(dim, n, f ^ 1, &mut queue);
            }
        }
        fresh
    }
}
