//! Stateless overworld generator: every block of a cube is a function of (seed, position). Trees and
//! ore veins are anchored to grid cells; a cube asks every nearby cell what it would place, so the
//! result does not depend on generation order and cubes can be generated in parallel.

use voxide_sdk::*;

use crate::blocks::*;
use std::sync::LazyLock;

use voxide_sdk::noise::*;
use voxide_sdk::terrain::*;

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

pub fn register(r: &mut Registry) {
    r.biome("plains", &PLAINS, 0x8db360, 0x77ab2f, 0x3f76e4, 0.8, 0.4);
    r.biome("forest", &FOREST, 0x79c05a, 0x59ae30, 0x3f76e4, 0.7, 0.8);
    r.biome("birch_forest", &BIRCH_FOREST, 0x88bb67, 0x6ba941, 0x3f76e4, 0.6, 0.6);
    r.biome("taiga", &TAIGA, 0x86b783, 0x68a464, 0x3d57d6, 0.25, 0.8);
    r.biome("snowy_plains", &SNOWY, 0x80b497, 0x60a17b, 0x3938c9, 0.0, 0.5);
    r.biome("desert", &DESERT, 0xbfb755, 0xaea42a, 0x3f76e4, 2.0, 0.0);
    r.biome("mountains", &MOUNTAINS, 0x8ab689, 0x6da36b, 0x3f76e4, 0.2, 0.3);
    r.biome("ocean", &OCEAN, 0x8db360, 0x77ab2f, 0x3f76e4, 0.5, 0.5);
    r.biome("beach", &BEACH, 0x8db360, 0x77ab2f, 0x3f76e4, 0.8, 0.4);

    for (ore, max_y, veins, size) in [("coal_ore", 128, 8, 12), ("iron_ore", 64, 6, 8), ("gold_ore", 32, 2, 7), ("diamond_ore", 16, 1, 6)] {
        r.ore("overworld", ore).y(i32::MIN, max_y).veins(veins).size(size).replace("stone");
    }
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
    fn of(b: &'static Biome) -> B {
        [B::Plains, B::Forest, B::Birch, B::Taiga, B::Snowy, B::Desert, B::Mountains, B::Ocean, B::Beach]
            .into_iter()
            .find(|k| std::ptr::eq(k.handle(), b))
            .unwrap_or(B::Plains)
    }

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

/// The overworld shape. Noise salts match the earlier hand-written generator, so existing worlds
/// continue without seams.
static TERRAIN: LazyLock<Terrain> = LazyLock::new(|| {
    Terrain::new(&STONE, SEA as f64)
        .sea(SEA, &WATER)
        .feature(preset::continents(14.0))
        .feature(preset::hills(5.0, 60.0))
        .feature(
            preset::mountains(70.0, 160.0)
                .mask(Mask::noise(500.0, 0.15, 0.15 + 1.0 / 3.0).salt(0x44).and(Mask::Continent(0.05, 0.30)))
                .named("mountains"),
        )
        .biome(BiomeRule::new(&OCEAN).height(i32::MIN, SEA - 2).surface(&SAND, &SAND, 3).deep_top(&GRAVEL, SEA - 8))
        .biome(BiomeRule::new(&SNOWY).height(i32::MIN, SEA + 1).continent(f64::NEG_INFINITY, 0.1).temperature(f64::NEG_INFINITY, -0.45).surface(&SNOWY_GRASS, &DIRT, 3).frozen(&ICE))
        .biome(BiomeRule::new(&BEACH).height(i32::MIN, SEA + 1).continent(f64::NEG_INFINITY, 0.1).surface(&SAND, &SAND, 3))
        .biome(BiomeRule::new(&MOUNTAINS).feature("mountains", 0.45, f64::INFINITY).surface(&GRASS, &DIRT, 3).bare_above(110))
        .biome(BiomeRule::new(&SNOWY).temperature(f64::NEG_INFINITY, -0.45).surface(&SNOWY_GRASS, &DIRT, 3).frozen(&ICE))
        .biome(BiomeRule::new(&TAIGA).temperature(f64::NEG_INFINITY, -0.15).surface(&GRASS, &DIRT, 3))
        .biome(BiomeRule::new(&DESERT).temperature(0.35, f64::INFINITY).humidity(f64::NEG_INFINITY, 0.0).surface(&SAND, &SAND, 3).under(&SANDSTONE, 1))
        .biome(BiomeRule::new(&FOREST).humidity(0.25, f64::INFINITY).surface(&GRASS, &DIRT, 3))
        .biome(BiomeRule::new(&BIRCH_FOREST).humidity(0.05, f64::INFINITY).temperature(f64::NEG_INFINITY, 0.1).surface(&GRASS, &DIRT, 3))
        .biome(BiomeRule::new(&PLAINS).surface(&GRASS, &DIRT, 3))
        .caves(Caves::new(&LAVA, 1, 11))
});

pub struct Column {
    pub height: i32,
    pub biome: B,
}

pub fn column(seed: u64, x: i32, z: i32) -> Column {
    let c = TERRAIN.column(seed, x, z);
    Column { height: c.height, biome: B::of(c.biome) }
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
    if col.height < SEA || unit(h.rotate_left(17)) >= tree_density(col.biome) {
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

pub fn overworld(g: &mut Gen) {
    let Some(cols) = TERRAIN.generate(g) else { return };
    let seed = g.seed();
    let [ox, oy, oz] = g.origin();
    let max_h = cols.iter().map(|c| c.height).max().unwrap_or(0);
    let min_h = cols.iter().map(|c| c.height).min().unwrap_or(0);
    let cols: Vec<Column> = cols.iter().map(|c| Column { height: c.height, biome: B::of(c.biome) }).collect();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biome_mix() {
        let mut counts = std::collections::BTreeMap::new();
        for x in (-4000..4000).step_by(64) {
            for z in (-4000..4000).step_by(64) {
                *counts.entry(format!("{:?}", column(7, x, z).biome)).or_insert(0) += 1;
            }
        }
        println!("{counts:?}");
        let sp = spawn(7);
        let c = column(7, sp[0], sp[2]);
        println!("spawn {sp:?} biome {:?} height {}", c.biome, c.height);
        assert!(counts.get("Plains").copied().unwrap_or(0) > 100);
    }
}
