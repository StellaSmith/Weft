//! Stateless overworld generator: every block of a cube is a function of (seed, position). Trees and
//! ore veins are anchored to grid cells; a cube asks every nearby cell what it would place, so the
//! result does not depend on generation order and cubes can be generated in parallel.

use weft_sdk::*;

use crate::OVERWORLD;
use crate::blocks::*;

use weft_sdk::noise::*;
use weft_sdk::terrain::*;

pub const SEA: i32 = 63;

static PLANT_SOIL: Tag = Tag::named("plant_soil");
static CACTUS_SOIL: Tag = Tag::named("cactus_soil");
static SUGAR_CANE_SOIL: Tag = Tag::named("sugar_cane_soil");
/// What a biome grows, as data any mod can set on any biome: trees per 5x5 cell (0..1), share of
/// grass on plantable soil, which trees and flowers, and tags `snowy` (nothing grows), `dry`
/// (cacti).
pub static TREES: Component<f32> = Component::named("trees");
pub static GRASS_COVER: Component<f32> = Component::named("grass");
/// The logs of the trees a biome grows, with weights: `"base:birch_log:1 base:oak_log:4"`
/// (oaks where unset).
pub static TREE_KINDS: Component<Text> = Component::named("tree_kinds");
/// Flowers a biome grows, picked evenly (`"base:dandelion base:rose"` where unset).
pub static FLOWERS: Component<Text> = Component::named("flowers");
/// On a log, the tree it grows into: `round <leaves>` (like oaks, 4-6 high) or `cone <leaves>`
/// (like spruces, 6-9 high).
pub static TREE: Component<Text> = Component::named("tree");
static SNOWY_TAG: Tag = Tag::named("snowy");
static DRY: Tag = Tag::named("dry");

pub static PLAINS: Biome = Biome::named("plains");
pub static FOREST: Biome = Biome::named("forest");
pub static BIRCH_FOREST: Biome = Biome::named("birch_forest");
pub static TAIGA: Biome = Biome::named("taiga");
pub static SNOWY: Biome = Biome::named("snowy_plains");
pub static DESERT: Biome = Biome::named("desert");
pub static MOUNTAINS: Biome = Biome::named("mountains");
pub static OCEAN: Biome = Biome::named("ocean");
pub static BEACH: Biome = Biome::named("beach");
pub static DEEP_CAVES: Biome = Biome::named("deep_caves");

pub fn register(r: &mut Registry) {
    // Tags say what a biome is (spawning, decoration and mods ask these, not names).
    r.biome(&PLAINS).colors(0x8db360, 0x77ab2f, 0x3f76e4).climate(0.8, 0.4).tag("animals").set(&TREES, 0.025).set(&GRASS_COVER, 0.25);
    r.biome(&FOREST).colors(0x79c05a, 0x59ae30, 0x3f76e4).climate(0.7, 0.8).tag("forest").tag("animals").set(&TREES, 0.4).set(&GRASS_COVER, 0.12).set_text(&TREE_KINDS, "base:birch_log:1 base:oak_log:4");
    r.biome(&BIRCH_FOREST).colors(0x88bb67, 0x6ba941, 0x3f76e4).climate(0.6, 0.6).tag("forest").tag("animals").set(&TREES, 0.45).set(&GRASS_COVER, 0.12).set_text(&TREE_KINDS, "base:birch_log");
    r.biome(&TAIGA).colors(0x86b783, 0x68a464, 0x3d57d6).climate(0.25, 0.8).tag("forest").tag("cold").tag("animals").set(&TREES, 0.45).set(&GRASS_COVER, 0.08).set_text(&TREE_KINDS, "base:spruce_log");
    r.biome(&SNOWY).colors(0x80b497, 0x60a17b, 0x3938c9).climate(0.0, 0.5).tag("cold").tag("snowy").tag("animals").set(&TREES, 0.03).set_text(&TREE_KINDS, "base:spruce_log");
    r.biome(&DESERT).colors(0xbfb755, 0xaea42a, 0x3f76e4).climate(2.0, 0.0).tag("dry").tag("hot");
    r.biome(&MOUNTAINS).colors(0x8ab689, 0x6da36b, 0x3f76e4).climate(0.2, 0.3).tag("mountain").tag("animals").set(&TREES, 0.06).set(&GRASS_COVER, 0.05).set_text(&TREE_KINDS, "base:spruce_log");
    r.biome(&OCEAN).colors(0x8db360, 0x77ab2f, 0x3f76e4).climate(0.5, 0.5).tag("ocean");
    r.biome(&BEACH).colors(0x8db360, 0x77ab2f, 0x3f76e4).climate(0.8, 0.4).tag("beach");
    // Underground (3D): whole cubes deep under the surface; spiders like it there.
    r.biome(&DEEP_CAVES).colors(0x8db360, 0x77ab2f, 0x3f76e4).climate(0.5, 0.5).tag("cave").tag("dark");

    for (ore, max_y, veins, size) in [(&COAL_ORE, 128, 8, 12), (&IRON_ORE, 64, 6, 8), (&GOLD_ORE, 32, 2, 7), (&DIAMOND_ORE, 16, 1, 6)] {
        r.ore(&crate::OVERWORLD, ore).y(i32::MIN, max_y).veins(veins).size(size).replace("#stone_ore_replaceables");
    }

    // The overworld's shape. Other mods add to it by these names (`base:desert`, `base:mountains`).
    let (low, high) = (f64::NEG_INFINITY, f64::INFINITY);
    r.terrain(&crate::OVERWORLD)
        .stone("stone")
        .base_height(SEA as f64)
        .sea(SEA, "water")
        .feature(preset::continents(18.0))
        // Biomes shape the land through their climate, not their borders, so nothing seams:
        // the cold flattens hills into tundra, deserts get dunes.
        .feature(preset::hills(16.0, 110.0).mask(Mask::Temperature(-0.35, -0.6).not()))
        .feature(preset::dunes(4.0, 26.0).mask(Mask::Temperature(0.25, 0.45).and(Mask::Humidity(0.1, -0.05))))
        .feature(preset::mountains(150.0, 260.0).mask(Mask::noise(700.0, -0.1, 0.3).salt(0x44).and(Mask::Continent(0.0, 0.2))).named("mountains"))
        .biome(BiomeRule::new("ocean").height(i32::MIN, SEA - 2).surface("sand", "sand", 3).deep_top("gravel", SEA - 8))
        .biome(BiomeRule::new("snowy_plains").named("snowy_coast").height(i32::MIN, SEA + 1).continent(low, 0.1).temperature(low, -0.45).surface("snowy_grass", "dirt", 3).frozen("ice"))
        .biome(BiomeRule::new("beach").height(i32::MIN, SEA + 1).continent(low, 0.1).surface("sand", "sand", 3))
        .biome(BiomeRule::new("mountains").feature("mountains", 0.45, high).surface("grass", "dirt", 3).bare_above(130))
        .biome(BiomeRule::new("snowy_plains").temperature(low, -0.45).surface("snowy_grass", "dirt", 3).frozen("ice"))
        .biome(BiomeRule::new("taiga").temperature(low, -0.15).surface("grass", "dirt", 3))
        .biome(BiomeRule::new("desert").temperature(0.35, high).humidity(low, 0.0).surface("sand", "sand", 3).under("sandstone", 1))
        .biome(BiomeRule::new("forest").humidity(0.25, high).surface("grass", "dirt", 3))
        .biome(BiomeRule::new("birch_forest").humidity(0.05, high).temperature(low, 0.1).surface("grass", "dirt", 3))
        .biome(BiomeRule::new("plains").surface("grass", "dirt", 3))
        .cave_biome("deep_caves", 24, 96.0, 0.25)
        .caves(Caves::new("lava", i32::MIN, 11))
        .decorate(decorate);
}

const TREE_CELL: i32 = 5;

struct TreeAt {
    x: i32,
    y: i32,
    z: i32,
    log: u32,
    leaves: u32,
    cone: bool,
    height: i32,
}

/// One of `"name:weight name ..."` (weight 1 when left out) by `roll`, as a block state.
fn pick(list: &str, roll: u64) -> Option<u32> {
    let entries = || {
        list.split_whitespace().map(|e| match e.rsplit_once(':').and_then(|(n, w)| Some((n, w.parse::<u32>().ok()?))) {
            Some(nw) => nw,
            None => (e, 1),
        })
    };
    let total: u32 = entries().map(|e| e.1).sum();
    let mut r = (roll % total.max(1) as u64) as u32;
    for (name, w) in entries() {
        if r < w {
            return Block::lookup(name);
        }
        r -= w;
    }
    None
}

/// A tree growing from `log` (its `tree` component), `h` deciding the height.
fn tree_at(log: u32, x: i32, y: i32, z: i32, h: u64) -> Option<TreeAt> {
    let (shape, leaves) = TREE.on_block(log)?.as_str().split_once(' ')?;
    let cone = shape == "cone";
    let height = if cone { 6 + (h % 4) as i32 } else { 4 + (h % 3) as i32 };
    Some(TreeAt { x, y, z, log, leaves: Block::lookup(leaves.trim())?, cone, height })
}

fn tree_in_cell(seed: u64, cx: i32, cz: i32) -> Option<TreeAt> {
    let h = hash(seed ^ 0x7733, cx as i64, 0, cz as i64);
    let x = cx * TREE_CELL + (h % TREE_CELL as u64) as i32;
    let z = cz * TREE_CELL + ((h >> 8) % TREE_CELL as u64) as i32;
    let col = OVERWORLD.column(seed, x, z);
    let b = col.biome;
    if col.height < SEA || unit(h.rotate_left(17)) >= TREES.on_biome(b).unwrap_or(0.0) as f64 || OVERWORLD.surface_carved(seed, x, z) {
        return None;
    }
    let log = pick(TREE_KINDS.on_biome(b).map_or("base:oak_log", |t| t.as_str()), h >> 40)?;
    tree_at(log, x, col.height + 1, z, h >> 32)
}

/// Calls `put(x, y, z, state, overwrite)` for every block of the tree.
fn tree_blocks(t: &TreeAt, mut put: impl FnMut(i32, i32, i32, u32, bool)) {
    let (log, leaves) = (t.log, t.leaves);
    let top = t.y + t.height;
    match t.cone {
        true => {
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
        false => {
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
    let Some(t) = tree_at(OAK_LOG.id(), p[0], p[1], p[2], w.random()) else { return };
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

/// Trees and plants, after the terrain filled the cube.
fn decorate(g: &mut Gen) {
    let Some(&cols) = g.columns() else { return };
    let seed = g.seed();
    let [ox, oy, oz] = g.origin();
    let max_h = cols.iter().map(|c| c.height).max().unwrap_or(0);
    let min_h = cols.iter().map(|c| c.height).min().unwrap_or(0);

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
            // Plants go where they could be planted (soil tags); the biome decides how many.
            let b = col.biome;
            if SNOWY_TAG.on_biome(b) {
                continue;
            }
            if PLANT_SOIL.on_block(below) {
                let grass_chance = GRASS_COVER.on_biome(b).unwrap_or(0.05) as f64;
                if roll < 0.012 {
                    if let Some(f) = pick(FLOWERS.on_biome(b).map_or("base:dandelion base:rose", |t| t.as_str()), h >> 32) {
                        g.set(x, y, z, f);
                    }
                } else if roll < grass_chance {
                    g.set(x, y, z, TALL_GRASS.id());
                }
            } else if CACTUS_SOIL.on_block(below) && DRY.on_biome(b) && roll < 0.006 && y + 2 < 16 {
                let n = 1 + (h >> 40) % 3;
                for k in 0..n as usize {
                    g.set(x, y + k, z, CACTUS.id());
                }
            } else if SUGAR_CANE_SOIL.on_block(below) && col.height == SEA && roll < 0.08 && y + 2 < 16 {
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
            let c = OVERWORLD.column(seed, x, z);
            if c.height > SEA + 1 && c.biome == PLAINS.id() && tree_free(seed, x, z) && !OVERWORLD.surface_carved(seed, x, z) {
                return [x, c.height, z];
            }
        }
    }
    for r in 0..200 {
        for k in 0..8 {
            let a = k as f64 / 8.0 * std::f64::consts::TAU;
            let (x, z) = ((r as f64 * 16.0 * cos_approx(a)) as i32, (r as f64 * 16.0 * sin_approx(a)) as i32);
            let c = OVERWORLD.column(seed, x, z);
            if c.height > SEA + 1 && [&OCEAN, &BEACH, &MOUNTAINS].iter().all(|b| c.biome != b.id()) && tree_free(seed, x, z) && !OVERWORLD.surface_carved(seed, x, z) {
                return [x, c.height, z];
            }
        }
    }
    [0, OVERWORLD.column(seed, 0, 0).height.max(SEA), 0]
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
