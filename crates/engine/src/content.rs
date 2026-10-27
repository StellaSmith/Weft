//! Frozen registries. Built once from every mod's registrar, then shared read-only.

use std::sync::OnceLock;

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use weft_abi::{self as abi, Callback, Layer, Model, ModId};

use crate::components::{self as comps, Components, RawSet, RawValue, Target};

pub const AIR: u32 = 0;

#[derive(Clone)]
pub struct ModelBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub textures: [u32; 6],
}

pub struct StateInfo {
    pub block: u32,
    pub flags: u32,
    /// Emission per channel (r, g, b).
    pub light: [u8; 3],
    /// Total absorption per channel (r, g, b), opacity included.
    pub absorb: [u8; 3],
    pub opacity: u8,
    pub tint: u8,
    pub tint_faces: u8,
    pub layer: Layer,
    pub level: u8,
    pub model: Model,
    /// Block texture ids (`Content::block_textures`) per face.
    pub textures: [u32; 6],
    pub boxes: Vec<ModelBox>,
    /// In block units.
    pub collision: Vec<[f32; 6]>,
    pub hardness: f32,
    pub friction: f32,
    pub drag: f32,
    /// Has the `conductor` tag (part of energy networks).
    pub conductor: bool,
    /// Liquid id (`Content::liquids`) for liquid blocks.
    pub liquid: Option<u16>,
    /// Face whose neighbor must hold it up, and the tag that neighbor needs (`None`: solid).
    pub support: Option<(u8, Option<u32>)>,
    /// Tag a horizontal neighbor of the support needs (unless the support is the same block).
    pub support_near: Option<u32>,
    /// Sound events break, place, step, hit (`u32::MAX` none), from its `sound` component.
    pub sounds: [u32; 4],
}

impl StateInfo {
    /// The block texture an item of this block shows as a flat sprite (vanilla's generated item
    /// models): plants and liquids. Other shapes are drawn as their model.
    pub fn item_sprite(&self) -> Option<u32> {
        match self.model {
            Model::Cross | Model::None => Some(self.textures[0]),
            Model::Liquid => Some(self.textures[3]),
            _ => None,
        }
    }

    /// Boxes an item of this block is drawn with when it is not a sprite (block units).
    pub fn item_shape(&self) -> Vec<ModelBox> {
        if self.model == Model::Boxes && !self.boxes.is_empty() {
            self.boxes.iter().map(|b| ModelBox { min: b.min, max: b.max, textures: b.textures }).collect()
        } else {
            vec![ModelBox { min: [0.0; 3], max: [1.0; 3], textures: self.textures }]
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct BlockCallbacks {
    pub on_use: Option<Callback>,
    pub on_placed: Option<Callback>,
    pub on_broken: Option<Callback>,
    pub on_random_tick: Option<Callback>,
    pub on_neighbor: Option<Callback>,
    pub on_scheduled: Option<Callback>,
    pub placement: Option<Callback>,
    pub on_grow: Option<Callback>,
    /// Growth over time: (mean ticks per step, light needed).
    pub grow: Option<(f64, u32)>,
}

pub struct BlockDef {
    pub name: String,
    pub owner: ModId,
    pub first_state: u32,
    pub states: u32,
    /// Named properties and their values; states are every combination, the last property
    /// changing fastest.
    pub props: Vec<Prop>,
    /// Resolved after items are frozen. `None` = drops nothing.
    pub drop: Option<u32>,
    pub drop_count: u32,
    pub cb: BlockCallbacks,
    /// The menu using the block opens.
    pub opens: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Prop {
    pub name: String,
    pub values: Vec<String>,
    /// States between two neighboring values.
    pub stride: u32,
}

impl BlockDef {
    /// The value index of every property in state `i` of this block (0-based within the block).
    pub fn prop_values(&self, i: u32) -> impl Iterator<Item = (&Prop, u32)> {
        self.props.iter().map(move |p| (p, (i / p.stride) % p.values.len() as u32))
    }

    /// `state` with property `name` at value index `v`.
    pub fn with_value(&self, state: u32, prop: usize, v: u32) -> u32 {
        let p = &self.props[prop];
        let i = state - self.first_state;
        let cur = (i / p.stride) % p.values.len() as u32;
        self.first_state + i - cur * p.stride + v * p.stride
    }

    /// `name` or `name[a=x,b=y]` with every property.
    pub fn state_name(&self, state: u32) -> String {
        if self.props.is_empty() {
            return self.name.clone();
        }
        let props: Vec<String> = self.prop_values(state - self.first_state).map(|(p, v)| format!("{}={}", p.name, p.values[v as usize])).collect();
        format!("{}[{}]", self.name, props.join(","))
    }

    /// The state with `props` (`a=x` pairs), the others at their defaults.
    pub fn state_of(&self, props: &[(&str, &str)]) -> Result<u32, String> {
        let mut state = self.first_state;
        for (k, v) in props {
            let pi = self.props.iter().position(|p| p.name == *k).ok_or_else(|| format!("{} has no property {k}", self.name))?;
            let vi = self.props[pi].values.iter().position(|x| x == v).ok_or_else(|| format!("{} property {k} has no value {v} (values: {})", self.name, self.props[pi].values.join(", ")))?;
            state = self.with_value(state, pi, vi as u32);
        }
        Ok(state)
    }

    /// States matching `props` (`a=x` pairs, the others any value); an error names what does
    /// not exist.
    pub fn matching(&self, props: &[(&str, &str)]) -> Result<Vec<u32>, String> {
        let mut fixed = Vec::new();
        for (name, value) in props {
            let Some(pi) = self.props.iter().position(|p| p.name == *name) else {
                let known: Vec<&str> = self.props.iter().map(|p| p.name.as_str()).collect();
                return Err(format!("{} has no property {name} (it has: {})", self.name, if known.is_empty() { "none".into() } else { known.join(", ") }));
            };
            let p = &self.props[pi];
            let Some(v) = p.values.iter().position(|x| x == value) else {
                return Err(format!("{} property {name} has no value {value} (values: {})", self.name, p.values.join(", ")));
            };
            fixed.push((p, v as u32));
        }
        Ok((0..self.states).filter(|&i| fixed.iter().all(|(p, v)| (i / p.stride) % p.values.len() as u32 == *v)).map(|i| self.first_state + i).collect())
    }
}

/// Splits `name[a=x,b=y]` into the name and the pairs.
pub fn split_state(text: &str) -> Result<(&str, Vec<(&str, &str)>), String> {
    let Some((name, rest)) = text.split_once('[') else { return Ok((text, Vec::new())) };
    let Some(inner) = rest.strip_suffix(']') else { return Err(format!("{text}: missing ]")) };
    let mut pairs = Vec::new();
    for kv in inner.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((k, v)) = kv.split_once('=') else { return Err(format!("{text}: {kv} is not property=value")) };
        pairs.push((k.trim(), v.trim()));
    }
    Ok((name, pairs))
}

pub struct ItemDef {
    pub name: String,
    pub owner: ModId,
    /// From the `max_stack`, `durability` and `attack_damage` components, read often enough to keep
    /// at hand.
    pub max_stack: u32,
    pub durability: u32,
    pub attack: f32,
    /// Item icon layer; `u32::MAX` when the icon is the 3D block.
    pub icon: u32,
    pub block: Option<u32>,
    pub on_use: Option<Callback>,
    /// Owning mod's load position, then registration order: how creative tabs list items.
    pub order: u64,
    /// The player's equipment slot it is worn in (`worn/<slot>` tag), by index.
    pub equip: Option<u32>,
}

/// A creative inventory tab: translated as `tab.<ns>.<name>`, or the mod's name for the tab a mod
/// gets automatically for items it put in no tab.
/// A liquid the engine flows (`abi::LiquidDesc`); its block has 16 states.
pub struct LiquidDef {
    pub name: String,
    pub first_state: u32,
    pub delay: u32,
    pub drop_off: u32,
    pub slope_find: u32,
    pub renewable: u32,
    /// Lighter than air: flows up.
    pub rises: bool,
    pub temperature: u32,
}

/// A block or any block with a tag.
#[derive(Clone, Debug)]
/// The blocks a reaction side names (any of them matches).
pub struct Match(pub Vec<u32>);

pub struct ReactionDef {
    pub liquid: Match,
    pub touching: Match,
    pub source: Option<u32>,
    pub flowing: Option<u32>,
    pub falls_into: Option<u32>,
}

pub struct CreativeTab {
    pub name: String,
    pub icon: u32,
    pub items: Vec<u32>,
}

pub struct BiomeDef {
    pub name: String,
    /// Color (0xRRGGBB) per tint kind, by `Content::tint_kinds` index.
    pub tints: Vec<u32>,
}

pub struct DimensionDef {
    pub name: String,
    pub owner: ModId,
    pub generator: Option<Callback>,
    pub spawn: Option<Callback>,
    pub column: Option<Callback>,
    pub min_cube_y: i32,
    pub max_cube_y: i32,
    pub sky_cube_y: i32,
    pub sky_light: bool,
    pub gravity: f32,
    pub day_length: u32,
    pub fixed_time: u32,
    pub sky_color: u32,
    pub fog_color: u32,
    pub ambient: u8,
    pub player_spawn: bool,
    /// False for the engine's void: nothing in it is written to the world.
    pub saved: bool,
    /// False for the engine's void: no block can be set there.
    pub editable: bool,
    /// False: blocks never update (`dim_flags::NO_BLOCK_UPDATES`).
    pub block_updates: bool,
    /// The engine's own dimensions generate their cubes here instead of a mod's `generator`.
    pub builtin: Option<fn(&Content, glam::IVec3, &mut [u32])>,
    /// Where players appear without a `spawn` callback.
    pub spawn_at: [i32; 3],
    /// Placed by the engine after `generator`, in name order.
    pub ores: Vec<OreDef>,
    pub structures: Vec<StructureDef>,
    /// Described with `Host::reg_terrain`: fills cubes before `generator` runs.
    pub terrain: Option<Box<crate::server::terrain::Terrain>>,
    /// `dim_flags::BARE`: only ores, scatters and decorations that name it.
    pub bare: bool,
    /// In name order, placed after `generator`.
    pub scatters: Vec<ScatterDef>,
    /// (mod, callback) run after the generator and scatters, in load order.
    pub decorations: Vec<(ModId, Callback)>,
    /// Columns from `column`, for a dimension without a terrain.
    pub column_cache: crate::server::terrain::ColumnCache<[abi::TerrainColumn; 256]>,
}

pub struct ScatterDef {
    pub owner: ModId,
    pub name: String,
    pub cell: i32,
    pub reach: i32,
    /// Biome component (f32) with the chance of a site per cell.
    pub density: Option<u32>,
    pub salt: u64,
    pub shape: Callback,
}

impl DimensionDef {
    /// The 256 columns of cube column (`cx`, `cz`), index z * 16 + x: its terrain's or its
    /// `column` callback's. None for a dimension with neither.
    pub fn columns(&self, seed: u64, cx: i32, cz: i32) -> Option<std::sync::Arc<[abi::TerrainColumn; 256]>> {
        if let Some(t) = &self.terrain {
            return Some(std::sync::Arc::new(t.columns(seed, cx, cz).cols));
        }
        let cb = self.column?;
        Some(self.column_cache.get(seed, cx, cz, || {
            std::array::from_fn(|i| {
                let mut a = abi::ColumnArgs { seed, x: cx * 16 + (i % 16) as i32, z: cz * 16 + (i / 16) as i32, height: i32::MIN, biome: 0 };
                crate::host::invoke(self.owner, cb, &mut a);
                abi::TerrainColumn { height: a.height, biome: a.biome }
            })
        }))
    }

    /// Height and biome of column (`x`, `z`), generated or not.
    pub fn column(&self, seed: u64, x: i32, z: i32) -> Option<abi::TerrainColumn> {
        if let Some(t) = &self.terrain {
            return Some(t.column(seed, x, z));
        }
        let cols = self.columns(seed, x.div_euclid(16), z.div_euclid(16))?;
        Some(cols[(z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize])
    }

    /// Whether caves took the surface block of column (`x`, `z`).
    pub fn carved(&self, seed: u64, x: i32, z: i32) -> bool {
        self.terrain.as_ref().is_some_and(|t| t.surface_carved(seed, x, z))
    }

    /// The terrain's sea level, if it has a sea.
    pub fn sea_level(&self) -> Option<i32> {
        self.terrain.as_ref().and_then(|t| t.sea_level())
    }

    /// Fraction of full daylight (0..1) at world time `time`: a sine over the day, flat at night.
    pub fn daylight(&self, time: u64) -> f32 {
        if !self.sky_light {
            return 0.0;
        }
        let (t, len) = if self.day_length == 0 { (self.fixed_time as f64, 24000.0) } else { ((time % self.day_length as u64) as f64, self.day_length as f64) };
        // 0 = sunrise, 0.25 = noon, 0.75 = midnight.
        ((t / len * std::f64::consts::TAU).sin() * 2.0 + 0.6).clamp(0.0, 1.0) as f32
    }
}

#[derive(Clone)]
pub struct OreDef {
    pub state: u32,
    /// Block ids it may replace, sorted.
    pub replace: Vec<u32>,
    pub min_y: i32,
    pub max_y: i32,
    pub veins: u32,
    pub size: u32,
    /// Derived from the block name, so adding an ore does not move the others.
    pub salt: u64,
    /// Biome tag the block's column must carry.
    pub biome: Option<u32>,
}

pub struct StructureDef {
    pub name: String,
    /// (offset from the lowest corner, state).
    pub blocks: Vec<(glam::IVec3, u32)>,
    pub extent: glam::IVec3,
    pub spacing: i32,
    pub chance: f32,
    pub biome: Option<u32>,
    pub sink: i32,
    pub salt: u64,
}

pub struct ModMeta {
    pub id: String,
    pub version: String,
    pub deps: Vec<String>,
    /// Position when every mod comes after its dependencies (ties by id): the order in which
    /// overrides apply and the later one wins.
    pub order: u32,
}

/// Ids of components the engine reads, resolved once.
#[derive(Default)]
pub struct Ids {
    pub max_stack: Option<u32>,
    pub durability: Option<u32>,
    pub attack_damage: Option<u32>,
    pub mining_speed: Option<u32>,
    pub tool_level: Option<u32>,
    pub harvest_level: Option<u32>,
    pub needs_tool: Option<u32>,
    pub damage: Option<u32>,
    pub name: Option<u32>,
    pub conductor: Option<u32>,
    pub worn: Option<u32>,
    pub worn_texture: Option<u32>,
    pub wear_per_hit: Option<u32>,
}

pub struct Content {
    pub mods: Vec<ModMeta>,
    pub states: Vec<StateInfo>,
    pub blocks: Vec<BlockDef>,
    pub items: Vec<ItemDef>,
    pub biomes: Vec<BiomeDef>,
    /// Word sets command parameters `word(#set)` complete from.
    pub word_sets: FxHashMap<String, Vec<String>>,
    /// Name and default color of each tint kind, kind 1 first.
    pub tint_kinds: Vec<(String, u32)>,
    pub dims: Vec<DimensionDef>,
    pub components: Components,
    pub ids: Ids,
    /// (`mineable/<class>`, `tools/<class>`) component ids per tool class.
    pub tool_classes: Vec<(u32, Option<u32>)>,
    /// Items carrying each tag, the preferred first (`refs::tag_items`).
    pub tag_items: FxHashMap<u32, Vec<u32>>,
    /// Block texture names, index = array layer.
    pub block_textures: Vec<String>,
    /// Block breaking stages, indices into `block_textures`.
    pub crack_textures: [u32; 10],
    /// Animated flames drawn around burning entities.
    pub fire_texture: u32,
    /// Engine commands first, then mods' in name order.
    pub commands: Vec<crate::commands::CommandDef>,
    /// Item icon names, index = array layer.
    pub item_textures: Vec<String>,
    pub block_names: FxHashMap<String, u32>,
    pub item_names: FxHashMap<String, u32>,
    pub dim_names: FxHashMap<String, u32>,
    /// Where players start: the dimension marked `PLAYER_SPAWN` by the mod loaded last, else the
    /// first one registered, else the engine's void (`void_dim`).
    pub spawn_dim: u32,
    pub void_dim: u32,
    pub biome_names: FxHashMap<String, u32>,
    pub creative_tabs: Vec<CreativeTab>,
    pub game: crate::registry::Game,
    pub liquids: Vec<LiquidDef>,
    pub reactions: Vec<ReactionDef>,
    /// Item -> (liquid it holds, the empty container left after pouring).
    pub item_liquid: FxHashMap<u32, (u16, u32)>,
    /// (empty container, liquid) -> filled item.
    pub containers: FxHashMap<(u32, u16), u32>,
    pub loot: FxHashMap<String, crate::loot::Table>,
    /// `abi::stack_flags` of stack components by component id.
    pub stack_flags: FxHashMap<u32, u32>,
}

static CONTENT: OnceLock<Content> = OnceLock::new();

pub fn content() -> &'static Content {
    CONTENT.get().expect("content not frozen yet")
}

pub fn try_content() -> Option<&'static Content> {
    CONTENT.get()
}

pub(crate) fn set(c: Content) {
    if CONTENT.set(c).is_err() {
        panic!("content frozen twice");
    }
}

/// Color (0xRRGGBB) of a tint kind on items, which have no biome: the kind's default.
pub fn item_tint(tint: u8) -> u32 {
    (tint as usize).checked_sub(1).and_then(|i| content().tint_kinds.get(i)).map_or(0xFFFFFF, |k| k.1)
}

impl Content {
    pub fn block_of(&self, state: u32) -> &BlockDef {
        &self.blocks[self.states[state as usize].block as usize]
    }

    /// Whether a block in `state` has growth steps ahead of it: (mean ticks, light needed).
    pub fn grows(&self, state: u32) -> Option<(f64, u32)> {
        let b = self.block_of(state);
        b.cb.grow.filter(|_| b.cb.on_grow.is_some() || self.grown(state).is_some())
    }

    /// The default growth step: the `age` property one higher, `None` at the last age.
    pub fn grown(&self, state: u32) -> Option<u32> {
        let b = self.block_of(state);
        let pi = b.props.iter().position(|p| p.name == "age")?;
        let p = &b.props[pi];
        let v = ((state - b.first_state) / p.stride) % p.values.len() as u32;
        (v + 1 < p.values.len() as u32).then(|| b.with_value(state, pi, v + 1))
    }

    /// The item a middle click on this block picks: the block's own item, else what it drops.
    pub fn pick_item(&self, state: u32) -> Option<u32> {
        let b = self.block_of(state);
        self.items.iter().position(|i| i.block == Some(b.first_state)).map(|i| i as u32).filter(|&i| i != 0).or(b.drop)
    }

    #[inline]
    pub fn block_has(&self, state: u32, comp: Option<u32>) -> bool {
        comp.is_some_and(|c| self.components.has(comps::BLOCK, state, c))
    }

    #[inline]
    pub fn item_has(&self, item: u32, comp: Option<u32>) -> bool {
        comp.is_some_and(|c| self.components.has(comps::ITEM, item, c))
    }

    pub fn item_value<T: Copy>(&self, item: u32, comp: Option<u32>) -> Option<T> {
        self.components.get(comps::ITEM, item, comp?)
    }

    pub fn block_value<T: Copy>(&self, state: u32, comp: Option<u32>) -> Option<T> {
        self.components.get(comps::BLOCK, state, comp?)
    }

    /// Whether `neighbor`, on the face the state rests on, holds it up.
    pub fn supports(&self, state: u32, neighbor: u32) -> bool {
        match &self.states[state as usize].support {
            None => true,
            Some((_, Some(tag))) => self.block_has(neighbor, Some(*tag)),
            Some((_, None)) => self.states[neighbor as usize].flags & abi::block_flags::SOLID != 0,
        }
    }

    /// Worn look of an item: the parts its equipment slot covers, inflation and texture.
    pub fn worn(&self, item: u32) -> Option<(abi::Worn, &str)> {
        let mut w = self.item_value::<abi::Worn>(item, self.ids.worn)?;
        if let Some(e) = self.items.get(item as usize).and_then(|d| d.equip) {
            w.roles = self.game.player_layout.equipment[e as usize].parts;
        }
        Some((w, self.components.text(comps::ITEM, item, self.ids.worn_texture?)?))
    }

    pub fn matches(&self, m: &Match, state: u32) -> bool {
        m.0.contains(&self.states[state as usize].block)
    }

    /// Tool level a block needs (`harvest_level`, see `abi::tool_tier`).
    pub fn harvest_level(&self, state: u32) -> u32 {
        self.block_value::<u32>(state, self.ids.harvest_level).unwrap_or(0)
    }

    /// Drops nothing without a fitting tool: `needs_tool`, or a harvest level above 0.
    pub fn needs_tool(&self, state: u32) -> bool {
        self.block_has(state, self.ids.needs_tool) || self.harvest_level(state) > 0
    }

    pub fn max_stack(&self, item: u32) -> u32 {
        self.items.get(item as usize).map_or(64, |d| d.max_stack)
    }

    pub fn mod_name(&self, id: ModId) -> &str {
        self.mods.get(id.0 as usize).map_or("engine", |m| m.id.as_str())
    }

    /// Full name of a state for saves, the network and players: `base:furnace[facing=north,lit=false]`.
    pub fn state_name(&self, state: u32) -> String {
        self.block_of(state).state_name(state)
    }

    /// `name` (its default state) or `name[a=x,..]`, the properties not given at their defaults.
    pub fn state_by_name(&self, name: &str) -> Option<u32> {
        self.parse_state(name).ok()
    }

    /// Like `state_by_name`, saying what is wrong; a tag gives its preferred block.
    pub fn parse_state(&self, text: &str) -> Result<u32, String> {
        crate::refs::state(&crate::refs::Ref::parse(text)?, &self.block_names, &self.blocks, &self.components)
    }

    /// An item by reference: its name, or the preferred item of a tag (`#ingots/iron`).
    pub fn item_by_ref(&self, text: &str) -> Option<u32> {
        crate::refs::Ref::parse(text).and_then(|r| crate::refs::item(&r, &self.item_names, &self.components, &self.tag_items)).ok()
    }

    /// The value of property `name` in `state`.
    pub fn state_prop(&self, state: u32, name: &str) -> Option<&str> {
        let b = self.block_of(state);
        let p = b.props.iter().find(|p| p.name == name)?;
        Some(&p.values[(((state - b.first_state) / p.stride) % p.values.len() as u32) as usize])
    }

    /// `state` with property `name` set to `value`.
    pub fn state_with(&self, state: u32, name: &str, value: &str) -> Option<u32> {
        let b = self.block_of(state);
        let pi = b.props.iter().position(|p| p.name == name)?;
        let v = b.props[pi].values.iter().position(|x| x == value)?;
        Some(b.with_value(state, pi, v as u32))
    }
}

// ---------------------------------------------------------------- registration side

/// Owned copy of everything one mod registered. Filled without locks, one per mod thread.
pub struct RawRegistrar {
    pub owner: ModId,
    pub blocks: Vec<RawBlock>,
    pub items: Vec<RawItem>,
    pub biomes: Vec<(BiomeDef, usize, Vec<RawValue>, crate::server::terrain::RawRule)>,
    pub dims: Vec<(DimensionDef, usize, Vec<RawValue>)>,
    pub tint_kinds: Vec<(String, u32)>,
    /// Biome, tint kind, color.
    pub biome_tints: Vec<(String, String, u32)>,
    pub game: crate::registry::RawGame,
    pub errors: Vec<String>,
}

#[derive(Clone)]
pub struct RawState {
    pub flags: u32,
    pub light: [u8; 3],
    pub filter: [u8; 3],
    pub opacity: u8,
    pub tint: u8,
    pub tint_faces: u8,
    /// Resolved into `tint` at freeze.
    pub tint_name: String,
    pub layer: Layer,
    pub level: u8,
    pub model: Model,
    pub textures: [String; 6],
    pub boxes: Vec<([f32; 3], [f32; 3], [String; 6])>,
    pub collision: Vec<[f32; 6]>,
    pub hardness: f32,
    pub friction: f32,
    pub drag: f32,
    pub support_side: u8,
    pub support: String,
    pub support_near: String,
    pub components: Vec<RawValue>,
}

pub struct RawBlock {
    pub name: String,
    pub cell: usize,
    pub states: Vec<RawState>,
    pub props: Vec<(String, Vec<String>)>,
    pub components: Vec<RawValue>,
    pub drop: String,
    pub drop_count: u32,
    pub cb: BlockCallbacks,
    pub replace: bool,
    /// False for a replacement that keeps the replaced block's states.
    pub own_states: bool,
    pub retexture: Vec<(String, String)>,
    /// Menu name, see `abi::BlockDesc::opens`.
    pub opens: String,
    /// Handles of blocks this one replaced; they resolve to this block.
    pub extra_cells: Vec<usize>,
}

pub struct RawItem {
    pub name: String,
    pub cell: usize,
    pub seq: u32,
    pub texture: String,
    pub block: String,
    pub components: Vec<RawValue>,
    pub on_use: Option<Callback>,
}

impl RawRegistrar {
    pub fn new(owner: ModId) -> Self {
        RawRegistrar {
            owner,
            blocks: Vec::new(),
            items: Vec::new(),
            biomes: Vec::new(),
            tint_kinds: Vec::new(),
            biome_tints: Vec::new(),
            dims: Vec::new(),
            game: Default::default(),
            errors: Vec::new(),
        }
    }
}

fn cb(c: Callback) -> Option<Callback> {
    c.is_some().then_some(c)
}

/// A replacement without states of its own keeps the replaced block's: their shapes, textures
/// (renamed by `retexture`), flags, support rules and callbacks it does not set, with its own
/// light, hardness, friction and drag on every state. A sixth state added to the original reaches
/// the replacement too.
fn inherit_states(b: &mut RawBlock, old: &RawBlock) {
    let own = b.states[0].clone();
    if b.opens.is_empty() {
        b.opens = old.opens.clone();
    }
    let rename = |t: &mut String| {
        if let Some((_, to)) = b.retexture.iter().find(|r| &r.0 == t) {
            *t = to.clone();
        }
    };
    b.props = old.props.clone();
    b.states = old
        .states
        .iter()
        .map(|o| {
            let mut s = o.clone();
            (s.light, s.hardness, s.friction, s.drag) = (own.light, own.hardness, own.friction, own.drag);
            s.textures.iter_mut().for_each(rename);
            for (_, _, t) in &mut s.boxes {
                t.iter_mut().for_each(rename);
            }
            s
        })
        .collect();
    let (n, o) = (&mut b.cb, &old.cb);
    for (mine, theirs) in [(&mut n.on_use, o.on_use), (&mut n.on_placed, o.on_placed), (&mut n.on_broken, o.on_broken), (&mut n.on_random_tick, o.on_random_tick), (&mut n.on_neighbor, o.on_neighbor), (&mut n.on_scheduled, o.on_scheduled), (&mut n.placement, o.placement), (&mut n.on_grow, o.on_grow)] {
        if mine.is_none() {
            *mine = theirs;
        }
    }
    let mut comps = old.components.clone();
    comps.append(&mut b.components);
    b.components = comps;
}

pub(crate) unsafe fn copy_block(d: &abi::BlockDesc) -> RawBlock {
    unsafe {
        let mut states = Vec::new();
        d.states.as_slice().iter().copied().for_each(|s| {
            let mut boxes = Vec::new();
            s.boxes.as_slice().iter().copied().for_each(|b| boxes.push((b.min, b.max, std::array::from_fn(|i| b.textures[i].as_str().to_string()))));
            states.push(RawState {
                flags: s.flags,
                light: s.light,
                filter: s.filter,
                opacity: s.opacity,
                tint: 0,
                tint_faces: s.tint_faces,
                tint_name: s.tint_name.as_str().to_string(),
                layer: s.layer,
                level: s.level,
                model: s.model,
                textures: std::array::from_fn(|i| s.textures[i].as_str().to_string()),
                boxes,
                collision: s.collision.as_slice().to_vec(),
                hardness: s.hardness,
                friction: s.friction,
                drag: s.drag,
                support_side: s.support_side,
                support: s.support.as_str().to_string(),
                support_near: s.support_near.as_str().to_string(),
                components: RawValue::list(s.components),
            })
        });
        RawBlock {
            name: d.name.as_str().to_string(),
            cell: d.cell as usize,
            states,
            props: d.props.as_slice().iter().map(|p| (p.name.as_str().to_string(), p.values.as_slice().iter().map(|v| v.as_str().to_string()).collect())).collect(),
            components: RawValue::list(d.components),
            drop: d.drop.as_str().to_string(),
            drop_count: d.drop_count,
            cb: BlockCallbacks {
                on_use: cb(d.on_use),
                on_placed: cb(d.on_placed),
                on_broken: cb(d.on_broken),
                on_random_tick: cb(d.on_random_tick),
                on_neighbor: cb(d.on_neighbor),
                on_scheduled: cb(d.on_scheduled),
                placement: cb(d.placement),
                on_grow: cb(d.on_grow),
                grow: (d.grow_seconds > 0.0).then_some((d.grow_seconds as f64 * 20.0, d.grow_light)),
            },
            replace: d.replace != 0,
            own_states: d.own_states != 0,
            retexture: d.retexture.as_slice().chunks_exact(2).map(|p| (p[0].as_str().to_string(), p[1].as_str().to_string())).collect(),
            opens: d.opens.as_str().to_string(),
            extra_cells: Vec::new(),
        }
    }
}

pub(crate) unsafe fn copy_item(d: &abi::ItemDesc) -> RawItem {
    unsafe {
        RawItem {
            name: d.name.as_str().to_string(),
            cell: d.cell as usize,
            seq: 0,
            texture: d.texture.as_str().to_string(),
            block: d.block.as_str().to_string(),
            components: RawValue::list(d.components),
            on_use: cb(d.on_use),
        }
    }
}

pub(crate) unsafe fn copy_dimension(owner: ModId, d: &abi::DimensionDesc) -> (DimensionDef, usize, Vec<RawValue>) {
    unsafe {
        (
            DimensionDef {
                name: d.name.as_str().to_string(),
                owner,
                generator: cb(d.generator),
                spawn: cb(d.spawn),
                column: cb(d.column),
                min_cube_y: d.min_cube_y,
                max_cube_y: d.max_cube_y,
                sky_cube_y: d.sky_cube_y,
                sky_light: d.sky_light != 0,
                gravity: d.gravity,
                day_length: d.day_length,
                fixed_time: d.fixed_time,
                sky_color: d.sky_color,
                fog_color: d.fog_color,
                ambient: d.ambient.min(15) as u8,
                player_spawn: d.flags & abi::dim_flags::PLAYER_SPAWN != 0,
                saved: true,
                editable: true,
                block_updates: d.flags & abi::dim_flags::NO_BLOCK_UPDATES == 0,
                builtin: None,
                spawn_at: [0, 64, 0],
                ores: Vec::new(),
                structures: Vec::new(),
                terrain: None,
                bare: d.flags & abi::dim_flags::BARE != 0,
                scatters: Vec::new(),
                decorations: Vec::new(),
                column_cache: crate::server::terrain::ColumnCache::new(4096),
            },
            d.cell as usize,
            RawValue::list(d.components),
        )
    }
}

pub const VOID_DIM: &str = "core:void";

fn void_dimension() -> DimensionDef {
    DimensionDef {
        name: VOID_DIM.into(),
        owner: crate::registry::ENGINE,
        generator: None,
        spawn: None,
        column: None,
        min_cube_y: -4,
        max_cube_y: 4,
        sky_cube_y: i32::MIN,
        sky_light: false,
        gravity: 0.0,
        day_length: 0,
        fixed_time: 18000,
        sky_color: 0x0a0a12,
        fog_color: 0x0a0a12,
        ambient: 15,
        player_spawn: false,
        saved: false,
        editable: false,
        block_updates: false,
        builtin: None,
        spawn_at: [0, 64, 0],
        ores: Vec::new(),
        structures: Vec::new(),
        terrain: None,
        bare: true,
        scatters: Vec::new(),
        decorations: Vec::new(),
        column_cache: crate::server::terrain::ColumnCache::new(1),
    }
}

pub const DEBUG_DIM: &str = "core:debug_blocks";

/// Every block state in a grid two blocks apart on a floor, like vanilla's debug world: for
/// checking textures and models at a glance. Nothing in it updates or changes.
fn debug_dimension() -> DimensionDef {
    DimensionDef {
        name: DEBUG_DIM.into(),
        sky_cube_y: 1,
        sky_light: true,
        gravity: 32.0,
        day_length: 24000,
        fixed_time: 6000,
        sky_color: 0x78a7ff,
        fog_color: 0xc0d8ff,
        ambient: 0,
        min_cube_y: -1,
        max_cube_y: 1,
        builtin: Some(debug_blocks),
        spawn_at: [-2, 0, -2],
        ..void_dimension()
    }
}

fn debug_blocks(c: &Content, cube: glam::IVec3, blocks: &mut [u32]) {
    if cube.y != 0 {
        return;
    }
    let floor = c.states.iter().position(|s| s.flags & abi::block_flags::OPAQUE != 0 && s.model == abi::Model::Cube).unwrap_or(0) as u32;
    let n = c.states.len() as i32 - 1;
    let side = (n as f64).sqrt().ceil() as i32;
    for z in 0..16 {
        for x in 0..16 {
            // The floor reaches a little past the grid, so players can walk around it.
            let (wx, wz) = (cube.x * 16 + x, cube.z * 16 + z);
            if !(-4..side * 2 + 4).contains(&wx) || !(-4..side * 2 + 4).contains(&wz) {
                continue;
            }
            blocks[(z * 16 + x) as usize] = floor;
            if !(0..side * 2).contains(&wx) || !(0..side * 2).contains(&wz) {
                continue;
            }
            let i = (wz / 2) * side + wx / 2;
            if wx % 2 == 0 && wz % 2 == 0 && i < n {
                blocks[(256 + z * 16 + x) as usize] = i as u32 + 1;
            }
        }
    }
}

fn write_cell(cell: usize, id: u32) {
    if cell != 0 {
        unsafe { (*(cell as *const std::sync::atomic::AtomicU32)).store(id, std::sync::atomic::Ordering::Relaxed) }
    }
}

/// Every mod after the mods it depends on, ties by id.
pub fn load_order(mods: &[ModMeta]) -> Vec<u32> {
    let mut placed: Vec<Option<u32>> = vec![None; mods.len()];
    let mut next = 0;
    while next < mods.len() as u32 {
        let ready = (0..mods.len()).find(|&i| placed[i].is_none() && mods[i].deps.iter().all(|d| mods.iter().position(|m| &m.id == d).is_none_or(|j| placed[j].is_some())));
        // A dependency cycle: place the rest by id rather than loop forever.
        let i = ready.unwrap_or_else(|| (0..mods.len()).find(|&i| placed[i].is_none()).unwrap());
        placed[i] = Some(next);
        next += 1;
    }
    placed.into_iter().map(|p| p.unwrap()).collect()
}

/// Registered tabs in mod order, filled with their tagged items in registration order; every mod
/// with leftover items gets one more tab of its own.
fn creative_tabs(mods: &[ModMeta], games: &[crate::registry::RawGame], items: &[ItemDef], item_names: &FxHashMap<String, u32>, c: &Components, errors: &mut Vec<String>) -> Vec<CreativeTab> {
    let mut tabs: Vec<CreativeTab> = Vec::new();
    for g in games {
        for (name, icon) in &g.creative_tabs {
            let Some(&icon) = item_names.get(icon) else {
                errors.push(format!("creative tab {name}: unknown icon item {icon}"));
                continue;
            };
            tabs.push(CreativeTab { name: name.clone(), icon, items: Vec::new() });
        }
    }
    // The tag `tab/<name>` of each tab, by component id.
    let tab_of: FxHashMap<u32, usize> = tabs.iter().enumerate().filter_map(|(i, t)| c.id(&format!("tab/{}", t.name)).map(|id| (id, i))).collect();
    let mut ordered: Vec<u32> = (1..items.len() as u32).collect();
    ordered.sort_by_key(|&i| items[i as usize].order);
    let mut leftover: Vec<(usize, Vec<u32>)> = Vec::new();
    for i in ordered {
        let def = &items[i as usize];
        // A misspelt tab would quietly send the item to misc.
        for id in c.of(comps::ITEM, i).filter(|id| !tab_of.contains_key(id)) {
            if let Some(tab) = c.names[id as usize].strip_prefix("tab/") {
                errors.push(format!("{}: {} is in creative tab {tab}, which no mod registers", mods.get(def.owner.0 as usize).map_or("core", |m| m.id.as_str()), def.name));
            }
        }
        match c.of(comps::ITEM, i).find_map(|id| tab_of.get(&id).copied()) {
            Some(t) => tabs[t].items.push(i),
            None => match leftover.iter_mut().find(|l| l.0 == def.owner.0 as usize) {
                Some(l) => l.1.push(i),
                None => leftover.push((def.owner.0 as usize, vec![i])),
            },
        }
    }
    for (m, list) in leftover {
        let name = format!("{}:misc", mods.get(m).map_or("core", |m| m.id.as_str()));
        tabs.push(CreativeTab { name, icon: list[0], items: list });
    }
    tabs.retain(|t| !t.items.is_empty());
    for t in &tabs {
        log::debug!("creative tab {}: {}", t.name, t.items.iter().map(|&i| items[i as usize].name.as_str()).collect::<Vec<_>>().join(" "));
    }
    tabs
}

/// Merges per-mod registrars into frozen content. Ids are assigned in name order, so they do not
/// depend on which mod finished registering first.
pub fn freeze(mut mods: Vec<ModMeta>, regs: Vec<RawRegistrar>) -> Result<Content, Vec<String>> {
    let order = load_order(&mods);
    for (m, o) in mods.iter_mut().zip(&order) {
        m.order = *o;
    }
    let mut errors: Vec<String> = Vec::new();
    let mut blocks: Vec<(ModId, RawBlock)> = Vec::new();
    let mut items: Vec<(ModId, RawItem)> = Vec::new();
    let mut biomes: Vec<(BiomeDef, usize, Vec<RawValue>, crate::server::terrain::RawRule)> = Vec::new();
    let mut dims: Vec<(DimensionDef, usize, Vec<RawValue>)> = Vec::new();
    let mut tint_kinds: Vec<(String, u32)> = vec![("core:grass".into(), 0x8db360), ("core:foliage".into(), 0x77ab2f), ("core:water".into(), 0x3f76e4)];
    let mut biome_tints = Vec::new();
    let mut games = Vec::new();
    for r in regs {
        games.push(r.game);
        tint_kinds.extend(r.tint_kinds);
        biome_tints.extend(r.biome_tints.into_iter().map(|t| (mods[r.owner.0 as usize].order, t)));
        errors.extend(r.errors.into_iter().map(|e| format!("{}: {e}", mods[r.owner.0 as usize].id)));
        blocks.extend(r.blocks.into_iter().map(|b| (r.owner, b)));
        items.extend(r.items.into_iter().map(|i| (r.owner, i)));
        biomes.extend(r.biomes);
        dims.extend(r.dims);
    }
    blocks.par_sort_by(|a, b| a.1.name.cmp(&b.1.name).then(a.1.replace.cmp(&b.1.replace)));
    items.par_sort_by(|a, b| a.1.name.cmp(&b.1.name));
    // A replacement wins over the original if its mod depends on the original's mod. The
    // original's item stays (it already points at the block by name).
    let depends = |m: ModId, on: ModId| mods[m.0 as usize].deps.contains(&mods[on.0 as usize].id);
    let mut kept: Vec<(ModId, RawBlock)> = Vec::with_capacity(blocks.len());
    for (owner, b) in blocks {
        match kept.last() {
            Some((prev_owner, prev)) if prev.name == b.name && b.replace && depends(owner, *prev_owner) => {
                log::info!("{} replaces block {}", mods[owner.0 as usize].id, b.name);
                let (_, old) = kept.pop().unwrap();
                let mut b = b;
                if !b.own_states {
                    inherit_states(&mut b, &old);
                    for (_, item) in items.iter_mut().filter(|(_, i)| i.name == b.name || i.block == b.name) {
                        if let Some((_, to)) = b.retexture.iter().find(|r| r.0 == item.texture) {
                            item.texture = to.clone();
                        }
                    }
                }
                b.extra_cells.push(old.cell);
                b.extra_cells.extend(old.extra_cells);
                kept.push((owner, b));
            }
            _ => kept.push((owner, b)),
        }
    }
    let mut blocks = kept;
    if tint_kinds.len() > abi::tint::MAX as usize {
        errors.push(format!("{} tint kinds registered, at most {} fit", tint_kinds.len(), abi::tint::MAX));
    }
    for (i, (name, _)) in tint_kinds.iter().enumerate() {
        if tint_kinds[..i].iter().any(|k| &k.0 == name) {
            errors.push(format!("tint kind {name} registered twice"));
        }
    }
    let tint_id = |name: &str| tint_kinds.iter().position(|k| k.0 == name).filter(|i| *i < abi::tint::MAX as usize).map(|i| i as u8 + 1);
    for (_, b) in &mut blocks {
        for s in b.states.iter_mut().filter(|s| !s.tint_name.is_empty()) {
            match tint_id(&s.tint_name) {
                Some(id) => s.tint = id,
                None => errors.push(format!("block {}: no tint kind {}", b.name, s.tint_name)),
            }
        }
    }
    for (b, ..) in &mut biomes {
        b.tints.extend(tint_kinds.iter().map(|k| k.1));
    }
    // In load order: a mod's colors win over those of the mods it depends on.
    biome_tints.sort_by_key(|t| t.0);
    for (_, (biome, kind, color)) in biome_tints {
        match (biomes.iter_mut().find(|b| b.0.name == biome), tint_id(&kind)) {
            (Some(b), Some(t)) => b.0.tints[t as usize - 1] = color,
            _ => errors.push(format!("biome tint: no biome {biome} or tint kind {kind}")),
        }
    }
    tint_kinds.truncate(abi::tint::MAX as usize);
    let mut seen = rustc_hash::FxHashSet::default();
    items.retain(|(_, i)| seen.insert(i.name.clone()));
    biomes.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    dims.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    let mod_name = |m: ModId| mods[m.0 as usize].id.clone();
    for w in blocks.windows(2) {
        if w[0].1.name == w[1].1.name {
            errors.push(format!("block {} registered by both {} and {}", w[0].1.name, mod_name(w[0].0), mod_name(w[1].0)));
        }
    }
    for w in items.windows(2) {
        if w[0].1.name == w[1].1.name {
            errors.push(format!("item {} registered by both {} and {}", w[0].1.name, mod_name(w[0].0), mod_name(w[1].0)));
        }
    }
    // Cubes and face records keep 16 bits; the top one stays free.
    if biomes.len() > 32768 {
        errors.push(format!("{} biomes registered, at most 32768 fit", biomes.len()));
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut block_textures: Vec<String> = vec!["core:missing".into()];
    let mut tex_index: FxHashMap<String, u32> = FxHashMap::default();
    tex_index.insert("core:missing".into(), 0);
    let mut tex = |name: &str| -> u32 {
        if name.is_empty() {
            return 0;
        }
        if let Some(&i) = tex_index.get(name) {
            return i;
        }
        let i = block_textures.len() as u32;
        block_textures.push(name.to_string());
        tex_index.insert(name.to_string(), i);
        i
    };
    let crack_textures: [u32; 10] = std::array::from_fn(|i| tex(&format!("core:block/destroy_{i}")));
    let fire_texture = tex("core:block/fire");

    let mut states = vec![StateInfo {
        block: 0,
        flags: abi::block_flags::REPLACEABLE | abi::block_flags::NOT_TARGETABLE,
        light: [0; 3],
        absorb: [0; 3],
        opacity: 0,
        tint: 0,
        tint_faces: 0,
        layer: Layer::Opaque,
        level: 0,
        model: Model::None,
        textures: [0; 6],
        boxes: Vec::new(),
        collision: Vec::new(),
        hardness: -1.0,
        friction: 0.6,
        drag: 1.0,
        conductor: false,
        liquid: None,
        support: None,
        support_near: None,
        sounds: [u32::MAX; 4],
    }];
    // What each registered block opens, by menu name (resolved once menus are frozen).
    let mut opens_names: Vec<String> = Vec::new();
    let mut block_defs = vec![BlockDef {
        name: "core:air".into(),
        owner: ModId(u32::MAX),
        first_state: 0,
        states: 1,
        props: Vec::new(),
        opens: None,
        drop: None,
        drop_count: 0,
        cb: BlockCallbacks::default(),
    }];
    let mut block_drops = vec![String::new()];
    // Per block: (block components, per-state components, support tags) for the component pass.
    let mut block_values: Vec<(ModId, Vec<RawValue>, Vec<Vec<RawValue>>, Vec<(String, String)>)> = vec![(ModId(u32::MAX), Vec::new(), vec![Vec::new()], vec![(String::new(), String::new())])];
    for (owner, b) in blocks {
        let index = block_defs.len() as u32;
        let first = states.len() as u32;
        write_cell(b.cell, first);
        for &cell in &b.extra_cells {
            write_cell(cell, first);
        }
        let mut state_values = Vec::new();
        let mut supports = Vec::new();
        for s in b.states {
            let mut boxes: Vec<ModelBox> = s
                .boxes
                .iter()
                .map(|(min, max, t)| ModelBox {
                    min: min.map(|v| v / 16.0),
                    max: max.map(|v| v / 16.0),
                    textures: std::array::from_fn(|i| tex(&t[i])),
                })
                .collect();
            let mut collision: Vec<[f32; 6]> = s.collision.iter().map(|c| c.map(|v| v / 16.0)).collect();
            if collision.is_empty() && s.model == Model::Boxes && !boxes.is_empty() {
                let mut u = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
                for bx in &boxes {
                    for i in 0..3 {
                        u[i] = u[i].min(bx.min[i]);
                        u[i + 3] = u[i + 3].max(bx.max[i]);
                    }
                }
                collision.push(u);
            }
            boxes.shrink_to_fit();
            // Random ticks reach blocks that have the callback, inherited ones included.
            let random = if b.cb.on_random_tick.is_some() { abi::block_flags::RANDOM_TICKS } else { 0 };
            states.push(StateInfo {
                block: index,
                flags: s.flags | random,
                light: s.light,
                absorb: std::array::from_fn(|i| s.opacity.saturating_add(s.filter[i])),
                opacity: s.opacity,
                tint: s.tint,
                tint_faces: s.tint_faces,
                layer: s.layer,
                level: s.level,
                model: s.model,
                // Box models carry textures per box; the state-level ones are unused.
                textures: if matches!(s.model, Model::Boxes | Model::None) {
                    [0; 6]
                } else {
                    std::array::from_fn(|i| tex(&s.textures[i]))
                },
                boxes,
                collision,
                hardness: s.hardness,
                friction: s.friction,
                drag: s.drag,
                conductor: false,
                liquid: None,
                // The tag id is filled in once components are frozen.
                support: (s.support_side < 6).then_some((s.support_side, None)),
                support_near: None,
                sounds: [u32::MAX; 4],
            });
            // Support rules name tags (`#plant_soil`); the tag exists by being named here.
            let mut support_tag = |t: String| -> String {
                if t.is_empty() {
                    return t;
                }
                crate::refs::tag_name(&t).map(str::to_string).unwrap_or_else(|e| {
                    errors.push(format!("{}: block {}: support: {e}", mods.get(owner.0 as usize).map_or("engine", |m| m.id.as_str()), b.name));
                    String::new()
                })
            };
            supports.push((support_tag(s.support), support_tag(s.support_near)));
            state_values.push(s.components);
        }
        let drop = if b.drop.is_empty() { b.name.clone() } else { b.drop.clone() };
        block_drops.push(drop);
        block_values.push((owner, b.components, state_values, supports));
        let count = (states.len() as u32) - first;
        let mut props = Vec::new();
        let mut stride = 1;
        for (name, values) in b.props.iter().rev() {
            props.push(Prop { name: name.clone(), values: values.clone(), stride });
            stride *= values.len().max(1) as u32;
        }
        props.reverse();
        if stride != count {
            errors.push(format!("{}: block {} has {count} states, its properties make {stride}", mods.get(owner.0 as usize).map_or("engine", |m| m.id.as_str()), b.name));
        }
        if b.cb.grow.is_some() && b.cb.on_grow.is_none() && !b.props.iter().any(|p| p.0 == "age") {
            errors.push(format!("{}: block {} grows, but has no `age` property and no on_grow", mods.get(owner.0 as usize).map_or("engine", |m| m.id.as_str()), b.name));
        }
        opens_names.push(b.opens.clone());
        block_defs.push(BlockDef {
            opens: None,
            name: b.name,
            owner,
            first_state: first,
            states: count,
            props,
            drop: None,
            drop_count: b.drop_count,
            cb: b.cb,
        });
    }

    let block_names: FxHashMap<String, u32> =
        block_defs.iter().enumerate().map(|(i, b)| (b.name.clone(), i as u32)).collect();

    let mut item_textures: Vec<String> = vec!["core:missing".into()];
    let mut item_tex_index: FxHashMap<String, u32> = FxHashMap::default();
    let mut item_defs = vec![ItemDef {
        name: "core:empty".into(),
        owner: ModId(u32::MAX),
        max_stack: 64,
        durability: 0,
        attack: 1.0,
        icon: 0,
        block: None,
        on_use: None,
        order: 0,
        equip: None,
    }];
    let mut item_values: Vec<(ModId, Vec<RawValue>)> = vec![(ModId(u32::MAX), Vec::new())];
    for (owner, it) in items {
        let id = item_defs.len() as u32;
        write_cell(it.cell, id);
        let block = if it.block.is_empty() {
            None
        } else {
            match block_names.get(&it.block) {
                Some(&b) => Some(block_defs[b as usize].first_state),
                None => {
                    errors.push(format!("item {} places unknown block {}", it.name, it.block));
                    None
                }
            }
        };
        let icon = if it.texture.is_empty() && block.is_some() {
            u32::MAX
        } else {
            *item_tex_index.entry(it.texture.clone()).or_insert_with(|| {
                item_textures.push(it.texture.clone());
                item_textures.len() as u32 - 1
            })
        };
        item_values.push((owner, it.components));
        item_defs.push(ItemDef {
            name: it.name,
            owner,
            max_stack: 64,
            durability: 0,
            attack: 1.0,
            icon,
            block,
            on_use: it.on_use,
            order: (mods[owner.0 as usize].order as u64) << 32 | it.seq as u64,
            equip: None,
        });
    }
    let item_names: FxHashMap<String, u32> =
        item_defs.iter().enumerate().map(|(i, b)| (b.name.clone(), i as u32)).collect();
    for (b, drop) in block_defs.iter_mut().zip(block_drops) {
        if drop == "-" || drop.is_empty() {
            continue;
        }
        b.drop = item_names.get(&drop).copied();
        if b.drop.is_none() && drop != b.name {
            errors.push(format!("block {} drops unknown item {drop}", b.name));
        }
    }

    let mut biome_values = Vec::new();
    let mut biome_rules = Vec::new();
    let biome_defs: Vec<BiomeDef> = biomes
        .into_iter()
        .enumerate()
        .map(|(i, (b, cell, v, rule))| {
            write_cell(cell, i as u32);
            biome_values.push(v);
            biome_rules.push(rule);
            b
        })
        .collect();
    let biome_names: FxHashMap<String, u32> = biome_defs.iter().enumerate().map(|(i, b)| (b.name.clone(), i as u32)).collect();
    let mut dim_values = Vec::new();
    let mut dim_defs: Vec<DimensionDef> = dims
        .into_iter()
        .enumerate()
        .map(|(i, (d, cell, v))| {
            write_cell(cell, i as u32);
            dim_values.push((d.owner, v));
            d
        })
        .collect();
    // Always there, so a dimension lookup never fails: where players go when no mod adds a
    // dimension (or theirs is gone).
    let void_dim = dim_defs.len() as u32;
    dim_defs.push(void_dimension());
    dim_values.push((crate::registry::ENGINE, Vec::new()));
    dim_defs.push(debug_dimension());
    dim_values.push((crate::registry::ENGINE, Vec::new()));
    let order_of_mod = |m: ModId| mods.get(m.0 as usize).map_or(0, |m| m.order);
    let spawn_dim = dim_defs[..void_dim as usize]
        .iter()
        .enumerate()
        .filter(|(_, d)| d.player_spawn)
        .max_by_key(|(_, d)| order_of_mod(d.owner))
        .or_else(|| dim_defs[..void_dim as usize].iter().enumerate().min_by_key(|(_, d)| order_of_mod(d.owner)))
        .map_or(void_dim, |(i, _)| i as u32);
    let dim_names: FxHashMap<String, u32> = dim_defs.iter().enumerate().map(|(i, d)| (d.name.clone(), i as u32)).collect();

    let entities = crate::registry::freeze_entities(&mut games, &mut errors);

    // ------------------------------------------------ components
    let mut cb = comps::Builder::default();
    let who = |m: ModId, entry: &str| format!("{}: {entry}", mods.get(m.0 as usize).map_or("engine", |m| m.id.as_str()));
    for (i, (owner, bv, sv, supports)) in block_values.iter().enumerate() {
        let name = &block_defs[i].name;
        for v in bv.iter().chain(sv.iter().flatten()) {
            cb.declare(&v.name, &v.schema, v.text.is_none().then_some(v.bytes.len()), &|| who(*owner, name), &mut errors);
        }
        // A support tag nobody else uses still needs an id (nothing will carry it).
        for t in supports.iter().flat_map(|(a, b)| [a, b]).filter(|t| !t.is_empty()) {
            cb.declare(t, "", None, &|| who(*owner, name), &mut errors);
        }
    }
    for (i, (owner, v)) in item_values.iter().enumerate() {
        for v in v {
            cb.declare(&v.name, &v.schema, v.text.is_none().then_some(v.bytes.len()), &|| who(*owner, &item_defs[i].name), &mut errors);
        }
    }
    for (k, (owner, v)) in entities.values.iter().enumerate() {
        for v in v {
            cb.declare(&v.name, &v.schema, v.text.is_none().then_some(v.bytes.len()), &|| who(*owner, &entities.kinds[k].name), &mut errors);
        }
    }
    for (i, v) in biome_values.iter().enumerate() {
        for v in v {
            cb.declare(&v.name, &v.schema, v.text.is_none().then_some(v.bytes.len()), &|| format!("biome {}", biome_defs[i].name), &mut errors);
        }
    }
    for (i, (owner, v)) in dim_values.iter().enumerate() {
        for v in v {
            cb.declare(&v.name, &v.schema, v.text.is_none().then_some(v.bytes.len()), &|| who(*owner, &dim_defs[i].name), &mut errors);
        }
    }
    let mut sets: Vec<RawSet> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.sets)).collect();
    // Stable sort keeps registration order within a mod.
    sets.sort_by_key(|s| mods[s.owner.0 as usize].order);
    for s in &sets {
        cb.declare(&s.value.name, &s.value.schema, (!s.remove && s.value.text.is_none()).then_some(s.value.bytes.len()), &|| who(s.owner, &format!("set on {:?}", s.target)), &mut errors);
    }
    for g in &games {
        // Values recipes carry may be on nothing else.
        for v in g.recipes.iter().flat_map(|r| &r.values) {
            cb.declare(&v.name, &v.schema, v.text.is_none().then_some(v.bytes.len()), &|| format!("a {} recipe", v.name), &mut errors);
        }
        for rc in &g.components {
            cb.declare(&rc.def.name, &rc.schema, Some(rc.def.size as usize), &|| who(rc.owner, "entity component"), &mut errors);
        }
    }
    cb.declare(crate::registry::MODIFIERS, "bytes", None, &|| "the engine".into(), &mut errors);
    cb.declare(abi::comp::NAME, "bytes", None, &|| "the engine".into(), &mut errors);
    for g in &games {
        for (name, schema, _) in &g.stack_components {
            cb.declare(name, schema, None, &|| format!("stack component {name}"), &mut errors);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    cb.start([states.len(), item_defs.len(), entities.kinds.len(), biome_defs.len(), dim_defs.len()]);
    for (i, (_, bv, sv, _)) in block_values.iter().enumerate() {
        let d = &block_defs[i];
        for (k, values) in sv.iter().enumerate() {
            let state = d.first_state + k as u32;
            for v in bv.iter().chain(values) {
                cb.put(comps::BLOCK, state, v, None, &|| d.name.clone());
            }
        }
    }
    for (i, (_, v)) in item_values.iter().enumerate() {
        for v in v {
            cb.put(comps::ITEM, i as u32, v, None, &|| item_defs[i].name.clone());
        }
    }
    // A block's item carries its block's components, unless it sets the same ones itself.
    for (i, d) in item_defs.iter().enumerate() {
        if let Some(s) = d.block {
            cb.inherit((comps::BLOCK, s), (comps::ITEM, i as u32));
        }
    }
    for (k, (_, v)) in entities.values.iter().enumerate() {
        for v in v {
            cb.put(comps::ENTITY, k as u32, v, None, &|| entities.kinds[k].name.clone());
        }
    }
    for (i, v) in biome_values.iter().enumerate() {
        for v in v {
            cb.put(comps::BIOME, i as u32, v, None, &|| biome_defs[i].name.clone());
        }
    }
    for (i, (_, v)) in dim_values.iter().enumerate() {
        for v in v {
            cb.put(comps::DIMENSION, i as u32, v, None, &|| dim_defs[i].name.clone());
        }
    }
    // Sets by name first, then by tag, so a tag added by name is seen by tag patterns.
    for pass in 0..2 {
        for s in &sets {
            if matches!(s.target, Target::Tagged(_)) != (pass == 1) {
                continue;
            }
            let m = (s.owner, mods[s.owner.0 as usize].id.as_str());
            let mut hits: Vec<(usize, u32)> = Vec::new();
            match &s.target {
                Target::Name(n) => {
                    // `name[prop=value]` narrows a block to the states with those values.
                    let (base, props) = match split_state(n) {
                        Ok(x) => x,
                        Err(e) => {
                            errors.push(format!("{}: component {}: {e}", m.1, s.value.name));
                            continue;
                        }
                    };
                    if let Some(&b) = block_names.get(base) {
                        match block_defs[b as usize].matching(&props) {
                            Ok(list) => hits.extend(list.into_iter().map(|st| (comps::BLOCK, st))),
                            Err(e) => errors.push(format!("{}: component {}: {e}", m.1, s.value.name)),
                        }
                    }
                    if let (Some(&i), true) = (item_names.get(base), props.is_empty()) {
                        hits.push((comps::ITEM, i));
                    }
                }
                Target::Tagged(pattern) => {
                    let tags: Vec<u32> = cb.names().iter().enumerate().filter(|(_, n)| comps::matches(pattern, n)).map(|(i, _)| i as u32).collect();
                    if tags.is_empty() {
                        errors.push(format!("{}: component {} set on tag #{pattern}, which no tag matches", m.1, s.value.name));
                    }
                    for kind in [comps::BLOCK, comps::ITEM] {
                        hits.extend(cb.entries_with_any(kind, &tags).into_iter().map(|e| (kind, e)));
                    }
                }
                Target::Entity(n) => hits.extend(entities.names.get(n).map(|&k| (comps::ENTITY, k))),
                Target::Biome(n) => hits.extend(biome_names.get(n).map(|&b| (comps::BIOME, b))),
                Target::Dimension(n) => hits.extend(dim_names.get(n).map(|&d| (comps::DIMENSION, d))),
            }
            if hits.is_empty() && !matches!(s.target, Target::Tagged(_)) {
                errors.push(format!("{}: component {} set on unknown {:?}", m.1, s.value.name, s.target));
            }
            for (kind, e) in hits {
                let label = || match kind {
                    comps::BLOCK => states.get(e as usize).map_or_else(String::new, |st| block_defs[st.block as usize].name.clone()),
                    comps::ITEM => item_defs[e as usize].name.clone(),
                    comps::ENTITY => entities.kinds[e as usize].name.clone(),
                    comps::BIOME => biome_defs[e as usize].name.clone(),
                    _ => dim_defs[e as usize].name.clone(),
                };
                if s.remove {
                    cb.remove(kind, e, &s.value.name);
                } else {
                    cb.put(kind, e, &s.value, Some(m), &label);
                }
            }
        }
    }
    errors.append(&mut cb.errors);
    let components = cb.finish();
    let id = |n: &str| components.id(n);
    let ids = Ids {
        max_stack: id(abi::comp::MAX_STACK),
        durability: id(abi::comp::DURABILITY),
        attack_damage: id(abi::comp::ATTACK_DAMAGE),
        mining_speed: id(abi::comp::MINING_SPEED),
        tool_level: id(abi::comp::TOOL_LEVEL),
        harvest_level: id(abi::comp::HARVEST_LEVEL),
        needs_tool: id(abi::comp::NEEDS_TOOL),
        damage: id(abi::comp::DAMAGE),
        name: id(abi::comp::NAME),
        conductor: id(abi::comp::CONDUCTOR),
        worn: id(abi::comp::WORN),
        worn_texture: id(abi::comp::WORN_TEXTURE),
        wear_per_hit: id(abi::comp::WEAR_PER_HIT),
    };
    for (i, s) in states.iter_mut().enumerate() {
        s.conductor = ids.conductor.is_some_and(|c| components.has(comps::BLOCK, i as u32, c));
        if let Some((face, _)) = s.support {
            let b = s.block as usize;
            let (tag, near) = &block_values[b].3[i - block_defs[b].first_state as usize];
            s.support = Some((face, (!tag.is_empty()).then(|| components.id(tag).unwrap_or(u32::MAX))));
            s.support_near = (!near.is_empty()).then(|| components.id(near).unwrap_or(u32::MAX));
        }
    }
    for (i, d) in item_defs.iter_mut().enumerate() {
        let i = i as u32;
        d.max_stack = components.get::<u32>(comps::ITEM, i, ids.max_stack.unwrap_or(u32::MAX)).unwrap_or(64).max(1);
        d.durability = components.get::<u32>(comps::ITEM, i, ids.durability.unwrap_or(u32::MAX)).unwrap_or(0);
        d.attack = components.get::<f32>(comps::ITEM, i, ids.attack_damage.unwrap_or(u32::MAX)).unwrap_or(1.0);
    }
    let tool_classes: Vec<(u32, Option<u32>)> = components
        .names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.strip_prefix("mineable/").map(|class| (i as u32, components.id(&format!("tools/{class}")))))
        .collect();

    let mut dim_defs = dim_defs;
    // Ores, scatters and decorations go into the dimensions they name, or every one but bare ones.
    let not_bare: Vec<usize> = (0..dim_defs.len()).filter(|&d| !dim_defs[d].bare).collect();
    let targets = |what: &str, names: &[String], errors: &mut Vec<String>| -> Vec<usize> {
        if names.is_empty() {
            return not_bare.clone();
        }
        names
            .iter()
            .filter_map(|n| {
                let d = dim_names.get(n).map(|&d| d as usize);
                if d.is_none() {
                    errors.push(format!("{what}: unknown dimension {n}"));
                }
                d
            })
            .collect()
    };
    let mut raw_ores: Vec<crate::registry::RawOre> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.ores)).collect();
    raw_ores.sort_by(|a, b| a.block.cmp(&b.block));
    let mut ore_defs: Vec<(Vec<usize>, OreDef)> = Vec::new();
    for o in raw_ores {
        let Some(&b) = block_names.get(&o.block) else {
            errors.push(format!("ore {}: unknown block", o.block));
            continue;
        };
        let dims = targets(&format!("ore {}", o.block), &o.dimensions, &mut errors);
        let mut replace: Vec<u32> = Vec::new();
        for r in &o.replace {
            match crate::refs::Ref::parse(r).and_then(|r| crate::refs::blocks(&r, &block_names, &block_defs, &components, states.len() as u32)) {
                Ok(b) => replace.extend(b),
                Err(e) => errors.push(format!("ore {}: replace: {e}", o.block)),
            }
        }
        replace.sort_unstable();
        replace.dedup();
        let salt = o.block.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3));
        let biome = match o.biome.as_str() {
            "" => None,
            t => match crate::refs::tag(&components, t) {
                Ok(t) => Some(t),
                Err(e) => {
                    errors.push(format!("ore {}: biome: {e}", o.block));
                    continue;
                }
            },
        };
        ore_defs.push((dims, OreDef {
            state: block_defs[b as usize].first_state,
            replace,
            min_y: o.min_y,
            max_y: o.max_y,
            veins: o.veins,
            size: o.size,
            salt,
            biome,
        }));
    }
    for (dims, o) in ore_defs {
        for d in dims {
            dim_defs[d].ores.push(o.clone());
        }
    }
    let mut raw_scatters: Vec<crate::registry::RawScatter> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.scatters)).collect();
    raw_scatters.sort_by(|a, b| a.name.cmp(&b.name));
    for s in raw_scatters {
        let what = format!("scatter {}", s.name);
        let density = (!s.density.is_empty()).then(|| components.id(&s.density)).flatten();
        if density.is_none() {
            errors.push(format!("{what}: density component {:?} is not set on any biome", s.density));
        }
        if s.cell < 1 || s.reach < 0 {
            errors.push(format!("{what}: cell {} and reach {} (cell at least 1)", s.cell, s.reach));
            continue;
        }
        for d in targets(&what, &s.dimensions, &mut errors) {
            dim_defs[d].scatters.push(ScatterDef { owner: s.owner, name: s.name.clone(), cell: s.cell, reach: s.reach, density, salt: crate::registry::name_hash(&s.name), shape: s.shape });
        }
    }
    let mut raw_decorations: Vec<(ModId, Vec<String>, Callback)> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.decorations)).collect();
    raw_decorations.sort_by_key(|d| mods[d.0.0 as usize].order);
    for (owner, dims, cb) in raw_decorations {
        for d in targets(&format!("decoration of {}", mods[owner.0 as usize].id), &dims, &mut errors) {
            dim_defs[d].decorations.push((owner, cb));
        }
    }
    let mut raw_structures: Vec<crate::registry::RawStructure> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.structures)).collect();
    raw_structures.sort_by(|a, b| a.name.cmp(&b.name));
    for s in raw_structures {
        let Some(&d) = dim_names.get(&s.dimension) else {
            errors.push(format!("structure {}: unknown dimension {}", s.name, s.dimension));
            continue;
        };
        let mut blocks = Vec::with_capacity(s.blocks.len());
        for (p, name) in &s.blocks {
            // `base:furnace[facing=east]`: that state, other properties at their defaults.
            let state = split_state(name).and_then(|(block, props)| block_defs[*block_names.get(block).ok_or_else(|| format!("unknown block {block}"))? as usize].state_of(&props));
            match state {
                Ok(st) => blocks.push((glam::IVec3::from_array(*p), st)),
                Err(e) => errors.push(format!("structure {}: {e}", s.name)),
            }
        }
        let biome = match s.biome.as_str() {
            "" => None,
            t => match crate::refs::tag(&components, t) {
                Ok(t) => Some(t),
                Err(e) => {
                    errors.push(format!("structure {}: biome: {e}", s.name));
                    continue;
                }
            },
        };
        let extent = blocks.iter().fold(glam::IVec3::ZERO, |m, (p, _)| m.max(*p + glam::IVec3::ONE));
        let salt = s.name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3));
        dim_defs[d as usize].structures.push(StructureDef { name: s.name, blocks, extent, spacing: s.spacing.max(1) as i32, chance: s.chance, biome, sink: s.sink, salt });
    }
    // Terrains: the owner's description first, then other mods' edits in load order.
    let mut raw_terrain: Vec<crate::server::terrain::RawTerrain> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.terrain)).collect();
    raw_terrain.retain(|t| {
        let known = dim_names.contains_key(&t.dimension);
        if !known {
            log::warn!("{}: terrain edit for {}, which no mod adds", mods[t.owner.0 as usize].id, t.dimension);
        }
        known
    });
    raw_terrain.sort_by_key(|t| {
        let d = &dim_defs[dim_names[&t.dimension] as usize];
        (t.dimension.clone(), t.owner != d.owner, mods[t.owner.0 as usize].order)
    });
    let mut by_dim: Vec<Vec<crate::server::terrain::RawTerrain>> = (0..dim_defs.len()).map(|_| Vec::new()).collect();
    for t in raw_terrain {
        by_dim[dim_names[&t.dimension] as usize].push(t);
    }
    for (d, ops) in by_dim.into_iter().enumerate() {
        if ops.is_empty() {
            continue;
        }
        let def = &dim_defs[d];
        if ops[0].owner != def.owner {
            log::warn!("{}: edits the terrain of {}, which has none (its mod generates it in code), edits skipped", mods[ops[0].owner.0 as usize].id, def.name);
            continue;
        }
        let names = crate::server::terrain::Names {
            block: &|n| block_names.get(n).map(|&b| block_defs[b as usize].first_state),
            biome: &|n| biome_names.get(n).copied(),
            biome_rule: &|n| biome_names.get(n).map(|&b| &biome_rules[b as usize]),
        };
        let t = crate::server::terrain::build(&def.name, &|m| mods[m.0 as usize].id.clone(), ops, &names, &mut errors);
        dim_defs[d].terrain = Some(Box::new(t));
    }
    let mut commands: Vec<crate::commands::CommandDef> = Vec::new();
    for (name, usage, _) in crate::server::BUILTIN_COMMANDS {
        let alts = crate::commands::parse_usage(usage).expect("engine command usage");
        commands.push(crate::commands::CommandDef::new(name, usage, alts, None, false));
    }
    for (name, usage, _) in crate::app::CLIENT_COMMANDS {
        let alts = crate::commands::parse_usage(usage).expect("engine command usage");
        commands.push(crate::commands::CommandDef::new(name, usage, alts, None, true));
    }
    // Word sets for `word(#set)` parameters: the engine's, then what mods add in load order.
    let mut word_sets: FxHashMap<String, Vec<String>> = FxHashMap::default();
    word_sets.insert("core:gizmos".into(), crate::client::gizmo::BUILTIN.iter().map(|g| g.to_string()).collect());
    let mut added: Vec<(u32, (String, Vec<String>))> = games.iter_mut().zip(0u32..).flat_map(|(g, i)| std::mem::take(&mut g.words).into_iter().map(move |w| (i, w))).collect();
    added.sort_by_key(|w| w.0);
    for (_, (set, words)) in added {
        let list = word_sets.entry(set).or_default();
        list.extend(words.into_iter().filter(|w| !list.contains(w)).collect::<Vec<_>>());
    }
    let mut mod_commands: Vec<(ModId, String, String, Callback, bool)> = games
        .iter()
        .flat_map(|g| g.commands.iter().map(|c| (c.0, c.1.clone(), c.2.clone(), c.3, false)).chain(g.client_commands.iter().map(|c| (c.0, c.1.clone(), c.2.clone(), c.3, true))))
        .collect();
    mod_commands.sort_by(|a, b| a.1.cmp(&b.1));
    for (owner, name, usage, cb, client) in mod_commands {
        if commands.iter().any(|d| d.name == name) {
            errors.push(format!("{}: command /{name} is already taken", mod_name(owner)));
            continue;
        }
        match crate::commands::parse_usage(&usage) {
            Ok(alts) => commands.push(crate::commands::CommandDef::new(&name, &usage, alts, Some((owner, cb)), client)),
            Err(e) => errors.push(format!("{}: command /{name}: {e}", mod_name(owner))),
        }
    }
    // Other mods extend existing commands, in load order: more completion words, more branches.
    let mut extensions: Vec<(ModId, crate::registry::CommandExtension)> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.command_extensions)).collect();
    extensions.sort_by_key(|e| mods.get(e.0.0 as usize).map_or(u32::MAX, |m| m.order));
    for (owner, ext) in extensions {
        let who = mod_name(owner);
        match ext {
            crate::registry::CommandExtension::Branch { command, usage, cb } => match (commands.iter_mut().find(|d| d.name == command), crate::commands::parse_usage(&usage)) {
                (Some(d), Ok(alts)) => d.add_branch(alts, (owner, cb)),
                (None, _) => errors.push(format!("{who}: extends unknown command /{command}")),
                (_, Err(e)) => errors.push(format!("{who}: branch of /{command}: {e}")),
            },
        }
    }
    let (liquids, reactions) = freeze_liquids(&mut games, &mut states, &block_defs, &block_names, &components, &mut errors);
    let (mut item_liquid, mut containers) = (FxHashMap::default(), FxHashMap::default());
    if let (Some(lc), Some(rc)) = (components.id(abi::comp::LIQUID), components.id(abi::comp::REMAINDER)) {
        for i in 1..item_defs.len() as u32 {
            let Some(liquid) = components.text(comps::ITEM, i, lc) else { continue };
            let Some(l) = liquids.iter().position(|d| d.name == liquid) else {
                errors.push(format!("item {} holds unknown liquid {liquid}", item_defs[i as usize].name));
                continue;
            };
            let empty = match components.text(comps::ITEM, i, rc) {
                Some(n) => item_names.get(n).copied().unwrap_or_else(|| {
                    // The container would vanish when poured.
                    errors.push(format!("item {}: unknown remainder {n}", item_defs[i as usize].name));
                    0
                }),
                None => 0,
            };
            item_liquid.insert(i, (l as u16, empty));
            if empty != 0 {
                containers.insert((empty, l as u16), i);
            }
        }
    }
    let creative_tabs = creative_tabs(&mods, &games, &item_defs, &item_names, &components, &mut errors);
    let mut ordered_items: Vec<u32> = (1..item_defs.len() as u32).collect();
    ordered_items.sort_by_key(|&i| item_defs[i as usize].order);
    let tag_items = crate::refs::tag_items(&components, &ordered_items);
    let mut defaults: FxHashMap<String, crate::loot::Table> = FxHashMap::default();
    for b in &block_defs[1..] {
        if let Some(item) = b.drop {
            defaults.insert(b.name.clone(), crate::loot::single(item, b.drop_count.max(1)));
        }
    }
    let raw_loot: Vec<crate::loot::RawLoot> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.loot)).collect();
    let raw_mods: Vec<crate::loot::RawLootMod> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.loot_mods)).collect();
    let resolve = crate::loot::Resolve { items: &item_names, tag_items: &tag_items, components: &components };
    let order_of = |m: ModId| mods.get(m.0 as usize).map_or(u32::MAX, |m| m.order);
    let loot = crate::loot::freeze(raw_loot, raw_mods, &order_of, &resolve, defaults, &mut errors);
    let mut stack_flags: FxHashMap<u32, u32> = FxHashMap::default();
    for g in &games {
        for (name, _, flags) in &g.stack_components {
            if let Some(id) = components.id(name) {
                *stack_flags.entry(id).or_default() |= flags;
            }
        }
    }
    let mut game = crate::registry::freeze(games, entities, &block_names, &item_names, &item_defs, &dim_names, &components, &tag_items, &order_of, &mut errors);
    // A structure's box is what its blocks fill; a set size would only disagree with them.
    if let Some(comp) = components.id(abi::comp::BLOCKS) {
        for (k, kind) in game.entities.iter_mut().enumerate() {
            if let Some((w, h)) = components.text(comps::ENTITY, k as u32, comp).and_then(crate::server::structure::extent) {
                (kind.width, kind.height, kind.eye) = (w, h, h * crate::registry::EYE);
            }
        }
    }
    // Items worn in an equipment slot carry its `worn/<slot>` tag.
    for (e, slot) in game.player_layout.equipment.iter().enumerate() {
        if let Some(t) = components.id(&format!("worn/{}", slot.name)) {
            for (i, d) in item_defs.iter_mut().enumerate() {
                if components.has(comps::ITEM, i as u32, t) {
                    d.equip = Some(e as u32);
                }
            }
        }
    }
    // Blocks open their menus; one with recipe kinds makes the block a station of them.
    for (b, menu) in opens_names.iter().enumerate().filter(|m| !m.1.is_empty()) {
        let def = &mut block_defs[b + 1];
        match game.menu_names.get(menu) {
            Some(&m) => {
                def.opens = Some(m);
                if let Some(item) = item_defs.iter().position(|i| i.block == Some(def.first_state)) {
                    for kind in &game.menus[m as usize].recipes {
                        game.recipe_stations.entry(kind.clone()).or_default().push(item as u32);
                    }
                }
            }
            None => errors.push(format!("{}: block {} opens unknown menu {menu}", mods.get(def.owner.0 as usize).map_or("engine", |m| m.id.as_str()), def.name)),
        }
    }
    word_sets.entry("core:rules".into()).or_default().extend(game.rules.iter().map(|r| r.0.clone()));
    for (table, t) in &loot {
        for k in t.pools.iter().flat_map(|p| &p.entries).filter_map(|e| e.smelt.as_ref()) {
            if !game.recipe_kinds.contains(k) {
                errors.push(format!("loot table {table} smelts by recipe kind {k}, which no mod registers"));
            }
        }
    }
    // Sound events named by `sound` prefixes, resolved once.
    if let Some(sc) = components.id(abi::comp::SOUND) {
        let resolve = |prefix: Option<&str>, parts: [&str; 4]| -> [u32; 4] {
            parts.map(|p| prefix.and_then(|pre| game.sound_names.get(&format!("{pre}.{p}")).copied()).unwrap_or(u32::MAX))
        };
        // A prefix with none of its events registered is a typo (`.sounds("stnoe")`).
        let mut unknown: Vec<(String, String)> = Vec::new();
        let mut check = |sounds: [u32; 4], prefix: Option<&str>, what: &str| {
            if let Some(p) = prefix.filter(|_| sounds.iter().all(|&s| s == u32::MAX)) {
                if !unknown.iter().any(|u| u.0 == p) {
                    unknown.push((p.to_string(), what.to_string()));
                }
            }
            sounds
        };
        for (i, s) in states.iter_mut().enumerate() {
            let prefix = components.text(comps::BLOCK, i as u32, sc);
            s.sounds = check(resolve(prefix, ["break", "place", "step", "hit"]), prefix, &block_defs[s.block as usize].name);
        }
        let kinds: Vec<[u32; 4]> = (0..game.entities.len() as u32)
            .map(|k| {
                let prefix = components.text(comps::ENTITY, k, sc);
                check(resolve(prefix, ["hurt", "death", "ambient", "step"]), prefix, &game.entities[k as usize].name)
            })
            .collect();
        for (k, s) in game.entities.iter_mut().zip(kinds) {
            k.sounds = s;
        }
        for (prefix, what) in unknown {
            errors.push(format!("{what}: no sound events named {prefix}.*"));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Content {
        mods,
        states,
        blocks: block_defs,
        items: item_defs,
        biomes: biome_defs,
        word_sets,
        tint_kinds,
        dims: dim_defs,
        components,
        ids,
        tool_classes,
        tag_items,
        block_textures,
        crack_textures,
        fire_texture,
        commands,
        item_textures,
        block_names,
        item_names,
        creative_tabs,
        dim_names,
        spawn_dim,
        void_dim,
        biome_names,
        game,
        liquids,
        reactions,
        item_liquid,
        containers,
        loot,
        stack_flags,
    })
}

fn freeze_liquids(
    games: &mut [crate::registry::RawGame],
    states: &mut [StateInfo],
    blocks: &[BlockDef],
    block_names: &FxHashMap<String, u32>,
    components: &Components,
    errors: &mut Vec<String>,
) -> (Vec<LiquidDef>, Vec<ReactionDef>) {
    let mut raw: Vec<abi::LiquidDesc> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for g in games.iter_mut() {
        for (name, d) in std::mem::take(&mut g.liquids) {
            names.push(name);
            raw.push(d);
        }
    }
    let mut order: Vec<usize> = (0..raw.len()).collect();
    order.sort_by(|&a, &b| names[a].cmp(&names[b]));
    let mut liquids = Vec::new();
    for i in order {
        let (name, d) = (&names[i], &raw[i]);
        let Some(&b) = block_names.get(name) else {
            errors.push(format!("liquid {name} has no block of that name"));
            continue;
        };
        let def = &blocks[b as usize];
        if def.states != 16 {
            errors.push(format!("liquid {name}: its block needs 16 states, has {}", def.states));
            continue;
        }
        let id = liquids.len() as u16;
        for s in 0..16 {
            states[(def.first_state + s) as usize].liquid = Some(id);
        }
        liquids.push(LiquidDef {
            name: name.clone(),
            first_state: def.first_state,
            delay: d.delay.max(1),
            drop_off: d.drop_off.clamp(1, 7),
            slope_find: d.slope_find.min(8),
            renewable: d.renewable,
            rises: d.density < 0,
            temperature: d.temperature,
        });
    }
    let matcher = |s: &str, errors: &mut Vec<String>| -> Option<Match> {
        match crate::refs::Ref::parse(s).and_then(|r| crate::refs::blocks(&r, block_names, blocks, components, blocks.iter().map(|b| b.states).sum())) {
            Ok(b) => Some(Match(b)),
            Err(e) => {
                errors.push(format!("liquid reaction: {e}"));
                None
            }
        }
    };
    let result = |s: &str, errors: &mut Vec<String>| -> Option<u32> {
        if s.is_empty() {
            return None;
        }
        let b = block_names.get(s).map(|&b| blocks[b as usize].first_state);
        if b.is_none() {
            errors.push(format!("liquid reaction makes unknown block {s}"));
        }
        b
    };
    let mut reactions = Vec::new();
    for g in games.iter_mut() {
        for r in std::mem::take(&mut g.reactions) {
            let (Some(liquid), Some(touching)) = (matcher(&r[0], errors), matcher(&r[1], errors)) else { continue };
            reactions.push(ReactionDef { liquid, touching, source: result(&r[2], errors), flowing: result(&r[3], errors), falls_into: result(&r[4], errors) });
        }
    }
    (liquids, reactions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, deps: &[&str]) -> ModMeta {
        ModMeta { id: id.into(), version: "1".into(), deps: deps.iter().map(|d| d.to_string()).collect(), order: 0 }
    }

    #[test]
    fn dependencies_load_first() {
        // Sorted by id, as the loader lists them: "aaa" depends on "zzz".
        let mods = [meta("aaa", &["zzz"]), meta("base", &[]), meta("zzz", &["base"])];
        assert_eq!(load_order(&mods), vec![2, 0, 1]);
    }
}
