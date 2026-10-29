//! Noise in 32-bit integers only (wrapping arithmetic, shifts, integer division), so the CPU and
//! the GPU compute the same bits: WGSL has neither doubles nor 64-bit integers, and float
//! results may differ between GPUs. `shaders/terrain.wgsl` is a line-by-line copy; keep them
//! in step (the `gpu_check` test compares them).
//!
//! Positions are in 1/16 block (`Q4`), noise values, weights and fractions in 1/4096 (`Q12`).

/// 1.0 in Q12.
pub const ONE: i32 = 4096;

#[inline]
pub fn fmix(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^ (h >> 16)
}

#[inline]
pub fn hash(seed: u32, x: i32, y: i32, z: i32) -> u32 {
    fmix(seed ^ fmix(x as u32 ^ fmix(y as u32 ^ fmix(z as u32 ^ 0x27D4_EB2F))))
}

/// A 32-bit seed for one field of a 64-bit world seed.
pub fn seed32(seed: u64, salt: u64) -> u32 {
    let s = seed ^ salt;
    fmix(s as u32 ^ fmix((s >> 32) as u32))
}

/// Floor division and the remainder in `0..d` (`d > 0`).
#[inline]
pub fn div_floor(p: i32, d: i32) -> (i32, i32) {
    let mut q = p / d;
    if p % d != 0 && p < 0 {
        q -= 1;
    }
    (q, p - q * d)
}

/// Cell and Q12 fraction of position `p` (Q4) on a lattice of `scale` (Q4, at most 2^20).
#[inline]
fn lattice(p: i32, scale: i32) -> (i32, i32) {
    let s = scale.clamp(1, (1 << 20) - 1);
    let (cell, r) = div_floor(p, s);
    (cell, ((r as u32 * 4096) / s as u32) as i32)
}

#[inline]
fn fade(t: i32) -> i32 {
    let t3 = (((t * t) >> 12) * t) >> 12;
    (t3 * (((t * (t * 6 - 15 * ONE)) >> 12) + 10 * ONE)) >> 12
}

#[inline]
fn lerp(a: i32, b: i32, t: i32) -> i32 {
    a + (((b - a) * t) >> 12)
}

#[inline]
fn grad2(h: u32, x: i32, y: i32) -> i32 {
    match h & 7 {
        0 => x + y,
        1 => x - y,
        2 => -x + y,
        3 => -x - y,
        4 => x,
        5 => -x,
        6 => y,
        _ => -y,
    }
}

#[inline]
fn grad3(h: u32, x: i32, y: i32, z: i32) -> i32 {
    match h % 12 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 => -y + z,
        10 => y - z,
        _ => -y - z,
    }
}

/// About -1..1 (Q12) at (`x`, `y`) (Q4), `scale` (Q4) per lattice cell.
pub fn perlin2(seed: u32, x: i32, y: i32, scale: i32) -> i32 {
    let (xi, xf) = lattice(x, scale);
    let (yi, yf) = lattice(y, scale);
    let (u, v) = (fade(xf), fade(yf));
    let g = |dx: i32, dy: i32| grad2(hash(seed, xi + dx, yi + dy, 0), xf - dx * ONE, yf - dy * ONE);
    let a = lerp(g(0, 0), g(1, 0), u);
    let b = lerp(g(0, 1), g(1, 1), u);
    (lerp(a, b, v) * 2867) >> 12
}

/// Like `perlin2` in three dimensions, with a scale per axis.
pub fn perlin3(seed: u32, x: i32, y: i32, z: i32, sx: i32, sy: i32, sz: i32) -> i32 {
    let (xi, xf) = lattice(x, sx);
    let (yi, yf) = lattice(y, sy);
    let (zi, zf) = lattice(z, sz);
    let (u, v, w) = (fade(xf), fade(yf), fade(zf));
    let g = |dx: i32, dy: i32, dz: i32| grad3(hash(seed, xi + dx, yi + dy, zi + dz), xf - dx * ONE, yf - dy * ONE, zf - dz * ONE);
    let x00 = lerp(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = lerp(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = lerp(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = lerp(g(0, 1, 1), g(1, 1, 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w)
}

/// Octaves of `perlin2`, each twice as fine and half as strong, about -1..1.
pub fn fbm(seed: u32, x: i32, z: i32, scale: i32, octaves: u32) -> i32 {
    let (mut sum, mut norm) = (0i32, 0i32);
    for o in 0..octaves.min(12) {
        let amp = ONE >> o;
        sum += (perlin2(seed.wrapping_add(o * 7919), x, z, (scale >> o).max(1)) * amp) >> 12;
        norm += amp;
    }
    if norm == 0 { 0 } else { sum * ONE / norm }
}

/// 0 at `from`, 1 at `to` (Q12), linear in between; `from > to` gives a falling edge.
pub fn ramp(v: i32, from: i32, to: i32) -> i32 {
    if to == from {
        return if v >= to { ONE } else { 0 };
    }
    // Parameters are within +-2^18 (`q12` clamps), so this stays in 32 bits.
    ((v - from).clamp(-(1 << 18), 1 << 18) * ONE / (to - from)).clamp(0, ONE)
}

/// To Q12 (and Q4 below), rounded, for parameters given as decimals; out of range (an open
/// range's infinity) saturates at +-2^18, far past any field.
pub fn q12(v: f64) -> i32 {
    (v * 4096.0).round().clamp(-(1 << 18) as f64, (1 << 18) as f64) as i32
}

pub fn q4(v: f64) -> i32 {
    (v * 16.0).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

/// The world's noise fields at Q4 positions, Q12 values.
#[derive(Clone, Copy)]
pub struct Fields {
    pub seed: u64,
}

impl Fields {
    pub fn fbm(&self, salt: u64, x: i32, z: i32, scale: i32, octaves: u32) -> i32 {
        fbm(seed32(self.seed, salt), x, z, scale, octaves)
    }

    pub fn ridge(&self, salt: u64, x: i32, z: i32, scale: i32) -> i32 {
        ONE - (self.fbm(salt, x, z, scale, 4) * 2).abs().min(ONE)
    }

    pub fn continent(&self, x: i32, z: i32) -> i32 {
        ((self.fbm(0x11, x, z, 900 * 16, 4) * 6554) >> 12) + 614
    }

    pub fn temperature(&self, x: i32, z: i32) -> i32 {
        (self.fbm(0x22, x, z, 1100 * 16, 3) * 5734) >> 12
    }

    pub fn humidity(&self, x: i32, z: i32) -> i32 {
        (self.fbm(0x33, x, z, 800 * 16, 3) * 5734) >> 12
    }
}

/// The terrain presets: height (Q4) at (`x`, `z`) (Q4); parameters in Q4 (heights, sizes).
pub mod shape {
    use super::*;

    pub fn continents(f: &Fields, x: i32, z: i32, depth: i32) -> i32 {
        ((f.continent(x, z) * 5 / 2).clamp(-ONE, ONE) * depth) >> 12
    }

    pub fn hills(f: &Fields, x: i32, z: i32, height: i32, size: i32) -> i32 {
        (f.fbm(0x55, x, z, size, 4) * height) >> 12
    }

    pub fn mountains(f: &Fields, x: i32, z: i32, height: i32, width: i32) -> i32 {
        let r = f.ridge(0x66, x, z, width);
        ((((r * r) >> 12) * height) >> 12) + ((f.fbm(0x55, x, z, 60 * 16, 4) * height) >> 12) / 7
    }

    pub fn plateaus(f: &Fields, x: i32, z: i32, height: i32, size: i32, step: i32) -> i32 {
        let v = (ramp(f.fbm(0x77, x, z, size, 3), 0, 1024) * height) >> 12;
        let step = step.max(1);
        div_floor(v, step).0 * step
    }

    pub fn canyons(f: &Fields, x: i32, z: i32, depth: i32, width: i32) -> i32 {
        let r = f.ridge(0x88, x, z, width.min(1 << 20) * 40);
        -((ramp(r, 3994, ONE) * depth) >> 12)
    }

    pub fn dunes(f: &Fields, x: i32, z: i32, height: i32, wavelength: i32) -> i32 {
        let wl = wavelength.clamp(1, (1 << 18) - 1);
        let warp = (f.fbm(0x99, x, z, wl * 4, 2) * wl) >> 12;
        let t = div_floor(x + z / 2 + warp, wl).1 * ONE / wl;
        // Gentle windward slope, steep lee side.
        let profile = if t < 3277 { t * ONE / 3277 } else { (ONE - t) * ONE / 819 };
        (profile * height) >> 12
    }

    pub fn craters(f: &Fields, x: i32, z: i32, radius: i32, depth: i32, spacing: i32) -> i32 {
        let sp = spacing.clamp(16, 512 * 16);
        let (cx, cz) = (div_floor(x, sp).0, div_floor(z, sp).0);
        let seed = seed32(f.seed, 0xC7A7);
        let (bx, bz) = (x >> 4, z >> 4);
        let mut total = 0;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let h = hash(seed, cx + dx, 0, cz + dz);
                let h2 = fmix(h);
                if (h & 4095) as i32 > 2458 {
                    continue;
                }
                // In blocks from here on: squares of Q4 distances overflow 32 bits.
                let r = ((radius.clamp(16, 512 * 16) * (2048 + ((h >> 12) & 4095) as i32)) >> 12) >> 4;
                let r2 = (r * r).max(1);
                let px = ((cx + dx) * sp + (((h2 & 4095) as i32 * sp) >> 12)) >> 4;
                let pz = ((cz + dz) * sp + ((((h2 >> 12) & 4095) as i32 * sp) >> 12)) >> 4;
                let d2 = (bx - px) * (bx - px) + (bz - pz) * (bz - pz);
                if d2 >= r2 * 2 {
                    continue;
                }
                let d = ((d2 as u32 * 4096) / r2 as u32) as i32;
                if d < ONE {
                    total -= ((ONE - d) * depth) >> 12;
                } else if d < 6554 {
                    let rim = (ONE - (d - 5325).abs() * ONE / 1229).max(0);
                    total += ((rim * depth) >> 12) / 4;
                }
            }
        }
        total
    }
}
