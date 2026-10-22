//! Stateless overworld generator: every block of a cube is a function of (seed, position). Trees and
//! ore veins are anchored to grid cells; a cube asks every nearby cell what it would place, so the
//! result does not depend on generation order and cubes can be generated in parallel.

use voxide_sdk::*;

use crate::blocks::*;
use crate::noise::*;

pub const SEA: i32 = 63;

pub static PLAINS: Biome = Biome::new();
pub static FOREST: Biome = Biome::new();
pub static BIRCH_FOREST: Biome = Biome::new();
pub static TAIGA: Biome = Biome::new();
pub static SNOWY: Biome = Biome::new();
pub static DESERT: Biome = Biome::new();
pub static MOUNTAINS: Biome = Biome::new();
pub static OCEAN: Biome = Biome::new();
pub static BEACH: Biome = Biome::new();

pub fn register_biomes(r: &mut Registry) {
    r.biome("plains", &PLAINS, 0x8db360, 0x77ab2f, 0x3f76e4, 0.8, 0.4);
    r.biome("forest", &FOREST, 0x79c05a, 0x59ae30, 0x3f76e4, 0.7, 0.8);
    r.biome("birch_forest", &BIRCH_FOREST, 0x88bb67, 0x6ba941, 0x3f76e4, 0.6, 0.6);
    r.biome("taiga", &TAIGA, 0x86b783, 0x68a464, 0x3d57d6, 0.25, 0.8);
    r.biome("snowy_plains", &SNOWY, 0x80b497, 0x60a17b, 0x3938c9, 0.0, 0.5);
    r.biome("desert", &DESERT, 0xbfb755, 0xaea42a, 0x3f76e4, 2.0, 0.0);
    r.biome("mountains", &MOUNTAINS, 0x8ab689, 0x6da36b, 0x3f76e4, 0.2, 0.3);
    r.biome("ocean", &OCEAN, 0x8db360, 0x77ab2f, 0x3f76e4, 0.5, 0.5);
    r.biome("beach", &BEACH, 0x8db360, 0x77ab2f, 0x3f76e4, 0.8, 0.4);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum B {
    Plains,
    Forest,
    Birch,
    Taiga,
    Snowy,
    Desert,
    Mountains,
    Ocean,
    Beach,
}

impl B {
    fn handle(self) -> &'static Biome {
        match self {
            B::Plains => &PLAINS,
            B::Forest => &FOREST,
            B::Birch => &BIRCH_FOREST,
            B::Taiga => &TAIGA,
            B::Snowy => &SNOWY,
            B::Desert => &DESERT,
            B::Mountains => &MOUNTAINS,
            B::Ocean => &OCEAN,
            B::Beach => &BEACH,
        }
    }
}

pub struct Column {
    pub height: i32,
    pub biome: B,
}

pub fn column(seed: u64, x: i32, z: i32) -> Column {
    let (fx, fz) = (x as f64, z as f64);
    let cont = fbm2(seed ^ 0x11, fx / 900.0, fz / 900.0, 4) * 1.6 + 0.15;
    let temp = fbm2(seed ^ 0x22, fx / 1100.0, fz / 1100.0, 3) * 1.4;
    let humid = fbm2(seed ^ 0x33, fx / 800.0, fz / 800.0, 3) * 1.4;
    let mount = fbm2(seed ^ 0x44, fx / 500.0, fz / 500.0, 3);
    let detail = fbm2(seed ^ 0x55, fx / 60.0, fz / 60.0, 4);
    let ridge = 1.0 - (fbm2(seed ^ 0x66, fx / 160.0, fz / 160.0, 4) * 2.0).abs().min(1.0);

    let land = (cont * 2.5).clamp(-1.0, 1.0);
    let mut h = SEA as f64 + land * 14.0 + detail * 5.0;
    let mountain_w = ((mount - 0.15) * 3.0).clamp(0.0, 1.0) * ((cont - 0.05) * 4.0).clamp(0.0, 1.0);
    h += mountain_w * (ridge * ridge * 70.0 + detail * 10.0);
    let height = h.floor() as i32;

    let biome = if height < SEA - 1 {
        B::Ocean
    } else if height <= SEA + 1 && land < 0.25 {
        if temp < -0.45 { B::Snowy } else { B::Beach }
    } else if mountain_w > 0.45 {
        B::Mountains
    } else if temp < -0.45 {
        B::Snowy
    } else if temp < -0.15 {
        B::Taiga
    } else if temp > 0.35 && humid < 0.0 {
        B::Desert
    } else if humid > 0.25 {
        B::Forest
    } else if humid > 0.05 && temp < 0.1 {
        B::Birch
    } else {
        B::Plains
    };
    Column { height, biome }
}

fn tree_density(b: B) -> f64 {
    match b {
        B::Forest => 0.4,
        B::Birch => 0.45,
        B::Taiga => 0.45,
        B::Plains => 0.025,
        B::Mountains => 0.06,
        B::Snowy => 0.03,
        _ => 0.0,
    }
}

const TREE_CELL: i32 = 5;

#[derive(Clone, Copy)]
enum Tree {
    Oak,
    Birch,
    Spruce,
}

struct TreeAt {
    x: i32,
    y: i32,
    z: i32,
    kind: Tree,
    height: i32,
}

fn tree_in_cell(seed: u64, cx: i32, cz: i32) -> Option<TreeAt> {
    let h = hash(seed ^ 0x7733, cx as i64, 0, cz as i64);
    let x = cx * TREE_CELL + (h % TREE_CELL as u64) as i32;
    let z = cz * TREE_CELL + ((h >> 8) % TREE_CELL as u64) as i32;
    let col = column(seed, x, z);
    if col.height < SEA || unit(h >> 16) >= tree_density(col.biome) {
        return None;
    }
    if matches!(col.biome, B::Desert | B::Beach | B::Ocean) {
        return None;
    }
    let kind = match col.biome {
        B::Taiga | B::Snowy | B::Mountains => Tree::Spruce,
        B::Birch => Tree::Birch,
        B::Forest if (h >> 40) % 5 == 0 => Tree::Birch,
        _ => Tree::Oak,
    };
    let height = match kind {
        Tree::Spruce => 6 + ((h >> 32) % 4) as i32,
        _ => 4 + ((h >> 32) % 3) as i32,
    };
    Some(TreeAt { x, y: col.height + 1, z, kind, height })
}

/// Calls `put(x, y, z, state, overwrite)` for every block of the tree.
fn tree_blocks(t: &TreeAt, mut put: impl FnMut(i32, i32, i32, u32, bool)) {
    let (log, leaves) = match t.kind {
        Tree::Oak => (OAK_LOG.id(), OAK_LEAVES.id()),
        Tree::Birch => (BIRCH_LOG.id(), BIRCH_LEAVES.id()),
        Tree::Spruce => (SPRUCE_LOG.id(), SPRUCE_LEAVES.id()),
    };
    let top = t.y + t.height;
    match t.kind {
        Tree::Spruce => {
            for dy in 2..=t.height + 1 {
                let y = t.y + dy;
                let r = if dy == t.height + 1 { 0 } else { 1 + ((t.height + 1 - dy) % 3).min(2) };
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dz * dz <= r * r + 1 {
                            put(t.x + dx, y, t.z + dz, leaves, false);
                        }
                    }
                }
            }
        }
        _ => {
            for y in top - 3..=top {
                let r: i32 = if y >= top - 1 { 1 } else { 2 };
                for dz in -r..=r {
                    for dx in -r..=r {
                        let corner = dx.abs() == r && dz.abs() == r;
                        if corner && (y == top || hash(0, (t.x + dx) as i64, y as i64, (t.z + dz) as i64) & 1 == 0) {
                            continue;
                        }
                        put(t.x + dx, y, t.z + dz, leaves, false);
                    }
                }
            }
        }
    }
    for y in t.y..top {
        put(t.x, y, t.z, log, true);
    }
    put(t.x, t.y - 1, t.z, DIRT.id(), true);
}

/// Places a tree into a live world (saplings). Checks for room first.
pub fn grow_tree(w: World, p: [i32; 3]) {
    let h = w.random();
    let t = TreeAt { x: p[0], y: p[1], z: p[2], kind: Tree::Oak, height: 4 + (h % 3) as i32 };
    for y in t.y + 1..t.y + t.height {
        if w.get([t.x, y, t.z]) != 0 {
            return;
        }
    }
    tree_blocks(&t, |x, y, z, s, overwrite| {
        let cur = w.get([x, y, z]);
        if overwrite || cur == 0 || TALL_GRASS.is(cur) {
            w.set([x, y, z], s);
        }
    });
}

struct Ore {
    block: &'static Block,
    max_y: i32,
    veins: u32,
    size: u32,
}

fn ores() -> [Ore; 4] {
    [
        Ore { block: &COAL_ORE, max_y: 128, veins: 8, size: 12 },
        Ore { block: &IRON_ORE, max_y: 64, veins: 6, size: 8 },
        Ore { block: &GOLD_ORE, max_y: 32, veins: 2, size: 7 },
        Ore { block: &DIAMOND_ORE, max_y: 16, veins: 1, size: 6 },
    ]
}

/// Cave density from noise sampled on a 4-block grid and trilinearly interpolated.
fn caves(seed: u64, ox: i32, oy: i32, oz: i32) -> Vec<bool> {
    const G: usize = 5;
    let mut grid = [[[0f64; G]; G]; G];
    for (gy, row) in grid.iter_mut().enumerate() {
        for (gz, line) in row.iter_mut().enumerate() {
            for (gx, v) in line.iter_mut().enumerate() {
                let (x, y, z) = ((ox + gx as i32 * 4) as f64, (oy + gy as i32 * 4) as f64, (oz + gz as i32 * 4) as f64);
                let a = perlin3(seed ^ 0xC1, x / 48.0, y / 32.0, z / 48.0);
                let b = perlin3(seed ^ 0xC2, x / 48.0, y / 32.0, z / 48.0);
                // Two thin noise zero-crossings intersect into winding tunnels.
                let spaghetti = 0.08 - (a.abs() + b.abs());
                let cheese = perlin3(seed ^ 0xC3, x / 90.0, y / 50.0, z / 90.0) - 0.42;
                *v = spaghetti.max(cheese);
            }
        }
    }
    let mut out = vec![false; 4096];
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                let (gx, gy, gz) = (x / 4, y / 4, z / 4);
                let (fx, fy, fz) = ((x % 4) as f64 / 4.0, (y % 4) as f64 / 4.0, (z % 4) as f64 / 4.0);
                let l = |a: f64, b: f64, t: f64| a + (b - a) * t;
                let c = |dy: usize, dz: usize, dx: usize| grid[gy + dy][gz + dz][gx + dx];
                let v = l(
                    l(l(c(0, 0, 0), c(0, 0, 1), fx), l(c(0, 1, 0), c(0, 1, 1), fx), fz),
                    l(l(c(1, 0, 0), c(1, 0, 1), fx), l(c(1, 1, 0), c(1, 1, 1), fx), fz),
                    fy,
                );
                out[(y * 16 + z) * 16 + x] = v > 0.0;
            }
        }
    }
    out
}

pub fn overworld(g: &mut Gen) {
    let seed = g.seed();
    let [ox, oy, oz] = g.origin();
    let mut cols: Vec<Column> = Vec::with_capacity(256);
    for z in 0..16 {
        for x in 0..16 {
            cols.push(column(seed, ox + x, oz + z));
        }
    }
    let max_h = cols.iter().map(|c| c.height).max().unwrap();
    let min_h = cols.iter().map(|c| c.height).min().unwrap();
    for (i, c) in cols.iter().enumerate() {
        g.biomes()[i] = c.biome.handle().id() as u8;
    }

    // Entirely above anything that can be placed: stays air.
    if oy > max_h.max(SEA) + 16 {
        return;
    }

    let cave = if oy + 15 < max_h - 1 { Some(caves(seed, ox, oy, oz)) } else { None };
    let stone = STONE.id();
    for z in 0..16usize {
        for x in 0..16usize {
            let col = &cols[z * 16 + x];
            let h = col.height;
            let (top, filler) = match col.biome {
                B::Desert | B::Beach => (SAND.id(), SAND.id()),
                B::Ocean => (if h < SEA - 8 { GRAVEL.id() } else { SAND.id() }, SAND.id()),
                B::Snowy => (SNOWY_GRASS.id(), DIRT.id()),
                B::Mountains if h > 110 => (STONE.id(), STONE.id()),
                _ => (GRASS.id(), DIRT.id()),
            };
            for y in 0..16usize {
                let wy = oy + y as i32;
                let i = (y * 16 + z) * 16 + x;
                let mut s = if wy > h {
                    if wy <= SEA {
                        if wy == SEA && col.biome == B::Snowy { ICE.id() } else { WATER.id() }
                    } else {
                        0
                    }
                } else if wy == h {
                    top
                } else if wy > h - 4 {
                    if matches!(col.biome, B::Desert) && wy <= h - 3 { SANDSTONE.id() } else { filler }
                } else {
                    stone
                };
                // Keep a seal under water so oceans and rivers do not drain into caves.
                let sealed = h <= SEA && wy > h - 4;
                if let Some(cave) = &cave {
                    if cave[i] && wy <= h && !sealed {
                        s = if (1..11).contains(&wy) { LAVA.id() } else { 0 };
                    }
                }
                g.set(x, y, z, s);
            }
        }
    }

    // Ores: veins anchored in this and neighbouring cubes.
    let [cx, cy, cz] = g.cube();
    if oy < 128 {
        for (oi, ore) in ores().iter().enumerate() {
            if oy > ore.max_y {
                continue;
            }
            for nz in cz - 1..=cz + 1 {
                for ny in cy - 1..=cy + 1 {
                    for nx in cx - 1..=cx + 1 {
                        for v in 0..ore.veins {
                            let h = hash(seed ^ 0x0E0 ^ oi as u64, nx as i64, ny as i64 * 31 + v as i64, nz as i64);
                            let (mut px, mut py, mut pz) = (
                                nx * 16 + (h % 16) as i32,
                                ny * 16 + ((h >> 4) % 16) as i32,
                                nz * 16 + ((h >> 8) % 16) as i32,
                            );
                            if py > ore.max_y {
                                continue;
                            }
                            for k in 0..ore.size {
                                let hk = hash(h, k as i64, 0, 0);
                                px += (hk % 3) as i32 - 1;
                                py += ((hk >> 2) % 3) as i32 - 1;
                                pz += ((hk >> 4) % 3) as i32 - 1;
                                let (lx, ly, lz) = (px - ox, py - oy, pz - oz);
                                if (0..16).contains(&lx) && (0..16).contains(&ly) && (0..16).contains(&lz) {
                                    let (lx, ly, lz) = (lx as usize, ly as usize, lz as usize);
                                    if g.get(lx, ly, lz) == stone {
                                        g.set(lx, ly, lz, ore.block.id());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Surface decoration only near the surface.
    if oy > max_h + 16 || oy + 15 < min_h - 1 {
        return;
    }
    let tc0x = (ox - 3).div_euclid(TREE_CELL);
    let tc1x = (ox + 18).div_euclid(TREE_CELL);
    let tc0z = (oz - 3).div_euclid(TREE_CELL);
    let tc1z = (oz + 18).div_euclid(TREE_CELL);
    for tcz in tc0z..=tc1z {
        for tcx in tc0x..=tc1x {
            let Some(t) = tree_in_cell(seed, tcx, tcz) else { continue };
            tree_blocks(&t, |x, y, z, s, overwrite| {
                let (lx, ly, lz) = (x - ox, y - oy, z - oz);
                if (0..16).contains(&lx) && (0..16).contains(&ly) && (0..16).contains(&lz) {
                    let (lx, ly, lz) = (lx as usize, ly as usize, lz as usize);
                    let cur = g.get(lx, ly, lz);
                    if overwrite || cur == 0 {
                        g.set(lx, ly, lz, s);
                    }
                }
            });
        }
    }
    for z in 0..16usize {
        for x in 0..16usize {
            let col = &cols[z * 16 + x];
            let y = col.height + 1 - oy;
            if !(0..16).contains(&y) || col.height < SEA {
                continue;
            }
            let y = y as usize;
            if y == 0 || g.get(x, y, z) != 0 {
                continue;
            }
            let below = g.get(x, y - 1, z);
            let h = hash(seed ^ 0xF10, (ox + x as i32) as i64, 0, (oz + z as i32) as i64);
            let roll = unit(h);
            if GRASS.is(below) {
                let grass_chance = match col.biome {
                    B::Plains => 0.25,
                    B::Forest | B::Birch => 0.12,
                    B::Taiga => 0.08,
                    _ => 0.05,
                };
                if roll < 0.012 {
                    g.set(x, y, z, if (h >> 32) & 1 == 0 { DANDELION.id() } else { ROSE.id() });
                } else if roll < grass_chance {
                    g.set(x, y, z, TALL_GRASS.id());
                }
            } else if SAND.is(below) && col.biome == B::Desert && roll < 0.006 && y + 2 < 16 {
                let n = 1 + (h >> 40) % 3;
                for k in 0..n as usize {
                    g.set(x, y + k, z, CACTUS.id());
                }
            } else if (SAND.is(below) || GRASS.is(below)) && col.height == SEA && roll < 0.08 && y + 2 < 16 {
                let n = 1 + (h >> 40) % 3;
                for k in 0..n as usize {
                    g.set(x, y + k, z, SUGAR_CANE.id());
                }
            }
        }
    }
}

pub fn spawn(seed: u64) -> [i32; 3] {
    // Open land first: spawning inside a dense forest is dark and confusing.
    for r in 0..200 {
        for k in 0..8 {
            let a = k as f64 / 8.0 * std::f64::consts::TAU;
            let (x, z) = ((r as f64 * 16.0 * cos_approx(a)) as i32, (r as f64 * 16.0 * sin_approx(a)) as i32);
            let c = column(seed, x, z);
            if c.height > SEA + 1 && c.biome == B::Plains && tree_free(seed, x, z) {
                return [x, c.height, z];
            }
        }
    }
    for r in 0..200 {
        for k in 0..8 {
            let a = k as f64 / 8.0 * std::f64::consts::TAU;
            let (x, z) = ((r as f64 * 16.0 * cos_approx(a)) as i32, (r as f64 * 16.0 * sin_approx(a)) as i32);
            let c = column(seed, x, z);
            if c.height > SEA + 1 && !matches!(c.biome, B::Ocean | B::Beach | B::Mountains) && tree_free(seed, x, z) {
                return [x, c.height, z];
            }
        }
    }
    [0, column(seed, 0, 0).height.max(SEA), 0]
}

fn tree_free(seed: u64, x: i32, z: i32) -> bool {
    let (cx, cz) = (x.div_euclid(TREE_CELL), z.div_euclid(TREE_CELL));
    tree_in_cell(seed, cx, cz).is_none_or(|t| (t.x - x).abs() > 2 || (t.z - z).abs() > 2)
}

// Spawn search only needs rough directions; avoids platform-dependent libm results.
fn cos_approx(a: f64) -> f64 {
    sin_approx(a + std::f64::consts::FRAC_PI_2)
}

fn sin_approx(a: f64) -> f64 {
    let x = (a / std::f64::consts::TAU).fract() * std::f64::consts::TAU - std::f64::consts::PI;
    // Bhaskara-like polynomial for sin on [-pi, pi], negated because of the shift.
    let y = 4.0 / std::f64::consts::PI * x - 4.0 / (std::f64::consts::PI * std::f64::consts::PI) * x * x.abs();
    -y
}

/// The test dimension: flat, bounded in height, nothing else.
pub fn flat(g: &mut Gen) {
    let [_, oy, _] = g.origin();
    for y in 0..16usize {
        let wy = oy + y as i32;
        let s = match wy {
            ..=-1 => STONE.id(),
            0..=2 => DIRT.id(),
            3 => GRASS.id(),
            _ => 0,
        };
        if s != 0 {
            for z in 0..16 {
                for x in 0..16 {
                    g.set(x, y, z, s);
                }
            }
        }
    }
}
