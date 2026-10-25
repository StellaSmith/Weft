//! Gradient noise built only from integer hashing and + - * floor, so it gives bit-identical results
//! on every platform (no libm transcendental functions). The engine's terrain and mods share it,
//! so a feature written in a mod sees the same fields the engine does.

#[inline]
pub fn hash(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    let mut h = seed ^ (x as u64).wrapping_mul(0x9E3779B97F4A7C15);
    h ^= (y as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
    h ^= (z as u64).wrapping_mul(0x165667B19E3779F9);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D049BB133111EB);
    h ^ (h >> 31)
}

/// Uniform in [0, 1).
#[inline]
pub fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

#[inline]
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn grad2(h: u64, x: f64, y: f64) -> f64 {
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
fn grad3(h: u64, x: f64, y: f64, z: f64) -> f64 {
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

/// Roughly in [-1, 1].
pub fn perlin2(seed: u64, x: f64, y: f64) -> f64 {
    let (xi, yi) = (x.floor(), y.floor());
    let (xf, yf) = (x - xi, y - yi);
    let (xi, yi) = (xi as i64, yi as i64);
    let (u, v) = (fade(xf), fade(yf));
    let g = |dx: i64, dy: i64| grad2(hash(seed, xi + dx, yi + dy, 0), xf - dx as f64, yf - dy as f64);
    let a = g(0, 0) + u * (g(1, 0) - g(0, 0));
    let b = g(0, 1) + u * (g(1, 1) - g(0, 1));
    (a + v * (b - a)) * 0.7
}

pub fn perlin3(seed: u64, x: f64, y: f64, z: f64) -> f64 {
    let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
    let (xf, yf, zf) = (x - xi, y - yi, z - zi);
    let (xi, yi, zi) = (xi as i64, yi as i64, zi as i64);
    let (u, v, w) = (fade(xf), fade(yf), fade(zf));
    let g = |dx: i64, dy: i64, dz: i64| {
        grad3(hash(seed, xi + dx, yi + dy, zi + dz), xf - dx as f64, yf - dy as f64, zf - dz as f64)
    };
    let lerp = |a: f64, b: f64, t: f64| a + t * (b - a);
    let x00 = lerp(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = lerp(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = lerp(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = lerp(g(0, 1, 1), g(1, 1, 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w)
}

pub fn fbm2(seed: u64, x: f64, y: f64, octaves: u32) -> f64 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += perlin2(seed.wrapping_add(o as u64 * 7919), x * freq, y * freq) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

/// 0 at `from`, 1 at `to`, linear in between; `from > to` gives a falling edge.
pub fn ramp(v: f64, from: f64, to: f64) -> f64 {
    ((v - from) / (to - from)).clamp(0.0, 1.0)
}

/// Noise fields of one seed, for terrain features, masks and biome rules.
#[derive(Clone, Copy)]
pub struct Fields {
    seed: u64,
}

impl Fields {
    pub fn new(seed: u64) -> Fields {
        Fields { seed }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// About -1..1, `scale` blocks per bump.
    pub fn fbm(&self, salt: u64, x: f64, z: f64, scale: f64, octaves: u32) -> f64 {
        fbm2(self.seed ^ salt, x / scale, z / scale, octaves)
    }

    /// 1 along the zero lines of a noise field, 0 away from them: ridges, canyons, rivers.
    pub fn ridge(&self, salt: u64, x: f64, z: f64, scale: f64) -> f64 {
        1.0 - (self.fbm(salt, x, z, scale, 4) * 2.0).abs().min(1.0)
    }

    /// Land vs ocean, about -1.5..1.5; above 0.1 is inland.
    pub fn continent(&self, x: f64, z: f64) -> f64 {
        self.fbm(0x11, x, z, 900.0, 4) * 1.6 + 0.15
    }

    pub fn temperature(&self, x: f64, z: f64) -> f64 {
        self.fbm(0x22, x, z, 1100.0, 3) * 1.4
    }

    pub fn humidity(&self, x: f64, z: f64) -> f64 {
        self.fbm(0x33, x, z, 800.0, 3) * 1.4
    }
}

/// The terrain presets (`weft_sdk::terrain::preset`): the height each adds at a point. Each is a
/// few lines; copy one into a `Feature::new` to make a new kind.
pub mod shape {
    use super::*;

    /// Oceans and land masses: `depth` blocks above or below the base height.
    pub fn continents(f: &Fields, x: f64, z: f64, depth: f64) -> f64 {
        (f.continent(x, z) * 2.5).clamp(-1.0, 1.0) * depth
    }

    /// Rolling hills.
    pub fn hills(f: &Fields, x: f64, z: f64, height: f64, size: f64) -> f64 {
        f.fbm(0x55, x, z, size, 4) * height
    }

    /// Mountain ranges along ridge lines `width` blocks apart, with rough detail on top.
    pub fn mountains(f: &Fields, x: f64, z: f64, height: f64, width: f64) -> f64 {
        let r = f.ridge(0x66, x, z, width);
        r * r * height + f.fbm(0x55, x, z, 60.0, 4) * height / 7.0
    }

    /// Flat-topped mesas `height` blocks high in terraces of `step`.
    pub fn plateaus(f: &Fields, x: f64, z: f64, height: f64, size: f64, step: f64) -> f64 {
        let v = ramp(f.fbm(0x77, x, z, size, 3), 0.0, 0.25) * height;
        (v / step).floor() * step
    }

    /// Winding cuts `depth` deep and about `width` wide; below sea level they fill with water.
    pub fn canyons(f: &Fields, x: f64, z: f64, depth: f64, width: f64) -> f64 {
        let r = f.ridge(0x88, x, z, width * 40.0);
        let edge = 1.0 - 1.0 / 40.0;
        -ramp(r, edge, 1.0) * depth
    }

    /// Parallel sand waves `wavelength` blocks apart.
    pub fn dunes(f: &Fields, x: f64, z: f64, height: f64, wavelength: f64) -> f64 {
        let warp = f.fbm(0x99, x, z, wavelength * 4.0, 2) * wavelength;
        let t = ((x + z * 0.5 + warp) / wavelength).rem_euclid(1.0);
        // Gentle windward slope, steep lee side.
        let profile = if t < 0.8 { t / 0.8 } else { (1.0 - t) / 0.2 };
        profile * height
    }

    /// Bowls with a raised rim, one per `spacing`-block cell at most.
    pub fn craters(f: &Fields, x: f64, z: f64, radius: f64, depth: f64, spacing: f64) -> f64 {
        let (cx, cz) = ((x / spacing).floor(), (z / spacing).floor());
        let mut total = 0.0;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let h = hash(f.seed() ^ 0xC7A7, cx as i64 + dx, 0, cz as i64 + dz);
                if unit(h) > 0.6 {
                    continue;
                }
                let r = radius * (0.5 + unit(h >> 7));
                let px = (cx + dx as f64 + unit(h >> 17)) * spacing;
                let pz = (cz + dz as f64 + unit(h >> 29)) * spacing;
                let d = ((x - px) * (x - px) + (z - pz) * (z - pz)) / (r * r);
                if d < 1.0 {
                    total -= (1.0 - d) * depth;
                } else if d < 1.6 {
                    total += (1.0 - (d - 1.3).abs() / 0.3).max(0.0) * depth * 0.25;
                }
            }
        }
        total
    }
}
