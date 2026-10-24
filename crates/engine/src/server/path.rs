//! Paths for walking mobs: A* over block cells with vanilla-like rules (room for the whole body,
//! a step up of one block by jumping, drops of up to three, no hot liquids), and the
//! navigation that walks an entity along one.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use glam::{DVec3, IVec3};
use rustc_hash::FxHashMap;
use voxide_abi::block_flags as bf;

use crate::content::Content;

/// Where an entity is walking to.
pub struct Nav {
    pub target: DVec3,
    pub speed: f32,
    pub path: Vec<IVec3>,
    pub next: usize,
    /// Age at which the path is looked for again (targets move, blocks change).
    pub repath: u64,
    /// Ticks without getting anywhere.
    pub stuck: u32,
}

const MAX_NODES: usize = 600;
const MAX_DROP: i32 = 3;

/// Cells a body `height` blocks tall can stand in, from the feet cell `from` towards `to`: the
/// whole way, or as far as it gets (the reachable cell closest to the target) when `to` is out
/// of reach or more than `MAX_NODES` cells away.
pub fn find(c: &Content, block: &impl Fn(IVec3) -> Option<u32>, from: IVec3, to: IVec3, height: i32) -> Vec<IVec3> {
    let open = |p: IVec3| {
        block(p).is_some_and(|s| {
            let st = &c.states[s as usize];
            st.flags & bf::SOLID == 0 && st.liquid.is_none_or(|l| c.liquids[l as usize].temperature < crate::server::liquid::BURNING)
        })
    };
    let room = |p: IVec3| (0..height).all(|k| open(p + IVec3::Y * k));
    let ground = |p: IVec3| block(p - IVec3::Y).is_some_and(|s| c.states[s as usize].flags & (bf::SOLID | bf::LIQUID) != 0);
    let standable = |p: IVec3| room(p) && ground(p);
    let wet = |p: IVec3| block(p).is_some_and(|s| c.states[s as usize].flags & bf::LIQUID != 0);
    let h = |p: IVec3| (p - to).as_vec3().length();

    let mut came: FxHashMap<IVec3, (IVec3, f32)> = FxHashMap::default();
    let mut queue = BinaryHeap::new();
    came.insert(from, (from, 0.0));
    queue.push(Reverse(((h(from) * 1000.0) as u32, from.x, from.y, from.z)));
    let mut best = (h(from), from);
    let mut visited = 0;
    while let Some(Reverse((_, x, y, z))) = queue.pop() {
        let p = IVec3::new(x, y, z);
        let g = came[&p].1;
        let d = h(p);
        if d < best.0 {
            best = (d, p);
        }
        if p == to || (d < 1.5 && (p.y - to.y).abs() <= 1) {
            best = (0.0, p);
            break;
        }
        visited += 1;
        if visited > MAX_NODES {
            break;
        }
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let side = IVec3::new(dx, 0, dz);
            // Diagonals only where both straight neighbours leave room (no corner cutting).
            if dx != 0 && dz != 0 && !(room(p + IVec3::new(dx, 0, 0)) && room(p + IVec3::new(0, 0, dz))) {
                continue;
            }
            let base = if dx != 0 && dz != 0 { 1.414 } else { 1.0 };
            let mut options = Vec::with_capacity(2);
            let q = p + side;
            if standable(q) {
                options.push((q, base));
            } else if !room(q) {
                // A step up: jump, with room above the head first.
                let up = q + IVec3::Y;
                if standable(up) && open(p + IVec3::Y * height) {
                    options.push((up, base + 1.0));
                }
            } else {
                // Open but nothing to stand on: drop down to the first ground.
                for k in 1..=MAX_DROP {
                    let down = q - IVec3::Y * k;
                    if !open(down) {
                        break;
                    }
                    if ground(down) {
                        options.push((down, base + 0.5 * k as f32));
                        break;
                    }
                }
            }
            for (n, cost) in options {
                let g2 = g + cost + if wet(n) { 2.0 } else { 0.0 };
                if came.get(&n).is_none_or(|&(_, old)| g2 < old) {
                    came.insert(n, (p, g2));
                    queue.push(Reverse((((g2 + h(n)) * 1000.0) as u32, n.x, n.y, n.z)));
                }
            }
        }
    }
    let mut path = vec![best.1];
    while let Some(&(prev, _)) = came.get(path.last().unwrap()) {
        if prev == *path.last().unwrap() {
            break;
        }
        path.push(prev);
    }
    path.reverse();
    path
}

/// Steering for this tick: walk direction, jump, and whether the target is reached.
pub fn steer(nav: &mut Nav, pos: DVec3, blocked: bool) -> Option<([f32; 2], bool)> {
    let feet = pos.floor().as_ivec3();
    while let Some(&w) = nav.path.get(nav.next) {
        let center = w.as_dvec3() + DVec3::new(0.5, 0.0, 0.5);
        let flat = (center - pos).with_y(0.0);
        if flat.length() < 0.35 && (w.y - feet.y).abs() <= 1 {
            nav.next += 1;
            continue;
        }
        let dir = flat.normalize_or_zero();
        return Some(([dir.x as f32, dir.z as f32], w.y > feet.y || blocked));
    }
    None
}
