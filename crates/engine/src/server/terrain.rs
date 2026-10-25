//! Terrain described as data (`Host::reg_terrain`): a base height plus height features, biome
//! rules that pick each column's biome and surface, water and caves. The dimension's mod
//! describes it, other mods edit it, and the merged result is evaluated here, so structures,
//! decorations and spawn search all read the same columns from one cache.

use std::sync::Arc;

use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use voxide_abi::{self as abi, Callback, ModId, TerrainColumn, rule_field as rf, terrain_op as op};
use voxide_noise::*;

use crate::host;

/// One `TerrainDesc` as sent, owned.
pub struct RawTerrain {
    pub owner: ModId,
    pub dimension: String,
    pub op: u32,
    pub anchor: String,
    pub block: String,
    pub level: i32,
    pub to: i32,
    pub value: f64,
    pub size: f64,
    pub feature: RawFeature,
    pub rule: RawRule,
    pub decorate: Option<Callback>,
}

#[derive(Clone)]
pub struct RawFeature {
    pub owner: ModId,
    pub name: String,
    pub preset: u32,
    pub params: [f64; 4],
    pub height: Callback,
    pub mask: Vec<abi::MaskStep>,
}

#[derive(Clone)]
pub struct RawRule {
    pub name: String,
    pub fields: u32,
    pub biome: String,
    pub temperature: (f64, f64),
    pub humidity: (f64, f64),
    pub continent: (f64, f64),
    pub height: (i32, i32),
    pub feature: (String, f64, f64),
    pub top: String,
    pub filler: String,
    pub depth: i32,
    pub under: (String, i32),
    pub deep_top: (String, i32),
    pub bare_above: i32,
    pub ice: String,
}

impl RawRule {
    /// The fields of `edit` named in its `fields` replace this rule's.
    fn apply(&mut self, edit: &RawRule) {
        let has = |f: u32| edit.fields & f != 0;
        if has(rf::BIOME) {
            self.biome = edit.biome.clone();
        }
        if has(rf::TEMPERATURE) {
            self.temperature = edit.temperature;
        }
        if has(rf::HUMIDITY) {
            self.humidity = edit.humidity;
        }
        if has(rf::CONTINENT) {
            self.continent = edit.continent;
        }
        if has(rf::HEIGHT) {
            self.height = edit.height;
        }
        if has(rf::FEATURE) {
            self.feature = edit.feature.clone();
        }
        if has(rf::SURFACE) {
            (self.top, self.filler, self.depth) = (edit.top.clone(), edit.filler.clone(), edit.depth);
        }
        if has(rf::UNDER) {
            self.under = edit.under.clone();
        }
        if has(rf::DEEP_TOP) {
            self.deep_top = edit.deep_top.clone();
        }
        if has(rf::BARE_ABOVE) {
            self.bare_above = edit.bare_above;
        }
        if has(rf::FROZEN) {
            self.ice = edit.ice.clone();
        }
    }
}

/// # Safety
/// The pointers in `d` must be valid for the call.
pub unsafe fn copy(owner: ModId, d: &abi::TerrainDesc) -> RawTerrain {
    unsafe {
        let s = |s: abi::Str| s.as_str().to_string();
        let f = &d.feature;
        let r = &d.rule;
        RawTerrain {
            owner,
            dimension: s(d.dimension),
            op: d.op,
            anchor: s(d.anchor),
            block: s(d.block),
            level: d.level,
            to: d.to,
            value: d.value,
            size: d.size,
            feature: RawFeature { owner, name: s(f.name), preset: f.preset, params: f.params, height: f.height, mask: f.mask.as_slice().to_vec() },
            rule: RawRule {
                name: s(r.name),
                fields: r.fields,
                biome: s(r.biome),
                temperature: (r.temperature[0], r.temperature[1]),
                humidity: (r.humidity[0], r.humidity[1]),
                continent: (r.continent[0], r.continent[1]),
                height: (r.height[0], r.height[1]),
                feature: (s(r.feature), r.feature_range[0], r.feature_range[1]),
                top: s(r.top),
                filler: s(r.filler),
                depth: r.depth,
                under: (s(r.under), r.under_layers),
                deep_top: (s(r.deep_top), r.deep_below),
                bare_above: r.bare_above,
                ice: s(r.ice),
            },
            decorate: d.decorate.is_some().then_some(d.decorate),
        }
    }
}

/// Where a feature applies, 0..1.
#[derive(Clone, Debug)]
pub enum Mask {
    Noise { salt: u64, scale: f64, octaves: u32, from: f64, to: f64 },
    Continent(f64, f64),
    Temperature(f64, f64),
    Humidity(f64, f64),
    All(Vec<Mask>),
    Not(Box<Mask>),
}

impl Mask {
    /// From postfix steps; `None` for an empty or malformed list.
    fn from_steps(steps: &[abi::MaskStep]) -> Option<Mask> {
        use abi::terrain_mask as k;
        let mut stack: Vec<Mask> = Vec::new();
        for s in steps {
            let m = match s.kind {
                k::NOISE => Mask::Noise { salt: s.salt, scale: s.scale, octaves: s.octaves, from: s.from, to: s.to },
                k::CONTINENT => Mask::Continent(s.from, s.to),
                k::TEMPERATURE => Mask::Temperature(s.from, s.to),
                k::HUMIDITY => Mask::Humidity(s.from, s.to),
                k::ALL => {
                    let at = stack.len().checked_sub(s.count as usize)?;
                    Mask::All(stack.split_off(at))
                }
                k::NOT => Mask::Not(Box::new(stack.pop()?)),
                _ => return None,
            };
            stack.push(m);
        }
        (stack.len() == 1).then(|| stack.pop()).flatten()
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

pub enum Height {
    /// `abi::terrain_preset` and its parameters.
    Preset(u32, [f64; 4]),
    Custom(ModId, Callback),
}

pub struct Feature {
    pub name: String,
    pub height: Height,
    pub mask: Option<Mask>,
}

impl Feature {
    fn height(&self, f: &Fields, x: f64, z: f64) -> f64 {
        use abi::terrain_preset as p;
        match self.height {
            Height::Preset(kind, [a, b, c, _]) => match kind {
                p::CONTINENTS => shape::continents(f, x, z, a),
                p::HILLS => shape::hills(f, x, z, a, b),
                p::MOUNTAINS => shape::mountains(f, x, z, a, b),
                p::PLATEAUS => shape::plateaus(f, x, z, a, b, c),
                p::CANYONS => shape::canyons(f, x, z, a, b),
                p::DUNES => shape::dunes(f, x, z, a, b),
                p::CRATERS => shape::craters(f, x, z, a, b, c),
                _ => 0.0,
            },
            Height::Custom(owner, cb) => {
                let mut args = abi::FeatureArgs { seed: f.seed(), x, z, height: 0.0 };
                host::invoke(owner, cb, &mut args);
                args.height
            }
        }
    }
}

/// A biome rule with names resolved: block states and biome ids.
pub struct Rule {
    pub biome: u32,
    temperature: (f64, f64),
    humidity: (f64, f64),
    continent: (f64, f64),
    height: (i32, i32),
    /// Index into `Terrain::features`, `usize::MAX` for a feature that does not exist.
    feature: Option<(usize, f64, f64)>,
    top: Option<u32>,
    filler: Option<u32>,
    depth: i32,
    under: Option<(u32, i32)>,
    deep_top: Option<(u32, i32)>,
    bare_above: Option<i32>,
    ice: Option<u32>,
}

pub struct Caves {
    lava: u32,
    lava_from: i32,
    lava_to: i32,
}

pub struct CaveBiome {
    pub biome: u32,
    depth: i32,
    size: f64,
    threshold: f64,
}

/// The columns of one column of cubes.
pub struct Columns {
    pub cols: [TerrainColumn; 256],
    rules: [u32; 256],
}

pub struct Terrain {
    stone: u32,
    base: f64,
    sea: Option<(i32, u32)>,
    pub features: Vec<Feature>,
    pub rules: Vec<Rule>,
    caves: Option<Caves>,
    /// Biomes of cubes deep underground.
    pub cave_biomes: Vec<CaveBiome>,
    /// (mod, callback) run after the terrain filled a cube, the owner's first.
    pub decorations: Vec<(ModId, Callback)>,
    cache: ColumnCache<Columns>,
    /// Column heights every 4 blocks over a cube's columns and its +X/+Z edge (5 x 5): caves
    /// measure depth from this smoothed surface, so steep slopes do not leave single-column
    /// pillars.
    corners: ColumnCache<[f32; 25]>,
}

/// The terrain being merged from the owner's description and the edits, still by name.
struct Draft {
    stone: Option<String>,
    base: f64,
    sea: Option<(i32, String)>,
    caves: Option<(String, i32, i32)>,
    features: Vec<RawFeature>,
    rules: Vec<RawRule>,
    cave_biomes: Vec<(String, i32, f64, f64)>,
    decorations: Vec<(ModId, Callback)>,
}

/// Looks up content by full name while terrains are built.
pub struct Names<'a> {
    /// Name to first block state.
    pub block: &'a dyn Fn(&str) -> Option<u32>,
    pub biome: &'a dyn Fn(&str) -> Option<u32>,
}

/// Merges `ops` (the owner's first, then the other mods' in load order) into a terrain.
/// Missing names are `errors`; an edit whose target is gone is skipped with a warning.
pub fn build(dimension: &str, owner_name: &dyn Fn(ModId) -> String, ops: Vec<RawTerrain>, names: &Names, errors: &mut Vec<String>) -> Terrain {
    let mut d = Draft { stone: None, base: 64.0, sea: None, caves: None, features: Vec::new(), rules: Vec::new(), cave_biomes: Vec::new(), decorations: Vec::new() };
    let rule_name = |r: &RawRule| if r.name.is_empty() { r.biome.clone() } else { r.name.clone() };
    for o in ops {
        let who = owner_name(o.owner);
        let missing = |what: &str| log::warn!("{who}: the terrain of {dimension} has no {what} {}, edit skipped", o.anchor);
        let new_rule = |rules: &[RawRule], r: &RawRule, errors: &mut Vec<String>| {
            let n = rule_name(r);
            if rules.iter().any(|x| rule_name(x) == n) {
                errors.push(format!("{who}: the terrain of {dimension} already has a biome rule named {n} (name the new one, or use replace_biome)"));
                return false;
            }
            true
        };
        let rule_at = |rules: &[RawRule]| rules.iter().position(|r| rule_name(r) == o.anchor);
        match o.op {
            op::STONE => d.stone = Some(o.block),
            op::BASE_HEIGHT => d.base = o.value,
            op::SEA => d.sea = Some((o.level, o.block)),
            op::CAVES => d.caves = Some((o.block, o.level, o.to)),
            op::FEATURE => {
                if !o.feature.name.is_empty() && d.features.iter().any(|f| f.name == o.feature.name) {
                    errors.push(format!("{who}: the terrain of {dimension} already has a feature named {}", o.feature.name));
                } else {
                    d.features.push(o.feature);
                }
            }
            op::REMOVE_FEATURE => match d.features.iter().position(|f| f.name == o.anchor) {
                Some(i) => drop(d.features.remove(i)),
                None => missing("feature"),
            },
            op::MASK_FEATURE => match d.features.iter_mut().find(|f| f.name == o.anchor) {
                Some(f) if f.mask.is_empty() => f.mask = o.feature.mask,
                Some(f) => {
                    f.mask.extend(o.feature.mask);
                    f.mask.push(abi::MaskStep { kind: abi::terrain_mask::ALL, count: 2, ..abi::MaskStep::DEFAULT });
                }
                None => missing("feature"),
            },
            op::BIOME => {
                if new_rule(&d.rules, &o.rule, errors) {
                    d.rules.push(o.rule);
                }
            }
            op::BIOME_BEFORE | op::BIOME_AFTER => match rule_at(&d.rules) {
                Some(i) if new_rule(&d.rules, &o.rule, errors) => d.rules.insert(i + (o.op == op::BIOME_AFTER) as usize, o.rule),
                Some(_) => {}
                None => missing("biome rule"),
            },
            op::REPLACE_BIOME => match rule_at(&d.rules) {
                Some(i) => {
                    let old = d.rules.remove(i);
                    if new_rule(&d.rules, &o.rule, errors) {
                        d.rules.insert(i, o.rule);
                    } else {
                        d.rules.insert(i, old);
                    }
                }
                None => missing("biome rule"),
            },
            op::REMOVE_BIOME => match rule_at(&d.rules) {
                Some(i) => drop(d.rules.remove(i)),
                None => missing("biome rule"),
            },
            op::EDIT_BIOME => match rule_at(&d.rules) {
                Some(i) => d.rules[i].apply(&o.rule),
                None => missing("biome rule"),
            },
            op::CAVE_BIOME => d.cave_biomes.push((o.rule.biome, o.level, o.size, o.value)),
            op::REMOVE_CAVE_BIOME => match d.cave_biomes.iter().position(|c| c.0 == o.anchor) {
                Some(i) => drop(d.cave_biomes.remove(i)),
                None => missing("cave biome"),
            },
            op::DECORATE => d.decorations.extend(o.decorate.map(|cb| (o.owner, cb))),
            n => errors.push(format!("{who}: unknown terrain edit {n} for {dimension}")),
        }
    }

    let block = |name: &str, what: &str, errors: &mut Vec<String>| -> Option<u32> {
        if name.is_empty() {
            return None;
        }
        let b = (names.block)(name);
        if b.is_none() {
            errors.push(format!("terrain of {dimension}: unknown block {name} ({what})"));
        }
        b
    };
    let stone = match &d.stone {
        Some(s) => block(s, "stone", errors).unwrap_or(0),
        None => {
            errors.push(format!("terrain of {dimension}: no stone block (`.stone(..)`)"));
            0
        }
    };
    let sea = d.sea.as_ref().and_then(|(l, w)| Some((*l, block(w, "sea", errors)?)));
    let caves = d.caves.as_ref().and_then(|(lava, from, to)| Some(Caves { lava: block(lava, "lava", errors)?, lava_from: *from, lava_to: *to }));
    let features: Vec<Feature> = d
        .features
        .iter()
        .map(|f| Feature {
            name: f.name.clone(),
            height: if f.preset == abi::terrain_preset::CUSTOM { Height::Custom(f.owner, f.height) } else { Height::Preset(f.preset, f.params) },
            mask: Mask::from_steps(&f.mask),
        })
        .collect();
    let mut rules = Vec::with_capacity(d.rules.len());
    for r in &d.rules {
        let biome = (names.biome)(&r.biome).unwrap_or_else(|| {
            errors.push(format!("terrain of {dimension}: unknown biome {}", r.biome));
            0
        });
        let feature = (!r.feature.0.is_empty()).then(|| {
            let i = features.iter().position(|f| f.name == r.feature.0);
            if i.is_none() {
                errors.push(format!("terrain of {dimension}: biome rule {} asks for feature {}, which it doesn't have", rule_name(r), r.feature.0));
            }
            (i.unwrap_or(usize::MAX), r.feature.1, r.feature.2)
        });
        let what = format!("biome rule {}", rule_name(r));
        rules.push(Rule {
            biome,
            temperature: r.temperature,
            humidity: r.humidity,
            continent: r.continent,
            height: r.height,
            feature,
            top: block(&r.top, &what, errors),
            filler: block(&r.filler, &what, errors),
            depth: r.depth,
            under: block(&r.under.0, &what, errors).map(|b| (b, r.under.1)),
            deep_top: block(&r.deep_top.0, &what, errors).map(|b| (b, r.deep_top.1)),
            bare_above: (r.bare_above != i32::MAX).then_some(r.bare_above),
            ice: block(&r.ice, &what, errors),
        });
    }
    let cave_biomes = d
        .cave_biomes
        .iter()
        .map(|(b, depth, size, threshold)| CaveBiome {
            biome: (names.biome)(b).unwrap_or_else(|| {
                errors.push(format!("terrain of {dimension}: unknown cave biome {b}"));
                0
            }),
            depth: *depth,
            size: *size,
            threshold: *threshold,
        })
        .collect();
    Terrain::new(stone, d.base, sea, features, rules, caves, cave_biomes, d.decorations)
}

impl Terrain {
    #[allow(clippy::too_many_arguments)]
    fn new(stone: u32, base: f64, sea: Option<(i32, u32)>, features: Vec<Feature>, rules: Vec<Rule>, caves: Option<Caves>, cave_biomes: Vec<CaveBiome>, decorations: Vec<(ModId, Callback)>) -> Terrain {
        Terrain { stone, base, sea, features, rules, caves, cave_biomes, decorations, cache: ColumnCache::new(16384), corners: ColumnCache::new(16384) }
    }

    /// Height, biome and rule of column (`x`, `z`).
    fn compute(&self, seed: u64, x: i32, z: i32) -> (TerrainColumn, u32) {
        let f = Fields::new(seed);
        let (fx, fz) = (x as f64, z as f64);
        let mut h = self.base;
        let mut strengths = [0f64; 16];
        let mut more = Vec::new();
        for (i, feat) in self.features.iter().enumerate() {
            let w = feat.mask.as_ref().map_or(1.0, |m| m.value(&f, fx, fz));
            match strengths.get_mut(i) {
                Some(s) => *s = w,
                None => more.push(w),
            }
            if w > 0.0 {
                h += w * feat.height(&f, fx, fz);
            }
        }
        let strength = |i: usize| if i < 16 { Some(strengths[i]) } else { more.get(i - 16).copied() };
        let height = h.floor() as i32;
        let (temp, humid, cont) = (f.temperature(fx, fz), f.humidity(fx, fz), f.continent(fx, fz));
        let inside = |v: f64, r: (f64, f64)| v >= r.0 && v < r.1;
        let rule = self
            .rules
            .iter()
            .position(|b| {
                height >= b.height.0
                    && height <= b.height.1
                    && inside(temp, b.temperature)
                    && inside(humid, b.humidity)
                    && inside(cont, b.continent)
                    && b.feature.is_none_or(|(i, lo, hi)| strength(i).is_some_and(|s| s > lo && s <= hi))
            })
            .unwrap_or(self.rules.len().saturating_sub(1));
        (TerrainColumn { height, biome: self.rules.get(rule).map_or(0, |r| r.biome) }, rule as u32)
    }

    /// Column (`x`, `z`) from the cache when its cube column is there, computed otherwise (a
    /// single lookup should not cost a whole cube column).
    pub fn column(&self, seed: u64, x: i32, z: i32) -> TerrainColumn {
        match self.cache.peek(seed, x >> 4, z >> 4) {
            Some(c) => c.cols[((z & 15) * 16 + (x & 15)) as usize],
            None => self.compute(seed, x, z).0,
        }
    }

    /// The columns of cube column (`cx`, `cz`).
    pub fn columns(&self, seed: u64, cx: i32, cz: i32) -> Arc<Columns> {
        self.cache.get(seed, cx, cz, || {
            let mut c = Columns { cols: [TerrainColumn::default(); 256], rules: [0; 256] };
            for i in 0..256 {
                (c.cols[i], c.rules[i]) = self.compute(seed, cx * 16 + (i % 16) as i32, cz * 16 + (i / 16) as i32);
            }
            c
        })
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

    /// Fills cube `pos` with stone, surface layers, water and caves and sets its biomes; returns
    /// its columns.
    pub fn generate(&self, seed: u64, pos: [i32; 3], blocks: &mut [u32], biomes: &mut [u16; 256]) -> Arc<Columns> {
        let [cx, cy, cz] = pos;
        let (ox, oy, oz) = (cx * 16, cy * 16, cz * 16);
        let cols = self.columns(seed, cx, cz);
        for (i, c) in cols.cols.iter().enumerate() {
            let (x, z) = ((ox + (i % 16) as i32) as f64, (oz + (i / 16) as i32) as f64);
            let mid = (oy + 8) as f64;
            let deep = self.cave_biomes.iter().enumerate().find(|&(k, b)| oy + 15 < c.height - b.depth && perlin3(seed ^ 0xCB10 ^ k as u64, x / b.size, mid / b.size, z / b.size) > b.threshold);
            biomes[i] = deep.map_or(c.biome, |(_, b)| b.biome) as u16;
        }
        let max_h = cols.cols.iter().map(|c| c.height).max().unwrap_or(0);
        let sea = self.sea.map_or(i32::MIN, |s| s.0);
        if oy > max_h.max(sea) + 16 {
            return cols;
        }
        // Decided per block from its column alone, so neighbouring cubes agree: tunnels may open
        // onto the surface, caverns close up under it except where `cavern_roof` lets them out.
        let cave = match &self.caves {
            Some(_) if oy <= max_h => Some(cave_fields(seed, ox, oy, oz)),
            _ => None,
        };
        let corners = self.corners.get(seed, cx, cz, || std::array::from_fn(|i| self.column(seed, ox + (i % 5) as i32 * 4, oz + (i / 5) as i32 * 4).height as f32));
        let stone = self.stone;
        let water = self.sea.map_or(0, |s| s.1);
        for z in 0..16usize {
            for x in 0..16usize {
                let i = z * 16 + x;
                let h = cols.cols[i].height;
                let rule = self.rules.get(cols.rules[i] as usize);
                let bare = rule.and_then(|r| r.bare_above).is_some_and(|y| h > y);
                let mut top = rule.and_then(|r| r.top).unwrap_or(stone);
                if let Some((b, below)) = rule.and_then(|r| r.deep_top) {
                    if h < below {
                        top = b;
                    }
                }
                let filler = rule.and_then(|r| r.filler).unwrap_or(stone);
                let depth = rule.map_or(0, |r| r.depth);
                let under = rule.and_then(|r| r.under);
                let ice = rule.and_then(|r| r.ice);
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
                            Some((b, layers)) if wy < h - depth + layers => b,
                            _ => filler,
                        }
                    } else {
                        stone
                    };
                    if let Some(c) = self.caves.as_ref().filter(|_| carved(y)) {
                        s = match lake(seed, wy).filter(|_| (c.lava_from..c.lava_to).contains(&wy)) {
                            Some(true) => c.lava,
                            Some(false) => stone,
                            None => 0,
                        };
                    }
                    blocks[(y * 16 + z) * 16 + x] = s;
                }
            }
        }
        cols
    }
}

/// Cave fields for the cube, sampled every 4 blocks on a grid aligned to the world and
/// trilinearly interpolated: whether a tunnel passes, and how far inside a cavern (> 0) the
/// block is.
fn cave_fields(seed: u64, ox: i32, oy: i32, oz: i32) -> Vec<(bool, f32)> {
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

const SHARDS: usize = 32;

/// Data computed once per column of cubes and shared by every cube of it. Generation runs on
/// many threads at once, so the map is split into locked shards; a shard that grows past its
/// share of the capacity is simply cleared.
struct ColumnCache<T> {
    shards: [Mutex<FxHashMap<(u64, i32, i32), Arc<T>>>; SHARDS],
    per_shard: usize,
}

impl<T> ColumnCache<T> {
    fn new(capacity: usize) -> ColumnCache<T> {
        ColumnCache { shards: std::array::from_fn(|_| Mutex::new(FxHashMap::default())), per_shard: capacity.div_ceil(SHARDS).max(1) }
    }

    fn shard(&self, x: i32, z: i32) -> &Mutex<FxHashMap<(u64, i32, i32), Arc<T>>> {
        let h = (x as u32).wrapping_mul(0x9E37_79B9) ^ (z as u32).wrapping_mul(0x85EB_CA6B);
        &self.shards[(h >> 27) as usize % SHARDS]
    }

    fn peek(&self, seed: u64, x: i32, z: i32) -> Option<Arc<T>> {
        self.shard(x, z).lock().get(&(seed, x, z)).cloned()
    }

    /// The value for cube column (`x`, `z`) under `seed`, computing it with `make` on a miss.
    /// Two threads missing together may both compute; the result is the same.
    fn get(&self, seed: u64, x: i32, z: i32, make: impl FnOnce() -> T) -> Arc<T> {
        if let Some(v) = self.peek(seed, x, z) {
            return v;
        }
        let v = Arc::new(make());
        let mut map = self.shard(x, z).lock();
        if map.len() >= self.per_shard {
            map.clear();
        }
        map.insert((seed, x, z), v.clone());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::terrain_preset as p;

    fn preset(kind: u32, params: [f64; 4]) -> Feature {
        Feature { name: String::new(), height: Height::Preset(kind, params), mask: None }
    }

    fn rule(biome: u32) -> Rule {
        let any = (f64::NEG_INFINITY, f64::INFINITY);
        Rule { biome, temperature: any, humidity: any, continent: any, height: (i32::MIN, i32::MAX), feature: None, top: None, filler: None, depth: 3, under: None, deep_top: None, bare_above: None, ice: None }
    }

    fn terrain(features: Vec<Feature>, rules: Vec<Rule>, lava: Option<u32>, base: f64) -> Terrain {
        Terrain::new(1, base, None, features, rules, lava.map(|lava| Caves { lava, lava_from: if lava == 1 { 1 } else { i32::MIN }, lava_to: 11 }), Vec::new(), Vec::new())
    }

    fn cube(t: &Terrain, seed: u64, pos: [i32; 3]) -> Vec<u32> {
        let mut blocks = vec![0u32; 4096];
        t.generate(seed, pos, &mut blocks, &mut [0; 256]);
        blocks
    }

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
        let t = terrain(vec![preset(p::HILLS, [16.0, 110.0, 0.0, 0.0]), preset(p::MOUNTAINS, [150.0, 260.0, 0.0, 0.0])], Vec::new(), Some(1), 64.0);
        let (mut columns, mut breached) = (0, 0);
        for cx in -3..3 {
            for cz in -3..3 {
                let heights: Vec<i32> = (0..256).map(|i| t.column(7, cx * 16 + (i % 16) as i32, cz * 16 + (i / 16) as i32).height).collect();
                let top = heights.iter().max().unwrap() >> 4;
                let bottom = (heights.iter().min().unwrap() >> 4) - 1;
                // Air within the column, by height.
                let mut air = std::collections::HashSet::new();
                for cy in bottom..=top {
                    for (i, &b) in cube(&t, 7, [cx, cy, cz]).iter().enumerate() {
                        if b == 0 {
                            air.insert((i % 256, cy * 16 + (i / 256) as i32));
                        }
                    }
                }
                for (i, &h) in heights.iter().enumerate() {
                    if cavern_roof(7, cx * 16 + (i % 16) as i32, cz * 16 + (i / 16) as i32) < 1.0 {
                        continue;
                    }
                    columns += 1;
                    // Open space in the 8 blocks under the surface means the cave reached up.
                    let open = (h - 8..h).filter(|&y| air.contains(&(i, y))).count();
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
        let t = terrain(vec![preset(p::HILLS, [16.0, 110.0, 0.0, 0.0])], Vec::new(), Some(1), 64.0);
        let mut checked = 0;
        for cx in -4..4 {
            for cz in -4..4 {
                for i in 0..256 {
                    let (x, z) = (cx * 16 + (i % 16) as i32, cz * 16 + (i / 16) as i32);
                    let h = t.column(3, x, z).height;
                    let air = cube(&t, 3, [cx, h >> 4, cz])[((h & 15) as usize * 16 + i / 16) * 16 + i % 16] == 0;
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
        let t = terrain(vec![preset(p::HILLS, [16.0, 110.0, 0.0, 0.0])], Vec::new(), Some(7), 64.0);
        let (mut lava, mut hanging) = (0, 0);
        for cx in -3..3 {
            for cz in -3..3 {
                let column: FxHashMap<i32, Vec<u32>> = (-32..1).map(|cy| (cy, cube(&t, 9, [cx, cy, cz]))).collect();
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
        let low = |b| Rule { height: (i32::MIN, 60), ..rule(b) };
        assert_eq!(terrain(Vec::new(), vec![low(1), rule(2)], None, 70.0).column(1, 0, 0).biome, 2);
        assert_eq!(terrain(Vec::new(), vec![low(1), rule(2)], None, 50.0).column(1, 0, 0).biome, 1);
    }

    fn raw(owner: u32, op: u32, anchor: &str, rule: Option<(&str, &str)>) -> RawTerrain {
        let r = rule.unwrap_or(("", ""));
        RawTerrain {
            owner: ModId(owner),
            dimension: "a:dim".into(),
            op,
            anchor: anchor.into(),
            block: if op == op::STONE { "a:stone".into() } else { String::new() },
            level: 0,
            to: 0,
            value: 0.0,
            size: 0.0,
            feature: RawFeature { owner: ModId(owner), name: String::new(), preset: p::HILLS, params: [0.0; 4], height: Callback::NONE, mask: Vec::new() },
            rule: RawRule {
                name: r.0.into(),
                fields: rf::BIOME,
                biome: r.1.into(),
                temperature: (f64::NEG_INFINITY, f64::INFINITY),
                humidity: (f64::NEG_INFINITY, f64::INFINITY),
                continent: (f64::NEG_INFINITY, f64::INFINITY),
                height: (i32::MIN, i32::MAX),
                feature: (String::new(), 0.0, 0.0),
                top: String::new(),
                filler: String::new(),
                depth: 3,
                under: (String::new(), 0),
                deep_top: (String::new(), i32::MIN),
                bare_above: i32::MAX,
                ice: String::new(),
            },
            decorate: None,
        }
    }

    fn built(ops: Vec<RawTerrain>) -> (Vec<u32>, Vec<String>) {
        let biomes = ["a:plains", "a:desert", "b:savanna", "b:dunes", "a:snow"];
        let names = Names { block: &|n| (n == "a:stone").then_some(1), biome: &|n| biomes.iter().position(|b| *b == n).map(|i| i as u32) };
        let mut errors = Vec::new();
        let t = build("a:dim", &|m| format!("mod{}", m.0), ops, &names, &mut errors);
        (t.rules.iter().map(|r| r.biome).collect(), errors)
    }

    /// Edits find rules by name (a biome's own name by default) and keep the owner's order around them.
    #[test]
    fn edits_insert_by_name() {
        let base = || vec![raw(0, op::STONE, "", None), raw(0, op::BIOME, "", Some(("", "a:desert"))), raw(0, op::BIOME, "", Some(("", "a:plains")))];
        let mut ops = base();
        ops.push(raw(1, op::BIOME_BEFORE, "a:desert", Some(("", "b:savanna"))));
        ops.push(raw(1, op::BIOME_AFTER, "a:desert", Some(("", "b:dunes"))));
        assert_eq!(built(ops), (vec![2, 1, 3, 0], vec![]));

        let mut ops = base();
        ops.push(raw(1, op::REPLACE_BIOME, "a:desert", Some(("", "b:dunes"))));
        ops.push(raw(1, op::REMOVE_BIOME, "a:plains", None));
        assert_eq!(built(ops).0, vec![3]);

        // A second rule of the same biome needs a name of its own.
        let mut ops = base();
        ops.push(raw(1, op::BIOME_BEFORE, "a:plains", Some(("", "a:desert"))));
        assert_eq!(built(ops).1.len(), 1);
        let mut ops = base();
        ops.push(raw(1, op::BIOME_BEFORE, "a:plains", Some(("b:hot_coast", "a:desert"))));
        assert_eq!(built(ops), (vec![1, 1, 0], vec![]));

        // Edits whose target is missing are skipped, not errors.
        let mut ops = base();
        ops.push(raw(1, op::BIOME_BEFORE, "a:jungle", Some(("", "b:savanna"))));
        assert_eq!(built(ops), (vec![1, 0], vec![]));

        // Only the given fields change.
        let mut ops = base();
        let mut edit = raw(1, op::EDIT_BIOME, "a:desert", None);
        edit.rule.fields = rf::HEIGHT;
        edit.rule.height = (0, 10);
        ops.push(edit);
        let names = Names { block: &|_| Some(1), biome: &|_| Some(0) };
        let t = build("a:dim", &|_| String::new(), ops, &names, &mut Vec::new());
        assert_eq!((t.rules[0].height, t.rules.len()), ((0, 10), 2));
    }

    #[test]
    fn masks_from_steps() {
        use abi::terrain_mask as k;
        let step = |kind, count| abi::MaskStep { kind, count, ..abi::MaskStep::DEFAULT };
        let m = Mask::from_steps(&[step(k::CONTINENT, 0), step(k::HUMIDITY, 0), step(k::NOT, 0), step(k::ALL, 2)]).unwrap();
        assert!(matches!(&m, Mask::All(v) if v.len() == 2 && matches!(v[1], Mask::Not(_))));
        assert!(Mask::from_steps(&[step(k::ALL, 2)]).is_none());
        assert!(Mask::from_steps(&[]).is_none());
    }
}
