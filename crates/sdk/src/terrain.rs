//! Terrain from presets: a base height plus height features, biomes picked by climate, surface
//! layers per biome, water and caves. Features are plain functions, so a mod can ship its own next
//! to the presets here. Like any generator, everything is a function of (seed, position).
//!
//! ```ignore
//! static TERRAIN: LazyLock<Terrain> = LazyLock::new(|| {
//!     Terrain::new(&STONE, 64.0)
//!         .sea(63, &WATER)
//!         .feature(preset::continents(14.0))
//!         .feature(preset::hills(8.0, 120.0))
//!         .feature(preset::mountains(60.0, 160.0).mask(Mask::noise(500.0, 0.2, 0.5)).named("mountains"))
//!         .biome(BiomeRule::new(&MOUNTAINS).feature("mountains", 0.5, 9.0).surface(&STONE, &STONE, 1))
//!         .biome(BiomeRule::new(&DESERT).temperature(0.35, 9.0).surface(&SAND, &SAND, 3))
//!         .biome(BiomeRule::new(&PLAINS).surface(&GRASS, &DIRT, 3))
//!         .caves(Caves::new(&LAVA, i32::MIN, 11))
//! });
//! r.dimension("overworld", &OVERWORLD).generator(|g| {
//!     TERRAIN.generate(g);
//! });
//! ```

use std::sync::Arc;

use crate::noise::*;
use crate::{Biome, Block, ColumnCache, Gen};

/// Noise fields of one seed, for features, masks and biome rules.
pub struct Fields {
    seed: u64,
}

impl Fields {
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

/// 0 at `from`, 1 at `to`, linear in between; `from > to` gives a falling edge.
fn ramp(v: f64, from: f64, to: f64) -> f64 {
    ((v - from) / (to - from)).clamp(0.0, 1.0)
}

/// Where a feature applies, 0..1.
#[derive(Clone)]
pub enum Mask {
    Noise { salt: u64, scale: f64, octaves: u32, from: f64, to: f64 },
    Continent(f64, f64),
    Temperature(f64, f64),
    Humidity(f64, f64),
    All(Vec<Mask>),
    /// 1 minus the mask: a biome's climate taking a feature away (flat tundra).
    Not(Box<Mask>),
}

impl Mask {
    /// Its own noise field: patches of about `scale` blocks, fading in between `from` and `to`.
    pub fn noise(scale: f64, from: f64, to: f64) -> Mask {
        Mask::Noise { salt: scale.to_bits(), scale, octaves: 3, from, to }
    }

    pub fn salt(self, salt: u64) -> Mask {
        match self {
            Mask::Noise { scale, octaves, from, to, .. } => Mask::Noise { salt, scale, octaves, from, to },
            m => m,
        }
    }

    pub fn not(self) -> Mask {
        Mask::Not(Box::new(self))
    }

    pub fn and(self, other: Mask) -> Mask {
        match self {
            Mask::All(mut v) => {
                v.push(other);
                Mask::All(v)
            }
            m => Mask::All(vec![m, other]),
        }
    }

    pub fn value(&self, f: &Fields, x: f64, z: f64) -> f64 {
        match self {
            Mask::Noise { salt, scale, octaves, from, to } => ramp(f.fbm(*salt, x, z, *scale, *octaves), *from, *to),
            Mask::Continent(a, b) => ramp(f.continent(x, z), *a, *b),
            Mask::Temperature(a, b) => ramp(f.temperature(x, z), *a, *b),
            Mask::Humidity(a, b) => ramp(f.humidity(x, z), *a, *b),
            Mask::All(v) => v.iter().fold(1.0, |acc, m| acc * m.value(f, x, z)),
            Mask::Not(m) => 1.0 - m.value(f, x, z),
        }
    }
}

type HeightFn = dyn Fn(&Fields, f64, f64) -> f64 + Send + Sync;

/// Adds to the height of every column, scaled by its mask.
pub struct Feature {
    name: String,
    height: Box<HeightFn>,
    mask: Option<Mask>,
}

impl Feature {
    pub fn new(height: impl Fn(&Fields, f64, f64) -> f64 + Send + Sync + 'static) -> Feature {
        Feature { name: String::new(), height: Box::new(height), mask: None }
    }

    /// Biome rules can ask for the mask strength by this name.
    pub fn named(mut self, name: &str) -> Feature {
        self.name = name.to_string();
        self
    }

    pub fn mask(mut self, mask: Mask) -> Feature {
        self.mask = Some(mask);
        self
    }
}

/// Ready-made features. Each is a few lines; copy one to make a new kind.
pub mod preset {
    use super::*;

    /// Oceans and land masses: `depth` blocks above or below the base height.
    pub fn continents(depth: f64) -> Feature {
        Feature::new(move |f, x, z| (f.continent(x, z) * 2.5).clamp(-1.0, 1.0) * depth)
    }

    /// Rolling hills.
    pub fn hills(height: f64, size: f64) -> Feature {
        Feature::new(move |f, x, z| f.fbm(0x55, x, z, size, 4) * height)
    }

    /// Mountain ranges along ridge lines `width` blocks apart, with rough detail on top.
    pub fn mountains(height: f64, width: f64) -> Feature {
        Feature::new(move |f, x, z| {
            let r = f.ridge(0x66, x, z, width);
            r * r * height + f.fbm(0x55, x, z, 60.0, 4) * height / 7.0
        })
    }

    /// Flat-topped mesas `height` blocks high in terraces of `step`.
    pub fn plateaus(height: f64, size: f64, step: f64) -> Feature {
        Feature::new(move |f, x, z| {
            let v = ramp(f.fbm(0x77, x, z, size, 3), 0.0, 0.25) * height;
            (v / step).floor() * step
        })
    }

    /// Winding cuts `depth` deep and about `width` wide; below sea level they fill with water.
    pub fn canyons(depth: f64, width: f64) -> Feature {
        Feature::new(move |f, x, z| {
            let r = f.ridge(0x88, x, z, width * 40.0);
            let edge = 1.0 - 1.0 / 40.0;
            -ramp(r, edge, 1.0) * depth
        })
    }

    /// Parallel sand waves `wavelength` blocks apart.
    pub fn dunes(height: f64, wavelength: f64) -> Feature {
        Feature::new(move |f, x, z| {
            let warp = f.fbm(0x99, x, z, wavelength * 4.0, 2) * wavelength;
            let t = ((x + z * 0.5 + warp) / wavelength).rem_euclid(1.0);
            // Gentle windward slope, steep lee side.
            let profile = if t < 0.8 { t / 0.8 } else { (1.0 - t) / 0.2 };
            profile * height
        })
    }

    /// Bowls with a raised rim, one per `spacing`-block cell at most.
    pub fn craters(radius: f64, depth: f64, spacing: f64) -> Feature {
        Feature::new(move |f, x, z| {
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
        })
    }
}

/// Surface layers and the climate a biome needs. Rules are tried in order; the first match wins.
pub struct BiomeRule {
    biome: &'static Biome,
    temperature: (f64, f64),
    humidity: (f64, f64),
    continent: (f64, f64),
    height: (i32, i32),
    feature: Option<(String, f64, f64)>,
    top: Option<&'static Block>,
    filler: Option<&'static Block>,
    depth: i32,
    under: Option<(&'static Block, i32)>,
    deep_top: Option<(&'static Block, i32)>,
    bare_above: Option<i32>,
    ice: Option<&'static Block>,
}

impl BiomeRule {
    pub fn new(biome: &'static Biome) -> BiomeRule {
        let any = (f64::NEG_INFINITY, f64::INFINITY);
        BiomeRule {
            biome,
            temperature: any,
            humidity: any,
            continent: any,
            height: (i32::MIN, i32::MAX),
            feature: None,
            top: None,
            filler: None,
            depth: 3,
            under: None,
            deep_top: None,
            bare_above: None,
            ice: None,
        }
    }

    /// Climate ranges are `min <= v < max`; fields are about -1.4..1.4.
    pub fn temperature(mut self, min: f64, max: f64) -> Self {
        self.temperature = (min, max);
        self
    }
    pub fn humidity(mut self, min: f64, max: f64) -> Self {
        self.humidity = (min, max);
        self
    }
    pub fn continent(mut self, min: f64, max: f64) -> Self {
        self.continent = (min, max);
        self
    }
    /// Surface height range, inclusive.
    pub fn height(mut self, min: i32, max: i32) -> Self {
        self.height = (min, max);
        self
    }
    /// Where the named feature's mask is in `min..max`.
    pub fn feature(mut self, name: &str, min: f64, max: f64) -> Self {
        self.feature = Some((name.to_string(), min, max));
        self
    }
    /// Top block, then `depth` blocks of filler, then the terrain's stone.
    pub fn surface(mut self, top: &'static Block, filler: &'static Block, depth: i32) -> Self {
        (self.top, self.filler, self.depth) = (Some(top), Some(filler), depth);
        self
    }
    /// The lowest `layers` of the filler become `block` (sandstone under sand).
    pub fn under(mut self, block: &'static Block, layers: i32) -> Self {
        self.under = Some((block, layers));
        self
    }
    /// Top block for columns lower than `below` (gravel on deep sea floors).
    pub fn deep_top(mut self, block: &'static Block, below: i32) -> Self {
        self.deep_top = Some((block, below));
        self
    }
    /// Plain stone above this height (bare peaks).
    pub fn bare_above(mut self, y: i32) -> Self {
        self.bare_above = Some(y);
        self
    }
    /// The water surface freezes into `block`.
    pub fn frozen(mut self, block: &'static Block) -> Self {
        self.ice = Some(block);
        self
    }
}

/// Two-noise tunnels plus large caverns. Below `lava_to` (and from `lava_from`) some layers of
/// height hold lava lakes (see `lake`).
pub struct Caves {
    lava: &'static Block,
    lava_from: i32,
    lava_to: i32,
}

impl Caves {
    pub fn new(lava: &'static Block, lava_from: i32, lava_to: i32) -> Caves {
        Caves { lava, lava_from, lava_to }
    }

    /// Cave fields for the cube, sampled every 4 blocks on a grid aligned to the world and
    /// trilinearly interpolated: whether a tunnel passes, and how far inside a cavern (> 0) the
    /// block is.
    fn fields(&self, seed: u64, ox: i32, oy: i32, oz: i32) -> Vec<(bool, f32)> {
        const G: usize = 5;
        let mut grid = [[[(0f64, 0f64); G]; G]; G];
        for (gy, row) in grid.iter_mut().enumerate() {
            for (gz, line) in row.iter_mut().enumerate() {
                for (gx, v) in line.iter_mut().enumerate() {
                    *v = cave_node(seed, ox + gx as i32 * 4, oy + gy as i32 * 4, oz + gz as i32 * 4);
                }
            }
        }
        let mut out = vec![(false, 0f32); 4096];
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let (t, c) = interpolate(|dy, dz, dx| grid[y / 4 + dy][z / 4 + dz][x / 4 + dx], x % 4, y % 4, z % 4);
                    out[(y * 16 + z) * 16 + x] = (t > 0.0, c as f32);
                }
            }
        }
        out
    }
}

/// Trilinear blend of grid nodes (`node(dy, dz, dx)`) at offset (x, y, z) inside a 4-block cell.
fn interpolate(node: impl Fn(usize, usize, usize) -> (f64, f64), x: usize, y: usize, z: usize) -> (f64, f64) {
    let (fx, fy, fz) = (x as f64 / 4.0, y as f64 / 4.0, z as f64 / 4.0);
    let l = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let field = |k: usize| {
        let c = |dy, dz, dx| {
            let v = node(dy, dz, dx);
            if k == 0 { v.0 } else { v.1 }
        };
        l(l(l(c(0, 0, 0), c(0, 0, 1), fx), l(c(0, 1, 0), c(0, 1, 1), fx), fz), l(l(c(1, 0, 0), c(1, 0, 1), fx), l(c(1, 1, 0), c(1, 1, 1), fx), fz), fy)
    };
    (field(0), field(1))
}

/// Whether a cave takes a block `depth` blocks under the smoothed surface. Near the surface a
/// cavern needs to be ever deeper inside its noise, so its ceiling domes over instead of breaking
/// through; where `cavern_roof` opens (`closed` < 1) it widens towards the surface into a pit.
fn carves(tunnel: bool, cavern: f32, closed: f32, depth: f32) -> bool {
    let need = closed * 0.25 * (1.0 - depth / 12.0).max(0.0) - (1.0 - closed) * PIT * (1.0 - depth / 24.0).max(0.0);
    tunnel || cavern > need
}

/// What a cave holds at height `wy` instead of air: in about half of the 64-block bands of height
/// a lake crosses every cave at one level, 4 blocks of lava on 2 of stone, so lakes are flat,
/// span the whole cave and never hang over open space. `Some(true)` lava, `Some(false)` stone.
fn lake(seed: u64, wy: i32) -> Option<bool> {
    let band = wy.div_euclid(64);
    let h = hash(seed ^ 0x1A7A, 0, band as i64, 0);
    if h & 1 == 0 {
        return None;
    }
    let level = band * 64 + 16 + ((h >> 8) % 32) as i32;
    match level - wy {
        1..=4 => Some(true),
        5..=6 => Some(false),
        _ => None,
    }
}

/// One column's result, for decorations and spawn search.
pub struct Column {
    pub height: i32,
    pub biome: &'static Biome,
    rule: usize,
}

pub struct Terrain {
    stone: &'static Block,
    base: f64,
    sea: Option<(i32, &'static Block)>,
    features: Vec<Feature>,
    biomes: Vec<BiomeRule>,
    caves: Option<Caves>,
    /// Biomes of cubes deep underground: (biome, depth under the surface, patch size, threshold).
    cave_biomes: Vec<(&'static Biome, i32, f64, f64)>,
    cache: ColumnCache<Vec<Column>>,
    /// Column heights every 4 blocks over a cube's columns and its +X/+Z edge (5 x 5): caves
    /// measure depth from this smoothed surface, so steep slopes do not leave single-column
    /// pillars.
    corners: ColumnCache<[f32; 25]>,
}

impl Terrain {
    pub fn new(stone: &'static Block, base_height: f64) -> Terrain {
        Terrain {
            stone,
            base: base_height,
            sea: None,
            features: Vec::new(),
            biomes: Vec::new(),
            caves: None,
            cave_biomes: Vec::new(),
            cache: ColumnCache::new(16384),
            corners: ColumnCache::new(16384),
        }
    }

    pub fn sea(mut self, level: i32, water: &'static Block) -> Self {
        self.sea = Some((level, water));
        self
    }

    pub fn feature(mut self, f: Feature) -> Self {
        self.features.push(f);
        self
    }

    pub fn biome(mut self, rule: BiomeRule) -> Self {
        self.biomes.push(rule);
        self
    }

    /// A biome in 3D: whole cubes at least `depth` blocks under the surface where a 3D noise of
    /// patches about `size` blocks across is above `threshold` (-1..1). Biomes are kept per
    /// column of each cube, so underground ones change every 16 blocks of height at most.
    pub fn cave_biome(mut self, biome: &'static Biome, depth: i32, size: f64, threshold: f64) -> Self {
        self.cave_biomes.push((biome, depth, size, threshold));
        self
    }

    pub fn caves(mut self, caves: Caves) -> Self {
        self.caves = Some(caves);
        self
    }

    pub fn sea_level(&self) -> Option<i32> {
        self.sea.map(|s| s.0)
    }

    /// Height and biome of the block column at (`x`, `z`).
    pub fn column(&self, seed: u64, x: i32, z: i32) -> Column {
        let f = Fields { seed };
        let (fx, fz) = (x as f64, z as f64);
        let mut h = self.base;
        let mut strengths: Vec<(&str, f64)> = Vec::new();
        for feat in &self.features {
            let w = feat.mask.as_ref().map_or(1.0, |m| m.value(&f, fx, fz));
            if !feat.name.is_empty() {
                strengths.push((&feat.name, w));
            }
            if w > 0.0 {
                h += w * (feat.height)(&f, fx, fz);
            }
        }
        let height = h.floor() as i32;
        let (temp, humid, cont) = (f.temperature(fx, fz), f.humidity(fx, fz), f.continent(fx, fz));
        let inside = |v: f64, r: (f64, f64)| v >= r.0 && v < r.1;
        let rule = self
            .biomes
            .iter()
            .position(|b| {
                height >= b.height.0
                    && height <= b.height.1
                    && inside(temp, b.temperature)
                    && inside(humid, b.humidity)
                    && inside(cont, b.continent)
                    && b.feature.as_ref().is_none_or(|(name, lo, hi)| strengths.iter().find(|s| s.0 == name).is_some_and(|s| s.1 > *lo && s.1 <= *hi))
            })
            .unwrap_or(self.biomes.len().saturating_sub(1));
        Column { height, biome: self.biomes.get(rule).map_or(&EMPTY, |b| b.biome), rule }
    }

    /// Whether caves took the block at the surface of column (`x`, `z`): decorations placed from
    /// outside the cube (trees) and spawn search check it so they do not float over pits.
    pub fn surface_carved(&self, seed: u64, x: i32, z: i32) -> bool {
        if self.caves.is_none() {
            return false;
        }
        let h = self.column(seed, x, z).height;
        if self.sea.is_some_and(|s| h <= s.0) {
            return false;
        }
        let (gx, gy, gz) = (x.div_euclid(4) * 4, h.div_euclid(4) * 4, z.div_euclid(4) * 4);
        let (t, c) = interpolate(|dy, dz, dx| cave_node(seed, gx + dx as i32 * 4, gy + dy as i32 * 4, gz + dz as i32 * 4), (x - gx) as usize, (h - gy) as usize, (z - gz) as usize);
        let corner = |dx: i32, dz: i32| self.column(seed, gx + dx * 4, gz + dz * 4).height as f32;
        let (fx, fz) = ((x - gx) as f32 / 4.0, (z - gz) as f32 / 4.0);
        let smooth = (corner(0, 0) * (1.0 - fx) + corner(1, 0) * fx) * (1.0 - fz) + (corner(0, 1) * (1.0 - fx) + corner(1, 1) * fx) * fz;
        carves(t > 0.0, c as f32, cavern_roof(seed, x, z), smooth - h as f32)
    }

    /// Smoothed surface height at local column (x, z) from the cube's corner grid.
    fn smooth(corners: &[f32; 25], x: usize, z: usize) -> f32 {
        let (cx, cz, fx, fz) = (x / 4, z / 4, (x % 4) as f32 / 4.0, (z % 4) as f32 / 4.0);
        let c = |dx: usize, dz: usize| corners[(cz + dz) * 5 + cx + dx];
        (c(0, 0) * (1.0 - fx) + c(1, 0) * fx) * (1.0 - fz) + (c(0, 1) * (1.0 - fx) + c(1, 1) * fx) * fz
    }

    /// Fills the cube with stone, surface layers, water and caves and sets its biomes. Returns the
    /// columns for decorating, or `None` when the cube is above everything.
    pub fn generate(&self, g: &mut Gen) -> Option<Arc<Vec<Column>>> {
        let seed = g.seed();
        let [ox, oy, oz] = g.origin();
        let [cx, _, cz] = g.cube();
        let cols = self.cache.get(seed, cx, cz, || {
            let mut cols = Vec::with_capacity(256);
            for z in 0..16 {
                for x in 0..16 {
                    cols.push(self.column(seed, ox + x, oz + z));
                }
            }
            cols
        });
        for (i, c) in cols.iter().enumerate() {
            let (x, z) = ((ox + (i % 16) as i32) as f64, (oz + (i / 16) as i32) as f64);
            let mid = (oy + 8) as f64;
            let deep = self.cave_biomes.iter().enumerate().find(|&(k, &(_, depth, size, threshold))| {
                oy + 15 < c.height - depth && perlin3(seed ^ 0xCB10 ^ k as u64, x / size, mid / size, z / size) > threshold
            });
            g.biomes()[i] = deep.map_or(c.biome, |(_, b)| b.0).id() as u16;
        }
        let max_h = cols.iter().map(|c| c.height).max().unwrap_or(0);
        let sea = self.sea.map_or(i32::MIN, |s| s.0);
        if oy > max_h.max(sea) + 16 {
            return None;
        }
        // Decided per block from its column alone, so neighbouring cubes agree: tunnels may open
        // onto the surface, caverns close up under it except where `cavern_roof` lets them out.
        let cave = match &self.caves {
            Some(c) if oy <= max_h => Some(c.fields(seed, ox, oy, oz)),
            _ => None,
        };
        let corners = self.corners.get(seed, cx, cz, || std::array::from_fn(|i| self.column(seed, ox + (i % 5) as i32 * 4, oz + (i / 5) as i32 * 4).height as f32));
        let stone = self.stone.id();
        let water = self.sea.map_or(0, |s| s.1.id());
        for z in 0..16usize {
            for x in 0..16usize {
                let col = &cols[z * 16 + x];
                let h = col.height;
                let rule = self.biomes.get(col.rule);
                let bare = rule.and_then(|r| r.bare_above).is_some_and(|y| h > y);
                let mut top = rule.and_then(|r| r.top).map_or(stone, |b| b.id());
                if let Some((b, below)) = rule.and_then(|r| r.deep_top) {
                    if h < below {
                        top = b.id();
                    }
                }
                let filler = rule.and_then(|r| r.filler).map_or(stone, |b| b.id());
                let depth = rule.map_or(0, |r| r.depth);
                let under = rule.and_then(|r| r.under);
                let ice = rule.and_then(|r| r.ice).map(|b| b.id());
                let closed = if cave.is_some() { cavern_roof(seed, ox + x as i32, oz + z as i32) } else { 0.0 };
                let smooth = Self::smooth(&corners, x, z);
                // Keeps a seal under water so seas and canyons do not drain into caves.
                let carved = |ly: usize| {
                    let Some(cave) = &cave else { return false };
                    let wy = oy + ly as i32;
                    let (tunnel, cavern) = cave[(ly * 16 + z) * 16 + x];
                    wy <= h && !(h <= sea && wy > h - 4) && carves(tunnel, cavern, closed, smooth - wy as f32)
                };
                for y in 0..16usize {
                    let wy = oy + y as i32;
                    let mut s = if wy > h {
                        if wy <= sea {
                            if wy == sea { ice.unwrap_or(water) } else { water }
                        } else {
                            0
                        }
                    } else if bare {
                        stone
                    } else if wy == h {
                        top
                    } else if wy >= h - depth {
                        match under {
                            Some((b, layers)) if wy < h - depth + layers => b.id(),
                            _ => filler,
                        }
                    } else {
                        stone
                    };
                    if let Some(c) = self.caves.as_ref().filter(|_| carved(y)) {
                        s = match lake(seed, wy).filter(|_| (c.lava_from..c.lava_to).contains(&wy)) {
                            Some(true) => c.lava.id(),
                            Some(false) => stone,
                            None => 0,
                        };
                    }
                    g.set(x, y, z, s);
                }
            }
        }
        Some(cols)
    }
}

/// Cave fields at a grid node (every 4 blocks): tunnels, caverns.
fn cave_node(seed: u64, x: i32, y: i32, z: i32) -> (f64, f64) {
    let (x, y, z) = (x as f64, y as f64, z as f64);
    let a = perlin3(seed ^ 0xC1, x / 48.0, y / 32.0, z / 48.0);
    let b = perlin3(seed ^ 0xC2, x / 48.0, y / 32.0, z / 48.0);
    // Two thin noise zero-crossings intersect into winding tunnels.
    let spaghetti = 0.08 - (a.abs() + b.abs());
    let cheese = perlin3(seed ^ 0xC3, x / 90.0, y / 50.0, z / 90.0) - 0.42;
    (spaghetti, cheese)
}

/// 1 where caverns stay under a roof, falling to 0 in patches where they may open into wide pits
/// like vanilla's cave entrances.
const PIT: f32 = 0.12;

fn cavern_roof(seed: u64, x: i32, z: i32) -> f32 {
    let n = perlin2(seed ^ 0xC4, x as f64 / 120.0, z as f64 / 120.0);
    ramp(n, 0.4, 0.25) as f32
}

static EMPTY: Biome = Biome::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_ramp_both_ways() {
        assert_eq!(ramp(0.5, 0.0, 1.0), 0.5);
        assert_eq!(ramp(-0.2, 0.0, -0.4), 0.5);
        assert_eq!(ramp(2.0, 0.0, 1.0), 1.0);
    }

    /// Caverns end under a roof, not at the bottom of the surface cube (that leaves flat stone
    /// slabs hanging over open space).
    #[test]
    fn caverns_keep_a_roof() {
        static S: Block = Block::new();
        let t = Terrain::new(&S, 64.0).feature(preset::hills(16.0, 110.0)).feature(preset::mountains(150.0, 260.0)).caves(Caves::new(&S, 1, 11));
        let (mut columns, mut breached) = (0, 0);
        for cx in -3..3 {
            for cz in -3..3 {
                let heights: Vec<i32> = (0..256).map(|i| t.column(7, cx * 16 + (i % 16) as i32, cz * 16 + (i / 16) as i32).height).collect();
                let top = heights.iter().max().unwrap() >> 4;
                let bottom = (heights.iter().min().unwrap() >> 4) - 1;
                // Air within the column, by height.
                let mut air = std::collections::HashMap::new();
                for cy in bottom..=top {
                    let mut blocks = vec![0u32; 4096];
                    let mut biomes = vec![0u16; 256];
                    let mut args = crate::abi::GenArgs { seed: 7, cube: [cx, cy, cz], blocks: blocks.as_mut_ptr(), biomes: biomes.as_mut_ptr() };
                    t.generate(Gen::wrap(&mut args));
                    for (i, &b) in blocks.iter().enumerate() {
                        if b == 0 {
                            air.insert((i % 256, cy * 16 + (i / 256) as i32), ());
                        }
                    }
                }
                for (i, &h) in heights.iter().enumerate() {
                    if cavern_roof(7, cx * 16 + (i % 16) as i32, cz * 16 + (i / 16) as i32) < 1.0 {
                        continue;
                    }
                    columns += 1;
                    // Open space in the 8 blocks under the surface means the cave reached up.
                    let open = (h - 8..h).filter(|&y| air.contains_key(&(i, y))).count();
                    if open >= 6 {
                        breached += 1;
                    }
                }
            }
        }
        assert!(breached * 50 < columns, "{breached} of {columns} columns open under the surface");
    }

    /// The point check trees use agrees with what generation carved.
    #[test]
    fn surface_carved_matches_generation() {
        static S: Block = Block::new();
        let t = Terrain::new(&S, 64.0).feature(preset::hills(16.0, 110.0)).caves(Caves::new(&S, 1, 11));
        let mut checked = 0;
        for cx in -4..4 {
            for cz in -4..4 {
                for i in 0..256 {
                    let (x, z) = (cx * 16 + (i % 16) as i32, cz * 16 + (i / 16) as i32);
                    let h = t.column(3, x, z).height;
                    let mut blocks = vec![0u32; 4096];
                    let mut biomes = vec![0u16; 256];
                    let mut args = crate::abi::GenArgs { seed: 3, cube: [cx, h >> 4, cz], blocks: blocks.as_mut_ptr(), biomes: biomes.as_mut_ptr() };
                    t.generate(Gen::wrap(&mut args));
                    let air = blocks[((h & 15) as usize * 16 + i / 16) * 16 + i % 16] == 0;
                    assert_eq!(air, t.surface_carved(3, x, z), "at {x} {z}");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 16384);
    }

    /// Lava lakes lie on stone, not in every open block between two heights (lava hanging
    /// over caves below).
    #[test]
    fn lava_lakes_rest_on_stone() {
        static S: Block = Block::new();
        static L: Block = Block::new();
        unsafe { (*L.cell()).store(7, std::sync::atomic::Ordering::Relaxed) };
        let t = Terrain::new(&S, 64.0).feature(preset::hills(16.0, 110.0)).caves(Caves::new(&L, i32::MIN, 11));
        let (mut lava, mut hanging) = (0, 0);
        for cx in -3..3 {
            for cz in -3..3 {
                let mut column: std::collections::HashMap<i32, Vec<u32>> = std::collections::HashMap::new();
                for cy in -32..1 {
                    let mut blocks = vec![0u32; 4096];
                    let mut biomes = vec![0u16; 256];
                    let mut args = crate::abi::GenArgs { seed: 9, cube: [cx, cy, cz], blocks: blocks.as_mut_ptr(), biomes: biomes.as_mut_ptr() };
                    t.generate(Gen::wrap(&mut args));
                    column.insert(cy, blocks);
                }
                let at = |i: usize, y: i32| column.get(&(y >> 4)).map(|b| b[((y & 15) as usize * 16 + i / 16) * 16 + i % 16]);
                for i in 0..256 {
                    for y in -511..16 {
                        if at(i, y) == Some(7) {
                            lava += 1;
                            if at(i, y - 1) == Some(0) {
                                hanging += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(lava > 0, "no lakes at all");
        assert_eq!(hanging, 0, "{hanging} of {lava} lava blocks over air");
    }

    #[test]
    fn first_matching_rule_wins() {
        static A: Biome = Biome::new();
        static B: Biome = Biome::new();
        static S: Block = Block::new();
        let t = Terrain::new(&S, 70.0).biome(BiomeRule::new(&A).height(i32::MIN, 60)).biome(BiomeRule::new(&B));
        assert!(std::ptr::eq(t.column(1, 0, 0).biome, &B));
        let t = Terrain::new(&S, 50.0).biome(BiomeRule::new(&A).height(i32::MIN, 60)).biome(BiomeRule::new(&B));
        assert!(std::ptr::eq(t.column(1, 0, 0).biome, &A));
    }
}
