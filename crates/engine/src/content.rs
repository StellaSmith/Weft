//! Frozen registries. Built once from every mod's registrar, then shared read-only.

use std::sync::OnceLock;

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use voxide_abi::{self as abi, Callback, Layer, Model, ModId};

use crate::components::{self as comps, Components, RawSet, RawValue, Target};

pub const AIR: u32 = 0;
pub const NO_ITEM: u32 = 0;

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
    /// Layers in the block texture array.
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
    /// Face whose neighbour must hold it up, and the tag that neighbour needs (`None`: solid).
    pub support: Option<(u8, Option<u32>)>,
}

impl StateInfo {
    #[inline]
    pub fn is(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

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
}

pub struct BlockDef {
    pub name: String,
    pub owner: ModId,
    pub first_state: u32,
    pub states: u32,
    /// Resolved after items are frozen. `None` = drops nothing.
    pub drop: Option<u32>,
    pub drop_count: u32,
    pub cb: BlockCallbacks,
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
    pub infinite: u32,
    /// Lighter than air: flows up.
    pub rises: bool,
    pub temperature: u32,
}

/// A block or any block with a tag.
#[derive(Clone, Copy, Debug)]
pub enum Match {
    Block(u32),
    Tag(u32),
}

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
    pub grass: u32,
    pub foliage: u32,
    pub water: u32,
    pub temperature: f32,
    pub rainfall: f32,
}

pub struct DimensionDef {
    pub name: String,
    pub owner: ModId,
    pub generator: Option<Callback>,
    pub spawn: Option<Callback>,
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
    /// Placed by the engine after `generator`, in name order.
    pub ores: Vec<OreDef>,
}

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
}

pub struct ModMeta {
    pub id: String,
    pub name: String,
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
    pub conductor: Option<u32>,
    pub energy: Option<u32>,
    pub worn: Option<u32>,
    pub worn_texture: Option<u32>,
    /// Item tags armour slots accept: head, chest, legs, feet.
    pub armor: [Option<u32>; 4],
    /// `needs_stone_tool`, `needs_iron_tool`, `needs_diamond_tool`.
    pub needs_tier: [Option<u32>; 3],
}

pub struct Content {
    pub mods: Vec<ModMeta>,
    pub states: Vec<StateInfo>,
    pub blocks: Vec<BlockDef>,
    pub items: Vec<ItemDef>,
    pub biomes: Vec<BiomeDef>,
    pub dims: Vec<DimensionDef>,
    pub components: Components,
    pub ids: Ids,
    /// (`mineable/<class>`, `tools/<class>`) component ids per tool class.
    pub tool_classes: Vec<(u32, Option<u32>)>,
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

impl Content {
    #[inline]
    pub fn state(&self, id: u32) -> &StateInfo {
        &self.states[id as usize]
    }

    pub fn block_of(&self, state: u32) -> &BlockDef {
        &self.blocks[self.states[state as usize].block as usize]
    }

    pub fn find_state(&self, name: &str) -> Option<u32> {
        self.block_names.get(name).map(|&b| self.blocks[b as usize].first_state)
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

    /// By name, for commands and other rare uses; hot paths keep the id.
    pub fn block_has_tag(&self, state: u32, tag: &str) -> bool {
        self.block_has(state, self.components.id(tag))
    }

    pub fn item_has_tag(&self, item: u32, tag: &str) -> bool {
        self.item_has(item, self.components.id(tag))
    }

    pub fn item_value<T: Copy>(&self, item: u32, comp: Option<u32>) -> Option<T> {
        self.components.get(comps::ITEM, item, comp?)
    }

    pub fn block_value<T: Copy>(&self, state: u32, comp: Option<u32>) -> Option<T> {
        self.components.get(comps::BLOCK, state, comp?)
    }

    /// Whether `neighbour`, on the face the state rests on, holds it up.
    pub fn supports(&self, state: u32, neighbour: u32) -> bool {
        match &self.states[state as usize].support {
            None => true,
            Some((_, Some(tag))) => self.block_has(neighbour, Some(*tag)),
            Some((_, None)) => self.states[neighbour as usize].flags & abi::block_flags::SOLID != 0,
        }
    }

    /// Worn look of an item: covered part roles, inflation and texture.
    pub fn worn(&self, item: u32) -> Option<(abi::Worn, &str)> {
        let w = self.item_value::<abi::Worn>(item, self.ids.worn)?;
        Some((w, self.components.text(comps::ITEM, item, self.ids.worn_texture?)?))
    }

    pub fn matches(&self, m: Match, state: u32) -> bool {
        match m {
            Match::Block(b) => self.states[state as usize].block == b,
            Match::Tag(t) => self.components.has(comps::BLOCK, state, t),
        }
    }

    /// Tool level a block needs: its `harvest_level`, or modern vanilla's `needs_stone_tool` (1),
    /// `needs_iron_tool` (2), `needs_diamond_tool` (3) tags, whichever is higher.
    pub fn harvest_level(&self, state: u32) -> u32 {
        let by_tag = self.ids.needs_tier.iter().enumerate().filter(|(_, t)| self.block_has(state, **t)).map(|(i, _)| i as u32 + 1).max().unwrap_or(0);
        self.block_value::<u32>(state, self.ids.harvest_level).unwrap_or(0).max(by_tag)
    }

    /// Drops nothing without a fitting tool: `needs_tool`, or any `needs_<tier>_tool` tag.
    pub fn needs_tool(&self, state: u32) -> bool {
        self.block_has(state, self.ids.needs_tool) || self.ids.needs_tier.iter().any(|t| self.block_has(state, *t))
    }

    pub fn max_stack(&self, item: u32) -> u32 {
        self.items.get(item as usize).map_or(64, |d| d.max_stack)
    }

    pub fn mod_name(&self, id: ModId) -> &str {
        self.mods.get(id.0 as usize).map_or("engine", |m| m.id.as_str())
    }

    /// Full name of a state for saves and the network: `"base:furnace#3"`.
    pub fn state_name(&self, state: u32) -> String {
        let b = self.block_of(state);
        let i = state - b.first_state;
        if i == 0 { b.name.clone() } else { format!("{}#{i}", b.name) }
    }

    pub fn state_by_name(&self, name: &str) -> Option<u32> {
        let (block, idx) = match name.rsplit_once('#') {
            Some((b, i)) => (b, i.parse::<u32>().ok()?),
            None => (name, 0),
        };
        let b = &self.blocks[*self.block_names.get(block)? as usize];
        (idx < b.states).then_some(b.first_state + idx)
    }
}

// ---------------------------------------------------------------- registration side

/// Owned copy of everything one mod registered. Filled without locks, one per mod thread.
pub struct RawRegistrar {
    pub owner: ModId,
    pub blocks: Vec<RawBlock>,
    pub items: Vec<RawItem>,
    pub biomes: Vec<(BiomeDef, usize, Vec<RawValue>)>,
    pub dims: Vec<(DimensionDef, usize, Vec<RawValue>)>,
    pub game: crate::registry::RawGame,
    pub errors: Vec<String>,
}

pub struct RawState {
    pub flags: u32,
    pub light: [u8; 3],
    pub filter: [u8; 3],
    pub opacity: u8,
    pub tint: u8,
    pub tint_faces: u8,
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
    pub components: Vec<RawValue>,
}

pub struct RawBlock {
    pub name: String,
    pub cell: usize,
    pub states: Vec<RawState>,
    pub components: Vec<RawValue>,
    pub drop: String,
    pub drop_count: u32,
    pub cb: BlockCallbacks,
    pub replace: bool,
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
            dims: Vec::new(),
            game: Default::default(),
            errors: Vec::new(),
        }
    }
}

fn cb(c: Callback) -> Option<Callback> {
    c.is_some().then_some(c)
}

pub(crate) unsafe fn copy_block(d: &abi::BlockDesc) -> RawBlock {
    unsafe {
        let mut states = Vec::new();
        abi::each_sized(d.states, |s| {
            let mut boxes = Vec::new();
            abi::each_sized(s.boxes, |b| boxes.push((b.min, b.max, std::array::from_fn(|i| b.textures[i].as_str().to_string()))));
            states.push(RawState {
                flags: s.flags,
                light: s.light,
                filter: s.filter,
                opacity: s.opacity,
                tint: s.tint,
                tint_faces: s.tint_faces,
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
                components: RawValue::list(s.components),
            })
        });
        RawBlock {
            name: d.name.as_str().to_string(),
            cell: d.cell as usize,
            states,
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
            },
            replace: d.replace != 0,
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
                ores: Vec::new(),
            },
            d.cell as usize,
            RawValue::list(d.components),
        )
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
    let mut biomes: Vec<(BiomeDef, usize, Vec<RawValue>)> = Vec::new();
    let mut dims: Vec<(DimensionDef, usize, Vec<RawValue>)> = Vec::new();
    let mut games = Vec::new();
    for r in regs {
        games.push(r.game);
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
                b.extra_cells.push(old.cell);
                b.extra_cells.extend(old.extra_cells);
                kept.push((owner, b));
            }
            _ => kept.push((owner, b)),
        }
    }
    let blocks = kept;
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
    if biomes.len() > 256 {
        errors.push(format!("{} biomes registered, cubes store at most 256", biomes.len()));
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
    }];
    let mut block_defs = vec![BlockDef {
        name: "core:air".into(),
        owner: ModId(u32::MAX),
        first_state: 0,
        states: 1,
        drop: None,
        drop_count: 0,
        cb: BlockCallbacks::default(),
    }];
    let mut block_drops = vec![String::new()];
    // Per block: (block components, per-state components, support tags) for the component pass.
    let mut block_values: Vec<(ModId, Vec<RawValue>, Vec<Vec<RawValue>>, Vec<String>)> = vec![(ModId(u32::MAX), Vec::new(), vec![Vec::new()], vec![String::new()])];
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
            states.push(StateInfo {
                block: index,
                flags: s.flags,
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
            });
            supports.push(s.support);
            state_values.push(s.components);
        }
        let drop = if b.drop.is_empty() { b.name.clone() } else { b.drop.clone() };
        block_drops.push(drop);
        block_values.push((owner, b.components, state_values, supports));
        block_defs.push(BlockDef {
            name: b.name,
            owner,
            first_state: first,
            states: (states.len() as u32) - first,
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
    let biome_defs: Vec<BiomeDef> = biomes
        .into_iter()
        .enumerate()
        .map(|(i, (b, cell, v))| {
            write_cell(cell, i as u32);
            biome_values.push(v);
            b
        })
        .collect();
    let biome_names: FxHashMap<String, u32> = biome_defs.iter().enumerate().map(|(i, b)| (b.name.clone(), i as u32)).collect();
    let mut dim_values = Vec::new();
    let dim_defs: Vec<DimensionDef> = dims
        .into_iter()
        .enumerate()
        .map(|(i, (d, cell, v))| {
            write_cell(cell, i as u32);
            dim_values.push((d.owner, v));
            d
        })
        .collect();
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
        for t in supports.iter().filter(|t| !t.is_empty()) {
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
        for rc in &g.components {
            cb.declare(&rc.def.name, &rc.schema, Some(rc.def.size as usize), &|| who(rc.owner, "entity component"), &mut errors);
        }
    }
    for t in crate::registry::ENGINE_TAGS {
        cb.declare(t, "", None, &|| "the engine".into(), &mut errors);
    }
    cb.declare(crate::registry::MODIFIERS, "bytes", None, &|| "the engine".into(), &mut errors);
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
                    let (base, idx) = match n.rsplit_once('#') {
                        Some((b, i)) => (b, i.parse::<u32>().ok()),
                        None => (n.as_str(), None),
                    };
                    if let Some(&b) = block_names.get(base) {
                        let d = &block_defs[b as usize];
                        match idx {
                            Some(i) if i < d.states => hits.push((comps::BLOCK, d.first_state + i)),
                            Some(_) => {}
                            None => hits.extend((0..d.states).map(|i| (comps::BLOCK, d.first_state + i))),
                        }
                    }
                    if let (Some(&i), None) = (item_names.get(base), idx) {
                        hits.push((comps::ITEM, i));
                    }
                }
                Target::Tagged(pattern) => {
                    let tags: Vec<u32> = cb.names().iter().enumerate().filter(|(_, n)| comps::matches(pattern, n)).map(|(i, _)| i as u32).collect();
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
        conductor: id(abi::comp::CONDUCTOR),
        energy: id(abi::comp::ENERGY),
        worn: id(abi::comp::WORN),
        worn_texture: id(abi::comp::WORN_TEXTURE),
        armor: ["armor_head", "armor_chest", "armor_legs", "armor_feet"].map(id),
        needs_tier: ["needs_stone_tool", "needs_iron_tool", "needs_diamond_tool"].map(id),
    };
    for (i, s) in states.iter_mut().enumerate() {
        s.conductor = ids.conductor.is_some_and(|c| components.has(comps::BLOCK, i as u32, c));
        if let Some((face, _)) = s.support {
            let b = s.block as usize;
            let tag = &block_values[b].3[i - block_defs[b].first_state as usize];
            s.support = Some((face, (!tag.is_empty()).then(|| components.id(tag).unwrap_or(u32::MAX))));
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
    let mut raw_ores: Vec<crate::registry::RawOre> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.ores)).collect();
    raw_ores.sort_by(|a, b| (&a.dimension, &a.block).cmp(&(&b.dimension, &b.block)));
    for o in raw_ores {
        let (Some(&d), Some(&b)) = (dim_names.get(&o.dimension), block_names.get(&o.block)) else {
            errors.push(format!("ore {} in {}: unknown block or dimension", o.block, o.dimension));
            continue;
        };
        let mut replace: Vec<u32> = Vec::new();
        for r in &o.replace {
            match r.strip_prefix('#') {
                Some(tag) => {
                    if let Some(t) = components.id(tag) {
                        replace.extend((0..states.len() as u32).filter(|&s| components.has(comps::BLOCK, s, t)).map(|s| states[s as usize].block));
                    }
                }
                None => match block_names.get(r) {
                    Some(&b) => replace.push(b),
                    None => errors.push(format!("ore {} in {}: unknown block {r} to replace", o.block, o.dimension)),
                },
            }
        }
        replace.sort_unstable();
        replace.dedup();
        let salt = o.block.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3));
        dim_defs[d as usize].ores.push(OreDef {
            state: block_defs[b as usize].first_state,
            replace,
            min_y: o.min_y,
            max_y: o.max_y,
            veins: o.veins,
            size: o.size,
            salt,
        });
    }
    let mut commands: Vec<crate::commands::CommandDef> = Vec::new();
    for (name, usage) in crate::commands::BUILTIN {
        let alts = crate::commands::parse_usage(usage).expect("engine command usage");
        commands.push(crate::commands::CommandDef { name: name.to_string(), usage: usage.to_string(), alts, handler: None, client: false });
    }
    let gizmos: Vec<&str> = crate::client::gizmo::BUILTIN.iter().map(|g| g.0).collect();
    for (name, usage) in crate::commands::CLIENT {
        let usage = usage.replace("{gizmos}", &gizmos.join(","));
        let alts = crate::commands::parse_usage(&usage).expect("engine command usage");
        commands.push(crate::commands::CommandDef { name: name.to_string(), usage, alts, handler: None, client: true });
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
            Ok(alts) => commands.push(crate::commands::CommandDef { name, usage, alts, handler: Some((owner, cb)), client }),
            Err(e) => errors.push(format!("{}: command /{name}: {e}", mod_name(owner))),
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
            let empty = components.text(comps::ITEM, i, rc).and_then(|n| item_names.get(n).copied()).unwrap_or(0);
            item_liquid.insert(i, (l as u16, empty));
            if empty != 0 {
                containers.insert((empty, l as u16), i);
            }
        }
    }
    let creative_tabs = creative_tabs(&mods, &games, &item_defs, &item_names, &components, &mut errors);
    let mut ordered_items: Vec<u32> = (1..item_defs.len() as u32).collect();
    ordered_items.sort_by_key(|&i| item_defs[i as usize].order);
    let first_tagged = |t: &str| components.id(t).and_then(|t| ordered_items.iter().copied().find(|&i| components.has(comps::ITEM, i, t)));
    let mut defaults: FxHashMap<String, crate::loot::Table> = FxHashMap::default();
    for b in &block_defs[1..] {
        if let Some(item) = b.drop {
            defaults.insert(b.name.clone(), crate::loot::single(item, b.drop_count.max(1)));
        }
    }
    let raw_loot: Vec<crate::loot::RawLoot> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.loot)).collect();
    let raw_mods: Vec<crate::loot::RawLootMod> = games.iter_mut().flat_map(|g| std::mem::take(&mut g.loot_mods)).collect();
    let resolve = crate::loot::Resolve { items: &item_names, by_tag: &first_tagged, components: &components };
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
    let game = crate::registry::freeze(games, entities, &block_names, &item_names, &item_defs, &components, &order_of, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Content {
        mods,
        states,
        blocks: block_defs,
        items: item_defs,
        biomes: biome_defs,
        dims: dim_defs,
        components,
        ids,
        tool_classes,
        block_textures,
        crack_textures,
        fire_texture,
        commands,
        item_textures,
        block_names,
        item_names,
        creative_tabs,
        dim_names,
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
            infinite: d.infinite,
            rises: d.density < 0,
            temperature: d.temperature,
        });
    }
    let matcher = |s: &str, errors: &mut Vec<String>| -> Option<Match> {
        if let Some(t) = s.strip_prefix('#') {
            // A tag nothing has matches nothing.
            return Some(Match::Tag(components.id(t).unwrap_or(u32::MAX)));
        }
        let m = block_names.get(s).map(|&b| Match::Block(b));
        if m.is_none() {
            errors.push(format!("liquid reaction names unknown block {s}"));
        }
        m
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
        ModMeta { id: id.into(), name: id.into(), version: "1".into(), deps: deps.iter().map(|d| d.to_string()).collect(), order: 0 }
    }

    #[test]
    fn dependencies_load_first() {
        // Sorted by id, as the loader lists them: "aaa" depends on "zzz".
        let mods = [meta("aaa", &["zzz"]), meta("base", &[]), meta("zzz", &["base"])];
        assert_eq!(load_order(&mods), vec![2, 0, 1]);
    }
}
