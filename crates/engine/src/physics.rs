//! Axis-separated AABB movement against block collision boxes and solid bodies (entities others
//! stand on and bump into), riding bodies, plus voxel raycasts. Shared by the client (local
//! player prediction) and the server (entities), so both agree.

use glam::{DVec3, IVec3};
use weft_abi::{Model, block_flags as bf};

use crate::content::Content;

/// A crouching player's box and eyes (modern vanilla), on client and server alike.
/// Crouching lowers a box and its eyes like the player's 1.8 to 1.5 and 1.62 to 1.27
/// (vanilla), whatever size it has.
pub fn crouched(height: f64, eye: f64) -> (f64, f64) {
    let r = |v: f64| (v * 1e6).round() / 1e6;
    (r(height * 1.5 / 1.8), r(eye * 1.27 / 1.62))
}

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub fn around(feet: DVec3, half_width: f64, height: f64) -> Aabb {
        Aabb {
            min: DVec3::new(feet.x - half_width, feet.y, feet.z - half_width),
            max: DVec3::new(feet.x + half_width, feet.y + height, feet.z + half_width),
        }
    }

    pub fn intersects(&self, o: &Aabb) -> bool {
        self.min.cmplt(o.max).all() && self.max.cmpgt(o.min).all()
    }

    pub fn shifted(&self, by: DVec3) -> Aabb {
        Aabb { min: self.min + by, max: self.max + by }
    }
}

/// Blocks a moving structure is built of, in its own coordinates (outside it: air).
pub trait Blocks: Sync {
    fn block(&self, p: IVec3) -> u32;
}

/// Collision boxes of a state in block-local coordinates.
pub fn boxes(c: &Content, state: u32, out: &mut Vec<[f64; 6]>) {
    let s = &c.states[state as usize];
    if s.flags & bf::SOLID == 0 {
        return;
    }
    if !s.collision.is_empty() {
        out.extend(s.collision.iter().map(|b| b.map(|v| v as f64)));
    } else if s.model != Model::Boxes {
        out.push([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
    }
}

/// A solid body this step: its box after its move (around all its blocks for a structure), how
/// far it moved, how high above it riders still move with it, whether it crushes what it
/// squeezes, and the blocks it is built of with where its block (0, 0, 0) starts.
#[derive(Clone, Copy)]
pub struct Body<'a> {
    pub id: u64,
    pub aabb: Aabb,
    pub delta: DVec3,
    pub carry: f64,
    pub crushes: bool,
    pub blocks: Option<(&'a dyn Blocks, DVec3)>,
}

impl Body<'_> {
    /// Its solid boxes touching `area`, in world space (until `f` returns true).
    pub fn solids(&self, c: &Content, area: Aabb, mut f: impl FnMut(Aabb) -> bool) -> bool {
        if !self.aabb.intersects(&area) {
            return false;
        }
        match self.blocks {
            None => f(self.aabb),
            Some((blocks, origin)) => each_solid(c, &|p| Some(blocks.block(p)), area.shifted(-origin), |b| f(b.shifted(origin))),
        }
    }

    /// The highest top of its solid boxes under a box with feet at `pos`, at most `reach` below
    /// the feet (and not above them).
    fn top_below(&self, c: &Content, pos: DVec3, half_width: f64, reach: f64) -> Option<f64> {
        let area = Aabb { min: DVec3::new(pos.x - half_width, pos.y - reach - 1e-3, pos.z - half_width), max: DVec3::new(pos.x + half_width, pos.y + 1e-3, pos.z + half_width) };
        let mut top: Option<f64> = None;
        self.solids(c, area, |b| {
            let inside = b.min.x < area.max.x && b.max.x > area.min.x && b.min.z < area.max.z && b.max.z > area.min.z;
            if inside && b.max.y <= pos.y + 1e-3 && b.max.y >= pos.y - reach - 1e-3 {
                top = Some(top.map_or(b.max.y, |t: f64| t.max(b.max.y)));
            }
            false
        });
        top
    }
}

/// What a rider's support does to it before its own move: carries it by `carry`, or lets go
/// (`released`: the support's last move, for keeping its momentum).
#[derive(Default)]
pub struct Ride {
    pub carry: DVec3,
    pub released: Option<DVec3>,
}

/// A rider moves with the body it stands on, and keeps doing so while in the air above it up to
/// the body's carry height (its own motion stays relative to the body, like inside a moving
/// ship); further away it lets go.
pub fn ride(pos: DVec3, half_width: f64, support: &mut Option<u64>, bodies: &[Body<'_>]) -> Ride {
    let Some(id) = *support else { return Ride::default() };
    let Some(b) = bodies.iter().find(|b| b.id == id) else {
        *support = None;
        return Ride::default();
    };
    // Where the body was before its move, which is where the rider stood on it.
    let (lo, hi) = (b.aabb.min - b.delta, b.aabb.max - b.delta);
    // A rider right at the edge stays on (exact edges come and go with rounding).
    let e = half_width + 1e-6;
    let over = pos.x + e > lo.x && pos.x - e < hi.x && pos.z + e > lo.z && pos.z - e < hi.z;
    // Anywhere from its bottom (inside a structure's cabin, say) up to its carry height.
    if over && pos.y > lo.y - 0.25 && pos.y - hi.y <= b.carry.max(0.01) {
        Ride { carry: b.delta, released: None }
    } else {
        *support = None;
        Ride { carry: DVec3::ZERO, released: Some(b.delta) }
    }
}

/// The body under a box with feet at `pos`, its top at most `reach` below the feet: the frame
/// a position put there belongs to.
pub fn body_below(c: &Content, pos: DVec3, half_width: f64, bodies: &[Body<'_>], reach: f64) -> Option<u64> {
    bodies.iter().filter_map(|b| b.top_below(c, pos, half_width, reach).map(|t| (b.id, t))).max_by(|a, b| a.1.total_cmp(&b.1)).map(|b| b.0)
}

/// The body whose top a box with feet at `pos` rests on.
pub fn standing_on(c: &Content, pos: DVec3, half_width: f64, bodies: &[Body<'_>], skip: u64) -> Option<u64> {
    bodies.iter().find(|b| b.id != skip && b.top_below(c, pos, half_width, 0.0).is_some()).map(|b| b.id)
}

/// How far bodies that moved this step and overlap a box push it out of them, along each body's
/// main direction of travel, and whether a crushing one did. A box already inside goes out too
/// (like vanilla's shulker boxes): a body that showed up around a rider, or a rider put on a
/// deck the client draws ahead of the server, must not stay stuck in it. The body it rides and
/// `skip` (itself) do not push.
pub fn push(c: &Content, pos: DVec3, half_width: f64, height: f64, bodies: &[Body<'_>], skip: u64, support: Option<u64>) -> (DVec3, bool) {
    let me = Aabb::around(pos, half_width, height);
    let (mut shift, mut crushes) = (DVec3::ZERO, false);
    for b in bodies.iter().filter(|b| b.id != skip && Some(b.id) != support && b.delta != DVec3::ZERO) {
        let d = b.delta.abs();
        let axis = if d.y >= d.x && d.y >= d.z { 1 } else if d.x >= d.z { 0 } else { 2 };
        let up = b.delta[axis] > 0.0;
        b.solids(c, me, |s| {
            if s.intersects(&me) {
                let need = if up { s.max[axis] - me.min[axis] } else { s.min[axis] - me.max[axis] };
                if need.abs() > shift[axis].abs() {
                    shift[axis] = need;
                }
                crushes |= b.crushes;
            }
            false
        });
    }
    (shift, crushes)
}

/// Calls `f` with the solid collision boxes of the blocks around `area` (unloaded positions count
/// as full blocks) until it returns true; true if it did. Nothing is collected: thousands of mobs
/// move every tick.
fn each_solid(c: &Content, block: &impl Fn(IVec3) -> Option<u32>, area: Aabb, mut f: impl FnMut(Aabb) -> bool) -> bool {
    // One extra layer below for collision boxes taller than a block (fences); everything else
    // stays inside its own block.
    let lo = area.min.floor().as_ivec3() - IVec3::Y;
    let hi = area.max.floor().as_ivec3();
    for y in lo.y..=hi.y {
        for z in lo.z..=hi.z {
            for x in lo.x..=hi.x {
                let p = IVec3::new(x, y, z);
                let b = p.as_dvec3();
                let full = Aabb { min: b, max: b + DVec3::ONE };
                let Some(s) = block(p) else {
                    if f(full) {
                        return true;
                    }
                    continue;
                };
                let st = &c.states[s as usize];
                if st.flags & bf::SOLID == 0 {
                    continue;
                }
                if st.collision.is_empty() {
                    if st.model != Model::Boxes && f(full) {
                        return true;
                    }
                    continue;
                }
                for bx in &st.collision {
                    let bx = bx.map(|v| v as f64);
                    if f(Aabb { min: b + DVec3::new(bx[0], bx[1], bx[2]), max: b + DVec3::new(bx[3], bx[4], bx[5]) }) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Whether `area` overlaps anything solid.
pub fn collides(c: &Content, block: &impl Fn(IVec3) -> Option<u32>, area: Aabb) -> bool {
    each_solid(c, block, area, |s| s.intersects(&area))
}

thread_local! {
    /// `move_box_with`'s obstacles, kept between calls.
    static SOLIDS: std::cell::RefCell<Vec<Aabb>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub struct MoveResult {
    pub pos: DVec3,
    pub hit: [bool; 3],
    pub on_ground: bool,
}

/// Moves a box with feet at `pos` by `delta`. `block(p)` returns the state at p, or `None` for
/// unloaded positions which are treated as solid so nothing falls out of the world while loading.
pub fn move_box(
    c: &Content,
    block: &impl Fn(IVec3) -> Option<u32>,
    pos: DVec3,
    half_width: f64,
    height: f64,
    delta: DVec3,
    step_height: f64,
) -> MoveResult {
    move_box_with(c, block, &[], 0, pos, half_width, height, delta, step_height)
}

/// `move_box` that also bumps into solid bodies (all but `skip`).
#[allow(clippy::too_many_arguments)]
pub fn move_box_with(
    c: &Content,
    block: &impl Fn(IVec3) -> Option<u32>,
    bodies: &[Body<'_>],
    skip: u64,
    pos: DVec3,
    half_width: f64,
    height: f64,
    delta: DVec3,
    step_height: f64,
) -> MoveResult {
    let start = Aabb::around(pos, half_width, height);
    let mut sweep = start;
    for i in 0..3 {
        if delta[i] < 0.0 {
            sweep.min[i] += delta[i];
        } else {
            sweep.max[i] += delta[i];
        }
    }
    sweep.max.y += step_height;
    let mut solids = SOLIDS.with_borrow_mut(std::mem::take);
    solids.clear();
    each_solid(c, block, sweep, |b| {
        solids.push(b);
        false
    });
    for b in bodies.iter().filter(|b| b.id != skip) {
        b.solids(c, sweep, |s| {
            solids.push(s);
            false
        });
    }

    let try_move = |from: Aabb, delta: DVec3| -> (Aabb, [bool; 3], DVec3) {
        let mut b = from;
        let mut hit = [false; 3];
        let mut moved = DVec3::ZERO;
        for axis in [1usize, 0, 2] {
            let mut d = delta[axis];
            if d == 0.0 {
                continue;
            }
            for s in &solids {
                let overlap = (0..3).filter(|&a| a != axis).all(|a| b.min[a] < s.max[a] - 1e-7 && b.max[a] > s.min[a] + 1e-7);
                if !overlap {
                    continue;
                }
                if d > 0.0 && b.max[axis] <= s.min[axis] + 1e-7 {
                    d = d.min(s.min[axis] - b.max[axis]);
                } else if d < 0.0 && b.min[axis] >= s.max[axis] - 1e-7 {
                    d = d.max(s.max[axis] - b.min[axis]);
                }
            }
            if d != delta[axis] {
                hit[axis] = true;
            }
            b.min[axis] += d;
            b.max[axis] += d;
            moved[axis] = d;
        }
        (b, hit, moved)
    };

    let (mut b, mut hit, _) = try_move(start, delta);
    let falling_or_grounded = delta.y <= 0.0;
    if step_height > 0.0 && (hit[0] || hit[2]) && falling_or_grounded {
        // Retry lifted by the step height, keep it if it gets further horizontally.
        let (up, _, lifted) = try_move(start, DVec3::new(0.0, step_height, 0.0));
        let (side, side_hit, _) = try_move(up, DVec3::new(delta.x, 0.0, delta.z));
        let (down, _, _) = try_move(side, DVec3::new(0.0, -lifted.y + delta.y.min(0.0), 0.0));
        let gained = |a: &Aabb| (a.min.x - start.min.x).powi(2) + (a.min.z - start.min.z).powi(2);
        if gained(&down) > gained(&b) + 1e-6 {
            b = down;
            hit = [side_hit[0], false, side_hit[2]];
        }
    }
    let on_ground = hit[1] && delta.y < 0.0;
    let pos = DVec3::new(b.min.x + half_width, b.min.y, b.min.z + half_width);
    SOLIDS.with_borrow_mut(|s| *s = solids);
    MoveResult { pos, hit, on_ground }
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    /// In the coordinates of `frame`: the moving structure (entity) the block is part of, 0 for
    /// the world.
    pub block: IVec3,
    pub frame: u64,
    /// Face index (-X, +X, -Y, +Y, -Z, +Z) that was entered.
    pub face: u8,
    pub state: u32,
}

/// Amanatides-Woo traversal. Non-cube blocks are hit when the ray crosses their collision box
/// (or selection box); blocks the ray starts inside are skipped.
/// First targetable block along the ray; with `liquids` liquids stop it too.
pub fn raycast(c: &Content, block: &impl Fn(IVec3) -> Option<u32>, origin: DVec3, dir: DVec3, max: f64, liquids: bool) -> Option<Hit> {
    let dir = dir.normalize();
    let mut p = origin.floor().as_ivec3();
    let step = dir.signum().as_ivec3();
    let inv = DVec3::ONE / dir.abs().max(DVec3::splat(1e-12));
    let mut t_max = DVec3::ZERO;
    for i in 0..3 {
        let o = origin[i] - p[i] as f64;
        t_max[i] = if dir[i] > 0.0 { (1.0 - o) * inv[i] } else { o * inv[i] };
    }
    let mut t = 0.0;
    while t <= max {
        if let Some(state) = block(p) {
            let s = &c.states[state as usize];
            if s.flags & bf::NOT_TARGETABLE == 0 || (liquids && s.flags & bf::LIQUID != 0) {
                let shape: Vec<[f32; 6]> = match s.model {
                    Model::Cube => vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]],
                    Model::Cross => vec![[0.125, 0.0, 0.125, 0.875, 0.8, 0.875]],
                    _ if !s.collision.is_empty() => s.collision.clone(),
                    _ if !s.boxes.is_empty() => s.boxes.iter().map(|b| [b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]]).collect(),
                    _ => vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]],
                };
                let local = origin - p.as_dvec3();
                let best = shape.iter().filter_map(|b| ray_box(local, dir, b, max)).min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, face)) = best {
                    return Some(Hit { frame: 0, block: p, face, state });
                }
            }
        }
        let axis = if t_max.x < t_max.y && t_max.x < t_max.z {
            0
        } else if t_max.y < t_max.z {
            1
        } else {
            2
        };
        t = t_max[axis];
        t_max[axis] += inv[axis];
        p[axis] += step[axis];
    }
    None
}

/// Entry distance and entered face, `None` if missed or if the ray starts inside.
fn ray_box(o: DVec3, d: DVec3, b: &[f32; 6], max: f64) -> Option<(f64, u8)> {
    let mut t0 = f64::NEG_INFINITY;
    let mut t1 = max;
    let mut face = 0u8;
    for i in 0..3 {
        let (lo, hi) = (b[i] as f64, b[i + 3] as f64);
        if d[i].abs() < 1e-12 {
            if o[i] < lo || o[i] > hi {
                return None;
            }
            continue;
        }
        let a = (lo - o[i]) / d[i];
        let c = (hi - o[i]) / d[i];
        let (near, far) = if a < c { (a, c) } else { (c, a) };
        if near > t0 {
            t0 = near;
            // Entering through the low side means the -axis face.
            face = (i * 2) as u8 + if d[i] > 0.0 { 0 } else { 1 };
        }
        t1 = t1.min(far);
    }
    (t0 <= t1 && t0 >= 0.0).then_some((t0, face))
}
