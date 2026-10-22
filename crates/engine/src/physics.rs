//! Axis-separated AABB movement against block collision boxes, plus voxel raycasts. Shared by the
//! client (local player prediction) and the server (entities), so both agree.

use glam::{DVec3, IVec3};
use voxide_abi::{Model, block_flags as bf};

use crate::content::Content;

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
    let mut boxes_buf = Vec::new();
    let mut collect = |area: Aabb| -> Vec<Aabb> {
        let mut out = Vec::new();
        let lo = area.min.floor().as_ivec3() - IVec3::ONE;
        let hi = area.max.floor().as_ivec3() + IVec3::ONE;
        for y in lo.y..=hi.y {
            for z in lo.z..=hi.z {
                for x in lo.x..=hi.x {
                    let p = IVec3::new(x, y, z);
                    let b = p.as_dvec3();
                    match block(p) {
                        None => out.push(Aabb { min: b, max: b + DVec3::ONE }),
                        Some(s) => {
                            boxes_buf.clear();
                            boxes(c, s, &mut boxes_buf);
                            for bx in &boxes_buf {
                                out.push(Aabb {
                                    min: b + DVec3::new(bx[0], bx[1], bx[2]),
                                    max: b + DVec3::new(bx[3], bx[4], bx[5]),
                                });
                            }
                        }
                    }
                }
            }
        }
        out
    };

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
    let solids = collect(sweep);

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
    MoveResult { pos: DVec3::new(b.min.x + half_width, b.min.y, b.min.z + half_width), hit, on_ground }
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub block: IVec3,
    /// Face index (-X, +X, -Y, +Y, -Z, +Z) that was entered.
    pub face: u8,
    pub state: u32,
}

/// Amanatides-Woo traversal. Non-cube blocks are hit when the ray crosses their collision box
/// (or the whole block if they have none).
pub fn raycast(c: &Content, block: &impl Fn(IVec3) -> Option<u32>, origin: DVec3, dir: DVec3, max: f64) -> Option<Hit> {
    let dir = dir.normalize();
    let mut p = origin.floor().as_ivec3();
    let step = dir.signum().as_ivec3();
    let inv = DVec3::ONE / dir.abs().max(DVec3::splat(1e-12));
    let mut t_max = DVec3::ZERO;
    for i in 0..3 {
        let o = origin[i] - p[i] as f64;
        t_max[i] = if dir[i] > 0.0 { (1.0 - o) * inv[i] } else { o * inv[i] };
    }
    let mut face = 0u8;
    let mut t = 0.0;
    while t <= max {
        if let Some(state) = block(p) {
            let s = &c.states[state as usize];
            if s.flags & bf::NOT_TARGETABLE == 0 {
                let shape: Vec<[f32; 6]> = if s.model == Model::Cube || (s.collision.is_empty() && s.boxes.is_empty()) {
                    vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
                } else if !s.collision.is_empty() {
                    s.collision.clone()
                } else {
                    s.boxes.iter().map(|b| [b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]]).collect()
                };
                let local = origin - p.as_dvec3();
                if shape.iter().any(|b| ray_box(local, dir, b, max)) {
                    return Some(Hit { block: p, face, state });
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
        face = (axis * 2) as u8 + if step[axis] > 0 { 0 } else { 1 };
    }
    None
}

fn ray_box(o: DVec3, d: DVec3, b: &[f32; 6], max: f64) -> bool {
    let mut t0 = 0.0f64;
    let mut t1 = max;
    for i in 0..3 {
        let (lo, hi) = (b[i] as f64, b[i + 3] as f64);
        if d[i].abs() < 1e-12 {
            if o[i] < lo || o[i] > hi {
                return false;
            }
            continue;
        }
        let a = (lo - o[i]) / d[i];
        let bb = (hi - o[i]) / d[i];
        t0 = t0.max(a.min(bb));
        t1 = t1.min(a.max(bb));
    }
    t0 <= t1
}
