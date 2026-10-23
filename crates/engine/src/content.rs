//! Frozen registries. Built once from every mod's registrar, then shared read-only.

use std::sync::OnceLock;

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use voxide_abi::{self as abi, Callback, Layer, Model, ModId};

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
    /// Block is tagged `core:conductor` (part of energy networks).
    pub conductor: bool,
}

impl StateInfo {
    #[inline]
    pub fn is(&self, flag: u32) -> bool {
        self.flags & flag != 0
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
    pub tool: String,
    pub harvest_level: u32,
    pub tags: Vec<String>,
    pub props: Vec<(String, Vec<u8>)>,
    /// Resolved after items are frozen. `None` = drops nothing.
    pub drop: Option<u32>,
    pub drop_count: u32,
    pub cb: BlockCallbacks,
}

pub struct ItemDef {
    pub name: String,
    pub owner: ModId,
    pub max_stack: u32,
    pub durability: u32,
    /// Item icon layer; `u32::MAX` when the icon is the 3D block.
    pub icon: u32,
    pub block: Option<u32>,
    pub tool: String,
    pub tool_level: u32,
    pub tool_speed: f32,
    pub attack: f32,
    pub tags: Vec<String>,
    pub props: Vec<(String, Vec<u8>)>,
    pub on_use: Option<Callback>,
    /// Owning mod's load position, then registration order: how creative tabs list items.
    pub order: u64,
}

/// A creative inventory tab: translated as `tab.<ns>.<name>`, or the mod's name for the tab a mod
/// gets automatically for items it put in no tab.
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
}

pub struct Content {
    pub mods: Vec<ModMeta>,
    pub states: Vec<StateInfo>,
    pub blocks: Vec<BlockDef>,
    pub items: Vec<ItemDef>,
    pub biomes: Vec<BiomeDef>,
    pub dims: Vec<DimensionDef>,
    /// Block texture names, index = array layer.
    pub block_textures: Vec<String>,
    /// Block breaking stages, indices into `block_textures`.
    pub crack_textures: [u32; 10],
    /// Engine commands first, then mods' in name order.
    pub commands: Vec<crate::commands::CommandDef>,
    /// Item icon names, index = array layer.
    pub item_textures: Vec<String>,
    pub block_names: FxHashMap<String, u32>,
    pub item_names: FxHashMap<String, u32>,
    pub dim_names: FxHashMap<String, u32>,
    pub creative_tabs: Vec<CreativeTab>,
    pub game: crate::registry::Game,
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

    pub fn item_has_tag(&self, item: u32, tag: &str) -> bool {
        self.items.get(item as usize).is_some_and(|d| d.tags.iter().any(|t| t == tag))
            || self.items.get(item as usize).and_then(|d| d.block).is_some_and(|s| self.block_of(s).tags.iter().any(|t| t == tag))
    }

    /// Worn look of an item: covered part roles, inflation and texture.
    pub fn worn(&self, item: u32) -> Option<(abi::Worn, &str)> {
        let b = self.item_prop(item, abi::WORN)?;
        let n = size_of::<abi::Worn>();
        if b.len() <= n {
            return None;
        }
        let w = abi::Worn { roles: u32::from_le_bytes(b[0..4].try_into().ok()?), inflate: f32::from_le_bytes(b[4..8].try_into().ok()?) };
        Some((w, std::str::from_utf8(&b[n..]).ok()?))
    }

    pub fn item_prop(&self, item: u32, key: &str) -> Option<&[u8]> {
        let d = self.items.get(item as usize)?;
        d.props.iter().find(|p| p.0 == key).map(|p| p.1.as_slice()).or_else(|| {
            let b = self.block_of(d.block?);
            b.props.iter().find(|p| p.0 == key).map(|p| p.1.as_slice())
        })
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
    pub biomes: Vec<(BiomeDef, usize)>,
    pub dims: Vec<(DimensionDef, usize)>,
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
}

pub struct RawBlock {
    pub name: String,
    pub cell: usize,
    pub states: Vec<RawState>,
    pub tool: String,
    pub harvest_level: u32,
    pub tags: Vec<String>,
    pub props: Vec<(String, Vec<u8>)>,
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
    pub max_stack: u32,
    pub durability: u32,
    pub texture: String,
    pub block: String,
    pub tool: String,
    pub tool_level: u32,
    pub tool_speed: f32,
    pub attack: f32,
    pub tags: Vec<String>,
    pub props: Vec<(String, Vec<u8>)>,
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

unsafe fn strs(s: abi::Slice<abi::Str>) -> Vec<String> {
    unsafe { s.as_slice().iter().map(|x| x.as_str().to_string()).collect() }
}

unsafe fn props(s: abi::Slice<abi::Prop>) -> Vec<(String, Vec<u8>)> {
    unsafe { s.as_slice().iter().map(|p| (p.key.as_str().to_string(), p.value.as_slice().to_vec())).collect() }
}

pub(crate) unsafe fn copy_block(d: &abi::BlockDesc) -> RawBlock {
    unsafe {
        let states = d
            .states
            .as_slice()
            .iter()
            .map(|s| RawState {
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
                boxes: s
                    .boxes
                    .as_slice()
                    .iter()
                    .map(|b| (b.min, b.max, std::array::from_fn(|i| b.textures[i].as_str().to_string())))
                    .collect(),
                collision: s.collision.as_slice().to_vec(),
                hardness: s.hardness,
                friction: s.friction,
                drag: s.drag,
            })
            .collect();
        RawBlock {
            name: d.name.as_str().to_string(),
            cell: d.cell as usize,
            states,
            tool: d.tool.as_str().to_string(),
            harvest_level: d.harvest_level,
            tags: strs(d.tags),
            props: props(d.props),
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
            max_stack: d.max_stack.max(1),
            durability: d.durability,
            texture: d.texture.as_str().to_string(),
            block: d.block.as_str().to_string(),
            tool: d.tool.as_str().to_string(),
            tool_level: d.tool_level,
            tool_speed: d.tool_speed,
            attack: d.attack,
            tags: strs(d.tags),
            props: props(d.props),
            on_use: cb(d.on_use),
        }
    }
}

pub(crate) unsafe fn copy_dimension(owner: ModId, d: &abi::DimensionDesc) -> (DimensionDef, usize) {
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
        )
    }
}

fn write_cell(cell: usize, id: u32) {
    if cell != 0 {
        unsafe { (*(cell as *const std::sync::atomic::AtomicU32)).store(id, std::sync::atomic::Ordering::Relaxed) }
    }
}

/// Registered tabs in mod order, filled with their tagged items in registration order; every mod
/// with leftover items gets one more tab of its own.
fn creative_tabs(mods: &[ModMeta], games: &[crate::registry::RawGame], items: &[ItemDef], item_names: &FxHashMap<String, u32>, errors: &mut Vec<String>) -> Vec<CreativeTab> {
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
    let mut ordered: Vec<u32> = (1..items.len() as u32).collect();
    ordered.sort_by_key(|&i| items[i as usize].order);
    let mut leftover: Vec<(usize, Vec<u32>)> = Vec::new();
    for i in ordered {
        let def = &items[i as usize];
        let tab = def.tags.iter().find_map(|t| t.strip_prefix("tab/")).and_then(|t| tabs.iter().position(|x| x.name == t));
        match tab {
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
pub fn freeze(mods: Vec<ModMeta>, regs: Vec<RawRegistrar>) -> Result<Content, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    let mut blocks: Vec<(ModId, RawBlock)> = Vec::new();
    let mut items: Vec<(ModId, RawItem)> = Vec::new();
    let mut biomes: Vec<(BiomeDef, usize)> = Vec::new();
    let mut dims: Vec<(DimensionDef, usize)> = Vec::new();
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
    }];
    let mut block_defs = vec![BlockDef {
        name: "core:air".into(),
        owner: ModId(u32::MAX),
        first_state: 0,
        states: 1,
        tool: String::new(),
        harvest_level: 0,
        tags: Vec::new(),
        props: Vec::new(),
        drop: None,
        drop_count: 0,
        cb: BlockCallbacks::default(),
    }];
    let mut block_drops = vec![String::new()];
    for (owner, b) in blocks {
        let index = block_defs.len() as u32;
        let first = states.len() as u32;
        write_cell(b.cell, first);
        for &cell in &b.extra_cells {
            write_cell(cell, first);
        }
        for s in &b.states {
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
                conductor: b.tags.iter().any(|t| t == "core:conductor"),
            });
        }
        let drop = if b.drop.is_empty() { b.name.clone() } else { b.drop.clone() };
        block_drops.push(drop);
        block_defs.push(BlockDef {
            name: b.name,
            owner,
            first_state: first,
            states: b.states.len() as u32,
            tool: b.tool,
            harvest_level: b.harvest_level,
            tags: b.tags,
            props: b.props,
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
        icon: 0,
        block: None,
        tool: String::new(),
        tool_level: 0,
        tool_speed: 1.0,
        attack: 1.0,
        tags: Vec::new(),
        props: Vec::new(),
        on_use: None,
        order: 0,
    }];
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
        item_defs.push(ItemDef {
            name: it.name,
            owner,
            max_stack: it.max_stack,
            durability: it.durability,
            icon,
            block,
            tool: it.tool,
            tool_level: it.tool_level,
            tool_speed: it.tool_speed,
            attack: it.attack,
            tags: it.tags,
            props: it.props,
            on_use: it.on_use,
            order: (owner.0 as u64) << 32 | it.seq as u64,
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

    let biome_defs: Vec<BiomeDef> = biomes
        .into_iter()
        .enumerate()
        .map(|(i, (b, cell))| {
            write_cell(cell, i as u32);
            b
        })
        .collect();
    let dim_defs: Vec<DimensionDef> = dims
        .into_iter()
        .enumerate()
        .map(|(i, (d, cell))| {
            write_cell(cell, i as u32);
            d
        })
        .collect();
    let dim_names: FxHashMap<String, u32> = dim_defs.iter().enumerate().map(|(i, d)| (d.name.clone(), i as u32)).collect();
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
                Some(tag) => replace.extend(block_defs.iter().enumerate().filter(|(_, b)| b.tags.iter().any(|t| t == tag)).map(|(i, _)| i as u32)),
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
        commands.push(crate::commands::CommandDef { name: name.to_string(), usage: usage.to_string(), alts, handler: None });
    }
    let mut mod_commands: Vec<(ModId, String, String, Callback)> = games.iter().flat_map(|g| g.commands.iter().cloned()).collect();
    mod_commands.sort_by(|a, b| a.1.cmp(&b.1));
    for (owner, name, usage, cb) in mod_commands {
        if commands.iter().any(|d| d.name == name) {
            errors.push(format!("{}: command /{name} is already taken", mod_name(owner)));
            continue;
        }
        match crate::commands::parse_usage(&usage) {
            Ok(alts) => commands.push(crate::commands::CommandDef { name, usage, alts, handler: Some((owner, cb)) }),
            Err(e) => errors.push(format!("{}: command /{name}: {e}", mod_name(owner))),
        }
    }
    let creative_tabs = creative_tabs(&mods, &games, &item_defs, &item_names, &mut errors);
    let game = crate::registry::freeze(games, &block_names, &item_names, &mut errors);
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
        block_textures,
        crack_textures,
        commands,
        item_textures,
        block_names,
        item_names,
        creative_tabs,
        dim_names,
        game,
    })
}
