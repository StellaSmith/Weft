//! Gameplay registries: entity kinds, components, menus, recipes, HUD parts. Frozen together with
//! blocks and items, ids again assigned by sorted name.

use std::sync::atomic::{AtomicU32, Ordering};

use rustc_hash::FxHashMap;
use weft_abi::{self as abi, Callback, ModId, entity_flags as ef, part_role};

use crate::components::{self as comps, Components, RawSet, RawValue};

pub const ENGINE: ModId = ModId(u32::MAX);
pub const PLAYER: &str = "core:player";

#[derive(Clone)]
pub struct HudElement {
    pub name: String,
    /// Translation key of its F3 panel title: the name with `.` for `:`.
    pub title: String,
    pub anchor: String,
    pub order: i32,
    pub owner: ModId,
    pub cb: Callback,
}
pub const ITEM: &str = "core:item";
/// Holder of virtual inventories (`Host::inventory_create`).
pub const VIRTUAL: &str = "core:virtual";
/// A block falling as an entity (`Host::fall_block`), placed again where it lands.
pub const FALLING: &str = "core:falling_block";

#[derive(Clone, Debug)]
pub struct Cuboid {
    /// Model pixels, relative to the part's pivot.
    pub origin: [f32; 3],
    pub size: [f32; 3],
    pub uv: [f32; 2],
    pub inflate: f32,
    pub mirror: bool,
    /// Own texel rectangle per face (-X, +X, -Y, +Y, -Z, +Z), instead of the box unwrap.
    pub faces: Option<[[f32; 4]; 6]>,
}

impl Cuboid {
    pub fn boxed(origin: [f32; 3], size: [f32; 3], uv: [f32; 2]) -> Cuboid {
        Cuboid { origin, size, uv, inflate: 0.0, mirror: false, faces: None }
    }

    /// Texel rectangles of the faces (-X, +X, -Y, +Y, -Z, +Z): its own, or the classic box
    /// unwrap (mirrored: each face flipped and the side faces swapped).
    pub fn face_rects(&self) -> [[f32; 4]; 6] {
        if let Some(f) = self.faces {
            return f;
        }
        let [w, h, d] = self.size;
        let [u, v] = self.uv;
        let mut r = [
            [u, v + d, d, h],
            [u + d + w, v + d, d, h],
            [u + d + w, v, w, d],
            [u + d, v, w, d],
            [u + d, v + d, w, h],
            [u + d * 2.0 + w, v + d, w, h],
        ];
        if self.mirror {
            r.swap(0, 1);
            for f in &mut r {
                f[0] += f[2];
                f[2] = -f[2];
            }
        }
        r
    }
}

#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    /// Index of the part it hangs from.
    pub parent: Option<u32>,
    pub pivot: [f32; 3],
    pub role: u32,
    pub cuboids: Vec<Cuboid>,
    pub rest: [f32; 3],
    /// Free-form triangles: model pixels (absolute, not pivot relative) and texels.
    pub mesh: Vec<weft_abi::MeshVertex>,
}

pub struct EntityKind {
    pub name: String,
    pub owner: ModId,
    pub width: f32,
    pub height: f32,
    pub eye: f32,
    pub speed: f32,
    pub flags: u32,
    pub category: u32,
    /// Set by the mod instead of `category`; resolved at freeze.
    pub category_name: String,
    pub texture: String,
    pub texture_size: [u32; 2],
    pub parts: Vec<Part>,
    /// (owner, callback) lists: the kind's own callback first, then hooks from other mods.
    pub on_tick: Vec<(ModId, Callback)>,
    pub on_hurt: Vec<(ModId, Callback)>,
    pub on_death: Vec<(ModId, Callback)>,
    pub on_interact: Vec<(ModId, Callback)>,
    /// Components attached at spawn.
    pub components: Vec<u32>,
    /// Starting values of attached components for this kind, where they differ from the
    /// component's default.
    pub values: Vec<(u32, Box<[u8]>)>,
    /// Size of the inventory created with it (block entities), 0 = none.
    pub slots: u32,
    /// Sound events hurt, death, ambient, step (`u32::MAX` none), from its `sound` component.
    pub sounds: [u32; 4],
    /// Solid bodies: riders up to this high above it move with it.
    pub carry_height: f32,
}

impl EntityKind {
    pub fn is(&self, f: u32) -> bool {
        self.flags & f != 0
    }
}

/// Per-entity storage of a component (indexed by component id; `runtime` false for components
/// that only exist on content entries).
#[derive(Clone, Default)]
pub struct ComponentDef {
    pub name: String,
    pub size: u32,
    pub flags: u32,
    pub default: Vec<u8>,
    pub runtime: bool,
}

pub struct MenuDef {
    pub name: String,
    pub owner: ModId,
    /// Slots of the menu's own inventory (its `slot_owner::MENU` groups).
    pub temp_slots: u32,
    pub groups: Vec<GroupDef>,
    /// The machine entity kind whose inventory `slot_owner::BLOCK` groups show.
    pub entity: String,
    /// Recipe kinds its `arrow` bars open.
    pub recipes: Vec<String>,
    pub on_change: Option<Callback>,
    pub on_take: Option<Callback>,
    pub on_tick: Option<Callback>,
    pub screen: Option<Callback>,
}

/// A slot group (`abi::SlotGroupDesc`), its start resolved.
#[derive(Clone, Debug)]
pub struct GroupDef {
    pub name: String,
    /// `abi::slot_owner`.
    pub owner: u32,
    pub start: u32,
    pub cols: u32,
    pub rows: u32,
    pub flags: u32,
    /// Tag or component name, empty for any item.
    pub accepts: String,
    pub limit: u32,
    pub size: f32,
    pub background: Option<String>,
    pub tooltip: Option<String>,
    pub priority: i32,
}

impl GroupDef {
    /// (owner inventory start, slot count, columns); the player's presets follow `layout`.
    pub fn span(&self, layout: &PlayerLayout) -> (u32, u32, u32) {
        use weft_abi::slot_owner as so;
        match self.owner {
            so::PLAYER_MAIN => (layout.hotbar, layout.slots - layout.hotbar - layout.worn.len() as u32, layout.hotbar.max(1)),
            so::PLAYER_HOTBAR => (0, layout.hotbar, layout.hotbar.max(1)),
            _ => (self.start, self.cols * self.rows, self.cols.max(1)),
        }
    }

    /// Owner inventory index of the group's `i`-th slot (the main inventory skips worn slots).
    pub fn index(&self, layout: &PlayerLayout, i: u32) -> u32 {
        let (start, _, _) = self.span(layout);
        if self.owner == weft_abi::slot_owner::PLAYER_MAIN {
            (start..layout.slots).filter(|s| !layout.worn.contains(s)).nth(i as usize).unwrap_or(u32::MAX)
        } else {
            start + i
        }
    }
}

impl MenuDef {
    /// (group, its first menu slot, slot count) in menu slot order.
    pub fn slot_groups(&self, layout: &PlayerLayout) -> impl Iterator<Item = (&GroupDef, u32, u32)> + '_ {
        let mut at = 0;
        let layout = layout.clone();
        self.groups.iter().map(move |g| {
            let n = g.span(&layout).1;
            at += n;
            (g, at - n, n)
        })
    }

    pub fn group(&self, layout: &PlayerLayout, name: &str) -> Option<(&GroupDef, u32, u32)> {
        self.slot_groups(layout).find(|g| g.0.name == name)
    }

    /// The group of menu slot `slot` and the slot's place in it.
    pub fn group_of(&self, layout: &PlayerLayout, slot: u32) -> Option<(&GroupDef, u32)> {
        self.slot_groups(layout).find(|g| slot >= g.1 && slot < g.1 + g.2).map(|g| (g.0, slot - g.1))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ingredient {
    Empty,
    Item(u32),
    /// Component id of the tag; `u32::MAX` when nothing carries it.
    Tag(u32),
    /// A liquid by name, in millibuckets.
    Fluid(String),
}

impl Ingredient {
    #[inline]
    pub fn matches(&self, item: u32, c: &Components) -> bool {
        match self {
            Ingredient::Empty => item == 0,
            Ingredient::Item(i) => *i == item,
            Ingredient::Tag(t) => item != 0 && c.has(comps::ITEM, item, *t),
            Ingredient::Fluid(_) => false,
        }
    }
}

pub struct Recipe {
    /// Shaped: width, height, cells (row major).
    pub shaped: Option<(u32, u32, Vec<Ingredient>)>,
    pub inputs: Vec<Ingredient>,
    /// How many of each input (millibuckets for fluids).
    pub input_counts: Vec<u32>,
    /// The main result first.
    pub outputs: Vec<RecipeOutput>,
    pub time: u32,
    /// (component, value) the recipe carries.
    pub values: Vec<(u32, Vec<u8>)>,
    /// Not shown in the recipe viewer.
    pub hidden: bool,
}

#[derive(Clone, Debug)]
pub struct RecipeOutput {
    pub what: Ingredient,
    pub count: u32,
    pub chance: f32,
}

impl Recipe {
    /// The main result as an item stack (empty when it is a fluid).
    pub fn result(&self) -> weft_abi::ItemStack {
        match self.outputs.first() {
            Some(RecipeOutput { what: Ingredient::Item(i), count, .. }) => weft_abi::ItemStack::new(*i, *count),
            _ => weft_abi::ItemStack::EMPTY,
        }
    }

    pub fn value(&self, comp: u32) -> Option<&[u8]> {
        self.values.iter().find(|v| v.0 == comp).map(|v| v.1.as_slice())
    }
}

/// How the recipe viewer shows a kind of recipe (`abi::RecipeKindDesc`).
pub struct RecipeKindDef {
    pub owner: ModId,
    pub input_grid: [u32; 2],
    pub output_grid: [u32; 2],
    pub info: Option<String>,
    pub info_values: Vec<String>,
    pub display: Option<Callback>,
}

pub struct RawRecipeKind {
    pub owner: ModId,
    pub name: String,
    pub def: RecipeKindDef,
}

#[derive(Default)]
pub struct Game {
    pub entities: Vec<EntityKind>,
    pub entity_names: FxHashMap<String, u32>,
    pub components: Vec<ComponentDef>,
    pub component_names: FxHashMap<String, u32>,
    pub menus: Vec<MenuDef>,
    pub menu_names: FxHashMap<String, u32>,
    pub recipes: FxHashMap<String, Vec<Recipe>>,
    pub recipe_kinds: rustc_hash::FxHashSet<String>,
    pub recipe_kind_defs: FxHashMap<String, RecipeKindDef>,
    /// Entity categories by `EntityKind::category`: the engine's three first.
    pub categories: Vec<CategoryDef>,
    /// Sound categories by `SoundDef::category`: the engine's six first.
    pub sound_categories: Vec<SoundCategory>,
    pub huds: Vec<HudElement>,
    pub player_menu: Option<u32>,
    pub player_layout: PlayerLayout,
    /// Mods' screens; of two with one name the mod loaded last comes last.
    pub screens: Vec<ScreenDef>,
    pub screen_parts: Vec<ScreenPart>,
    /// Block index -> (entity kind, inventory slots).
    pub block_entities: FxHashMap<u32, u32>,
    pub player_kind: u32,
    pub item_kind: u32,
    pub virtual_kind: u32,
    pub falling_kind: u32,
    pub energy: u32,
    /// (owner, hook, callback) in mod order, see `abi::client_hook`.
    pub client_hooks: Vec<(ModId, u32, Callback)>,
    /// Mod message channels by id (sorted by name, so both sides agree).
    pub packets: Vec<PacketDef>,
    pub packet_names: FxHashMap<String, u32>,
    /// Mod key bindings: (action, default key).
    pub keys: Vec<(String, String)>,
    /// World rules (name, default), sorted by name; values live in the server.
    pub rules: Vec<(String, i64)>,
    pub events: Vec<EventDef>,
    pub event_names: FxHashMap<String, u32>,
    pub interceptors: FxHashMap<String, Vec<(ModId, Callback)>>,
    /// Steps per chain in run order: (stage, owner, modifier).
    pub modifiers: FxHashMap<String, Vec<(u32, ModId, Callback)>>,
    pub attributes: Vec<AttrDef>,
    pub attr_names: FxHashMap<String, u32>,
    /// The engine component holding entities' attribute modifiers.
    pub modifiers_comp: u32,
    /// Entity components with `SYNC_WATCHERS` / `SYNC_MENU`.
    pub watched_comps: Vec<u32>,
    pub menu_comps: Vec<u32>,
    /// Per component: synced to anyone at all (its changes are looked for every tick).
    pub synced_comps: Vec<bool>,
    /// Index of `core:mob_spawning` in `rules`.
    pub mob_spawning_rule: usize,
    /// (owner, component, system) in mod order.
    pub systems: Vec<(ModId, u32, Callback)>,
    /// `{if:name}` conditions mods answer.
    pub text_conditions: FxHashMap<String, (ModId, Callback)>,
    /// Particle kinds by id (sorted by name).
    pub particles: Vec<ParticleDef>,
    pub particle_names: FxHashMap<String, u32>,
    /// Particle texture names, by particle texture index.
    pub particle_textures: Vec<String>,
    /// Items shown with a recipe type in the recipe viewer (the machines making them).
    pub recipe_stations: FxHashMap<String, Vec<u32>>,
    /// Sound events by id (sorted by name).
    pub sounds: Vec<SoundDef>,
    pub sound_names: FxHashMap<String, u32>,
    /// Background music, in mod load order.
    pub music: Vec<MusicDef>,
    /// Natural spawning rules, in mod load order.
    pub spawns: Vec<SpawnRule>,
    /// Settings by id (sorted by name), see `config.rs`.
    pub configs: Vec<crate::config::ConfigDef>,
    pub config_names: FxHashMap<String, u32>,
}

pub struct SoundDef {
    pub name: String,
    pub files: Vec<String>,
    pub category: u32,
    /// Set by mods naming their own category; resolved at freeze.
    pub category_name: String,
    pub volume: f32,
    pub pitch: [f32; 2],
    pub range: f32,
}

pub struct MusicDef {
    pub name: String,
    pub tracks: Vec<String>,
    pub dimension: Option<u32>,
    pub when: u32,
}

pub struct ParticleDef {
    pub name: String,
    /// Frames over its life (indices into `particle_textures`); empty for block dust.
    pub frames: Vec<u32>,
    pub life: [f32; 2],
    pub scale: [f32; 2],
    pub gravity: f32,
    pub drag: f32,
    pub color: u32,
    pub color_end: u32,
    pub flags: u32,
    pub shader: String,
}

pub struct RawParticle {
    pub name: String,
    pub cell: usize,
    pub textures: Vec<String>,
    /// Numbers only: its `Str`s and `cell` are copied out (they point into the mod).
    pub desc: abi::ParticleDesc,
    pub shader: String,
}

// The descriptor's pointers are cleared when it is copied.
unsafe impl Send for RawParticle {}

#[derive(Default)]
pub struct PacketDef {
    pub server: Option<(ModId, Callback)>,
    pub client: Option<(ModId, Callback)>,
}

// ---------------------------------------------------------------- raw registration

pub struct RawEntity {
    pub name: String,
    pub cell: usize,
    pub desc: EntityKind,
    pub replace: bool,
    pub components: Vec<RawValue>,
}

pub struct RawComponent {
    pub owner: ModId,
    pub def: ComponentDef,
    pub schema: String,
}

#[derive(Default)]
pub struct RawRecipe {
    pub kind: String,
    pub pattern: Vec<String>,
    pub keys: Vec<(char, String)>,
    pub inputs: Vec<String>,
    pub input_counts: Vec<u32>,
    pub output: String,
    pub count: u32,
    pub time: u32,
    /// (item or `fluid:..`, count, chance) after the main output.
    pub outputs: Vec<(String, u32, f32)>,
    pub values: Vec<crate::components::RawValue>,
    pub flags: u32,
}

#[derive(Default)]
pub struct RawGame {
    pub entities: Vec<RawEntity>,
    pub components: Vec<RawComponent>,
    /// (owner, kind, hook, callback); hook 0 is the tick.
    pub hooks: Vec<(ModId, String, u32, Callback)>,
    pub recipes: Vec<RawRecipe>,
    pub recipe_kinds: Vec<RawRecipeKind>,
    pub categories: Vec<CategoryDef>,
    pub sound_categories: Vec<SoundCategory>,
    pub menus: Vec<(usize, MenuDef)>,
    pub huds: Vec<HudElement>,
    /// (owner, name, usage, callback).
    pub commands: Vec<(ModId, String, String, Callback)>,
    /// Commands that run on the client: (owner, name, usage, callback).
    pub client_commands: Vec<(ModId, String, String, Callback)>,
    pub command_extensions: Vec<(ModId, CommandExtension)>,
    /// (mod, menu): of several, the mod loaded last wins.
    pub player_menu: Option<(ModId, String)>,
    /// (mod, layout): likewise.
    pub player_layout: Option<(ModId, PlayerLayout)>,
    pub screens: Vec<ScreenDef>,
    pub screen_parts: Vec<ScreenPart>,
    /// (set, words) for `word(#set)` command parameters.
    pub words: Vec<(String, Vec<String>)>,
    pub block_entities: Vec<(String, String, u32)>,
    /// (kind, component, its starting value on that kind).
    pub attach: Vec<(String, String, Option<Vec<u8>>)>,
    pub ores: Vec<RawOre>,
    pub terrain: Vec<crate::server::terrain::RawTerrain>,
    pub structures: Vec<RawStructure>,
    /// (name, icon item) in registration order.
    pub creative_tabs: Vec<(String, String)>,
    /// Components and tags set on other mods' entries.
    pub sets: Vec<RawSet>,
    pub client_hooks: Vec<(ModId, u32, Callback)>,
    /// (owner, name, side, callback).
    pub packets: Vec<(ModId, String, u32, Callback)>,
    pub keys: Vec<(String, String)>,
    /// (block name, flow rules).
    pub liquids: Vec<(String, weft_abi::LiquidDesc)>,
    /// World rules: (name, default).
    pub rules: Vec<(String, i64)>,
    /// liquid, touching, source result, flowing result, falls-into result.
    pub reactions: Vec<[String; 5]>,
    /// (owner, event, schema, handler).
    pub event_handlers: Vec<(ModId, String, String, Callback)>,
    pub interceptors: Vec<(ModId, String, Callback)>,
    /// (owner, chain, stage, modifier).
    pub modifiers: Vec<(ModId, String, u32, Callback)>,
    /// (name, default, min, max).
    pub attributes: Vec<(String, f64, f64, f64)>,
    pub loot: Vec<crate::loot::RawLoot>,
    pub loot_mods: Vec<crate::loot::RawLootMod>,
    /// (component, schema, `stack_flags`).
    pub stack_components: Vec<(String, String, u32)>,
    /// (owner, component, schema, system).
    pub systems: Vec<(ModId, String, String, Callback)>,
    pub text_conditions: Vec<(ModId, String, Callback)>,
    pub particles: Vec<RawParticle>,
    /// (handle cell, setting).
    pub configs: Vec<(usize, crate::config::ConfigDef)>,
    /// (recipe type, item).
    pub recipe_stations: Vec<(String, String)>,
    /// (handle cell, event).
    pub sounds: Vec<(usize, SoundDef)>,
    /// (dimension name, music).
    pub music: Vec<(String, MusicDef)>,
    pub spawns: Vec<RawSpawn>,
}

/// A spawning rule as registered (names).
pub struct RawSpawn {
    pub entity: String,
    pub biome: String,
    pub ground: String,
    pub dimension: String,
    pub light: [u32; 2],
    pub y: [i32; 2],
    pub group: [u32; 2],
    pub weight: u32,
    pub chance: f32,
}

/// A spawning rule with its names resolved (see `abi::SpawnDesc`).
pub struct SpawnRule {
    pub entity: u32,
    pub category: u32,
    pub biome: Option<u32>,
    pub ground: Option<u32>,
    pub dimension: Option<u32>,
    pub light: [u32; 2],
    pub y: [i32; 2],
    pub group: [u32; 2],
    pub weight: u32,
    pub chance: f32,
}

/// A mod's addition to a command registered elsewhere.
pub enum CommandExtension {
    Branch { command: String, usage: String, cb: Callback },
}

/// An event mods can subscribe to; `size` is learned from the first value emitted when the
/// engine does not know it.
pub struct EventDef {
    pub name: String,
    pub schema: String,
    pub size: std::sync::atomic::AtomicU32,
    pub handlers: Vec<(ModId, Callback)>,
}

/// A mod's screen.
/// Something a mod adds to a place of a screen.
#[derive(Clone)]
pub struct ScreenPart {
    pub screen: String,
    pub area: String,
    pub order: i32,
    pub owner: ModId,
    pub cb: Callback,
}

#[derive(Clone)]
pub struct ScreenDef {
    pub name: String,
    pub owner: ModId,
    pub cb: Callback,
}

/// Slots of every player's inventory.
#[derive(Clone, Debug)]
pub struct PlayerLayout {
    pub slots: u32,
    /// The first this many are the hotbar.
    pub hotbar: u32,
    /// Equipment: never filled by pickups, worn on the model.
    pub worn: std::ops::Range<u32>,
}

impl Default for PlayerLayout {
    fn default() -> Self {
        PlayerLayout { slots: 36, hotbar: 9, worn: 36..36 }
    }
}

impl PlayerLayout {
    /// Main inventory slots: neither hotbar nor equipment.
    pub fn main(&self) -> impl Iterator<Item = u32> + '_ {
        (self.hotbar..self.slots).filter(|i| !self.worn.contains(i))
    }
}

/// A sound category: its volume setting and whether caves and water change it.
#[derive(Clone, Debug)]
pub struct SoundCategory {
    pub name: String,
    /// `<ns>:volume_<name>`, none for `core:ui`.
    pub setting: Option<String>,
    pub world: bool,
}

impl SoundCategory {
    pub fn new(name: &str, world: bool) -> SoundCategory {
        let (ns, short) = name.split_once(':').unwrap_or(("core", name));
        let setting = (name != "core:ui").then(|| format!("{ns}:volume_{short}"));
        SoundCategory { name: name.to_string(), setting, world }
    }
}

/// `SoundDef::category` of the engine's music track: the first category.
pub const MUSIC_CATEGORY: u32 = 0;

/// What natural spawning and despawning know of entities by category.
#[derive(Clone, Debug)]
pub struct CategoryDef {
    pub name: String,
    /// At most this many spawn naturally within 64 blocks of a player.
    pub cap: u32,
    /// Further than this from every player they vanish (checked every 5 s); 0 = never.
    pub despawn: f64,
}

pub struct AttrDef {
    pub name: String,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    /// Stable key in saved modifiers.
    pub hash: u64,
    /// The entity kind component of the same name holding a kind's own value.
    pub comp: Option<u32>,
}

/// Events the engine emits, with the schemas of their `abi` structs.
pub const ENGINE_EVENTS: &[(&str, &str, u32)] = &[
    (abi::event::BLOCK_BROKEN, "{player:u64,pos:[i32;3],state:u32,item:u32}", size_of::<abi::BlockChange>() as u32),
    (abi::event::BLOCK_PLACED, "{player:u64,pos:[i32;3],state:u32,item:u32}", size_of::<abi::BlockChange>() as u32),
    (abi::event::ENTITY_DIED, "{entity:u64,killer:u64,kind:u32,cause:u32,pos:[f64;3]}", size_of::<abi::Death>() as u32),
    (abi::event::PLAYER_JOINED, "{player:u64}", size_of::<abi::PlayerEvent>() as u32),
    (abi::event::ITEM_PICKED_UP, "{player:u64,stack:{item:u32,count:u32,data:u32}}", size_of::<abi::Pickup>() as u32),
    (abi::event::ENTITY_HURT, "{entity:u64,source:u64,amount:f32,cause:u32}", size_of::<abi::Hurt>() as u32),
];

/// FNV-1a, for names stored in saves.
pub fn name_hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3))
}

pub struct RawOre {
    pub dimension: String,
    pub block: String,
    pub replace: Vec<String>,
    pub min_y: i32,
    pub max_y: i32,
    pub veins: u32,
    pub size: u32,
    pub biome: String,
}

pub struct RawStructure {
    pub name: String,
    pub dimension: String,
    pub blocks: Vec<([i32; 3], String)>,
    pub spacing: u32,
    pub chance: f32,
    pub biome: String,
    pub sink: i32,
}

fn opt(c: Callback) -> Option<Callback> {
    c.is_some().then_some(c)
}

pub(crate) unsafe fn copy_entity(owner: ModId, d: &abi::EntityDesc) -> RawEntity {
    unsafe {
        let mut parts = Vec::new();
        d.parts.as_slice().iter().copied().for_each(|p| {
            let mut cuboids = Vec::new();
            p.cuboids.as_slice().iter().copied().for_each(|c| {
                cuboids.push(Cuboid { origin: c.origin, size: c.extent, uv: c.uv, inflate: c.inflate, mirror: c.mirror != 0, faces: (c.per_face != 0).then_some(c.faces) })
            });
            let n = parts.len() as i32;
            let parent = (p.parent >= 0 && p.parent < n).then_some(p.parent as u32);
            parts.push(Part { name: p.name.as_str().to_string(), parent, pivot: p.pivot, role: p.role, rest: p.rest, cuboids, mesh: p.mesh.as_slice().to_vec() });
        });
        let one = |c: Callback| opt(c).map(|c| vec![(owner, c)]).unwrap_or_default();
        RawEntity {
            name: d.name.as_str().to_string(),
            cell: d.cell as usize,
            replace: d.replace != 0,
            components: RawValue::list(d.components),
            desc: EntityKind {
                name: d.name.as_str().to_string(),
                owner,
                width: d.width,
                height: d.height,
                eye: d.eye,
                speed: d.speed,
                flags: d.flags,
                category: 0,
                category_name: d.category_name.as_str().to_string(),
                texture: d.texture.as_str().to_string(),
                texture_size: d.texture_size,
                parts,
                on_tick: one(d.on_tick),
                on_hurt: one(d.on_hurt),
                on_death: one(d.on_death),
                on_interact: one(d.on_interact),
                components: Vec::new(),
                values: Vec::new(),
                slots: 0,
                sounds: [u32::MAX; 4],
                carry_height: d.carry_height,
            },
        }
    }
}

pub(crate) unsafe fn copy_menu(owner: ModId, d: &abi::MenuDesc) -> (usize, MenuDef) {
    unsafe {
        // Groups without a start take the slots right after the previous one of their owner.
        let mut next = [0u32; 5];
        let text = |s: abi::Str| (s.len > 0).then(|| s.as_str().to_string());
        let groups: Vec<GroupDef> = d
            .groups
            .as_slice()
            .iter()
            .map(|g| {
                let o = (g.owner as usize).min(4);
                let start = if g.start == u32::MAX { next[o] } else { g.start };
                next[o] = next[o].max(start + g.cols * g.rows);
                GroupDef {
                    name: g.name.as_str().to_string(),
                    owner: g.owner,
                    start,
                    cols: g.cols.max(1),
                    rows: g.rows.max(1),
                    flags: g.flags,
                    accepts: g.accepts.as_str().to_string(),
                    limit: g.limit,
                    size: if g.size > 0.0 { g.size } else { 18.0 },
                    background: text(g.background),
                    tooltip: text(g.tooltip),
                    priority: g.priority,
                }
            })
            .collect();
        (
            d.cell as usize,
            MenuDef {
                name: d.name.as_str().to_string(),
                owner,
                temp_slots: groups.iter().filter(|g| g.owner == weft_abi::slot_owner::MENU).map(|g| g.start + g.cols * g.rows).max().unwrap_or(0),
                groups,
                entity: d.entity.as_str().to_string(),
                recipes: d.recipes.as_str().split_whitespace().map(str::to_string).collect(),
                on_change: opt(d.on_change),
                on_take: opt(d.on_take),
                on_tick: opt(d.on_tick),
                screen: opt(d.screen),
            },
        )
    }
}

pub(crate) unsafe fn copy_recipe(d: &abi::RecipeDesc) -> RawRecipe {
    unsafe {
        RawRecipe {
            kind: d.kind.as_str().to_string(),
            pattern: d.pattern.as_slice().iter().map(|s| s.as_str().to_string()).collect(),
            keys: d
                .keys
                .as_slice()
                .iter()
                .map(|k| (char::from_u32(k.ch).unwrap_or(' '), k.item.as_str().to_string()))
                .collect(),
            inputs: d.inputs.as_slice().iter().map(|s| s.as_str().to_string()).collect(),
            input_counts: d.input_counts.as_slice().to_vec(),
            output: d.output.as_str().to_string(),
            count: d.count.max(1),
            time: d.time,
            outputs: d.outputs.as_slice().iter().map(|o| (o.item.as_str().to_string(), o.count.max(1), o.chance)).collect(),
            values: crate::components::RawValue::list(d.values),
            flags: d.flags,
        }
    }
}

fn write_cell(cell: usize, id: u32) {
    if cell != 0 {
        unsafe { (*(cell as *const AtomicU32)).store(id, Ordering::Relaxed) }
    }
}

fn builtin(name: &str, width: f32, height: f32, flags: u32) -> EntityKind {
    EntityKind {
        name: name.into(),
        owner: ENGINE,
        width,
        height,
        eye: height * 0.9,
        speed: 4.3,
        flags,
        category: 0,
        category_name: String::new(),
        texture: String::new(),
        texture_size: [64, 32],
        parts: Vec::new(),
        on_tick: Vec::new(),
        on_hurt: Vec::new(),
        on_death: Vec::new(),
        on_interact: Vec::new(),
        components: Vec::new(),
        values: Vec::new(),
        slots: 0,
        sounds: [u32::MAX; 4],
        carry_height: 0.0,
    }
}

/// Classic player model so other players are visible without any mod providing one (a
/// `core:models/player.geo.json` in a resource pack replaces it).
fn player_parts() -> Vec<Part> {
    let part = |name: &str, pivot: [f32; 3], role: u32, o: [f32; 3], s: [f32; 3], uv: [f32; 2]| Part {
        name: name.into(),
        parent: None,
        pivot,
        role,
        rest: [0.0; 3],
        cuboids: vec![Cuboid::boxed(o, s, uv)],
        mesh: Vec::new(),
    };
    vec![
        part("head", [0.0, 24.0, 0.0], part_role::HEAD, [-4.0, 0.0, -4.0], [8.0, 8.0, 8.0], [0.0, 0.0]),
        part("body", [0.0, 24.0, 0.0], part_role::STATIC, [-4.0, -12.0, -2.0], [8.0, 12.0, 4.0], [16.0, 16.0]),
        part("rightArm", [-6.0, 22.0, 0.0], part_role::ARM_LEFT, [-2.0, -10.0, -2.0], [4.0, 12.0, 4.0], [40.0, 16.0]),
        part("leftArm", [6.0, 22.0, 0.0], part_role::ARM_RIGHT, [-2.0, -10.0, -2.0], [4.0, 12.0, 4.0], [40.0, 16.0]),
        part("rightLeg", [-2.0, 12.0, 0.0], part_role::LEG_LEFT, [-2.0, -12.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0]),
        part("leftLeg", [2.0, 12.0, 0.0], part_role::LEG_RIGHT, [-2.0, -12.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0]),
    ]
}

/// Tags the engine refers to by name, so they have ids even when no mod uses them.
pub const ENGINE_TAGS: &[&str] = &abi::comp::ARMOR;
/// Entity component with attribute modifiers.
pub const MODIFIERS: &str = "core:modifiers";

/// Entity kinds, frozen before components (which attach to them).
pub struct Entities {
    pub kinds: Vec<EntityKind>,
    pub names: FxHashMap<String, u32>,
    /// Each kind's own components and its owner.
    pub values: Vec<(ModId, Vec<RawValue>)>,
}

pub fn freeze_entities(raws: &mut [RawGame], errors: &mut Vec<String>) -> Entities {
    let mut entities: Vec<RawEntity> = raws.iter_mut().flat_map(|r| std::mem::take(&mut r.entities)).collect();
    let mut player = builtin(PLAYER, 0.3, 1.8, ef::FALL_DAMAGE | ef::PUSHABLE);
    player.eye = 1.62;
    player.texture = "core:entity/player".into();
    player.parts = player_parts();
    entities.push(RawEntity { name: PLAYER.into(), cell: 0, desc: player, replace: false, components: Vec::new() });
    entities.push(RawEntity { name: ITEM.into(), cell: 0, desc: builtin(ITEM, 0.125, 0.25, ef::INVULNERABLE), replace: false, components: Vec::new() });
    entities.push(RawEntity { name: VIRTUAL.into(), cell: 0, desc: builtin(VIRTUAL, 0.0, 0.0, ef::INVULNERABLE | ef::NO_GRAVITY), replace: false, components: Vec::new() });
    entities.push(RawEntity { name: FALLING.into(), cell: 0, desc: builtin(FALLING, 0.98, 1.0, ef::INVULNERABLE | ef::SOLID), replace: false, components: Vec::new() });
    entities.sort_by(|a, b| a.name.cmp(&b.name).then(a.replace.cmp(&b.replace)));
    let mut kept: Vec<RawEntity> = Vec::with_capacity(entities.len());
    let mut replaced_cells: Vec<(usize, String)> = Vec::new();
    for e in entities {
        match kept.last() {
            Some(prev) if prev.name == e.name && e.replace => {
                // The replaced kind's handle must resolve to the replacement too.
                if prev.cell != 0 {
                    replaced_cells.push((prev.cell, e.name.clone()));
                }
                kept.pop();
                kept.push(e);
            }
            Some(prev) if prev.name == e.name => {
                errors.push(format!("entity {} registered twice", e.name));
            }
            _ => kept.push(e),
        }
    }
    let mut out = Entities { kinds: Vec::new(), names: FxHashMap::default(), values: Vec::new() };
    for (i, e) in kept.into_iter().enumerate() {
        write_cell(e.cell, i as u32);
        out.names.insert(e.name.clone(), i as u32);
        out.values.push((e.desc.owner, e.components));
        out.kinds.push(e.desc);
    }
    for (cell, name) in replaced_cells {
        write_cell(cell, out.names[&name]);
    }
    out
}

pub fn freeze(
    raws: Vec<RawGame>,
    entities: Entities,
    block_names: &FxHashMap<String, u32>,
    item_names: &FxHashMap<String, u32>,
    items: &[crate::content::ItemDef],
    dim_names: &FxHashMap<String, u32>,
    c: &Components,
    order: &dyn Fn(ModId) -> u32,
    errors: &mut Vec<String>,
) -> Game {
    let mut g = Game::default();
    let mut components: Vec<RawComponent> = Vec::new();
    let mut hooks = Vec::new();
    let mut recipes = Vec::new();
    let mut menus = Vec::new();
    let mut player_menu = None;
    let mut player_layout: Option<(ModId, PlayerLayout)> = None;
    let mut block_entities = Vec::new();
    let mut attach: Vec<(String, String, Option<Vec<u8>>)> = Vec::new();
    let mut packets: Vec<(ModId, String, u32, Callback)> = Vec::new();
    let mut event_handlers: Vec<(ModId, String, String, Callback)> = Vec::new();
    let mut systems: Vec<(ModId, String, String, Callback)> = Vec::new();
    let mut interceptors: Vec<(ModId, String, Callback)> = Vec::new();
    let mut modifiers: Vec<(ModId, String, u32, Callback)> = Vec::new();
    let mut attributes: Vec<(String, f64, f64, f64)> = vec![(abi::attribute::MAX_HEALTH.into(), 20.0, 1.0, 1024.0), (abi::attribute::MOVEMENT_SPEED.into(), 1.0, 0.0, 16.0), (abi::attribute::VISIBILITY.into(), 1.0, 0.0, 16.0)];
    let mut particles: Vec<RawParticle> = engine_particles();
    let mut configs: Vec<(usize, crate::config::ConfigDef)> = crate::config::engine_configs().into_iter().map(|d| (0, d)).collect();
    let mut stations: Vec<(String, String)> = Vec::new();
    let mut sounds: Vec<(usize, SoundDef)> = engine_sounds();
    let mut music: Vec<(String, MusicDef)> = Vec::new();
    let mut spawns: Vec<RawSpawn> = Vec::new();
    // Music first (`MUSIC_CATEGORY`); the order is also that of the volume settings.
    let mut sound_categories: Vec<SoundCategory> = ["music", "blocks", "creatures", "players", "ambient", "ui"].map(|n| SoundCategory::new(&format!("core:{n}"), n != "music" && n != "ui")).to_vec();
    let mut categories: Vec<CategoryDef> = vec![
        CategoryDef { name: "core:misc".into(), cap: 6, despawn: 0.0 },
        CategoryDef { name: "core:hostile".into(), cap: 8, despawn: 128.0 },
        CategoryDef { name: "core:passive".into(), cap: 6, despawn: 0.0 },
    ];
    for r in raws {
        sounds.extend(r.sounds);
        music.extend(r.music);
        particles.extend(r.particles);
        configs.extend(r.configs);
        stations.extend(r.recipe_stations);
        for (owner, name, cb) in r.text_conditions {
            if g.text_conditions.insert(name.clone(), (owner, cb)).is_some() {
                errors.push(format!("text condition {name} registered twice"));
            }
        }
        g.client_hooks.extend(r.client_hooks);
        packets.extend(r.packets);
        g.keys.extend(r.keys);
        g.rules.extend(r.rules);
        spawns.extend(r.spawns);
        event_handlers.extend(r.event_handlers);
        systems.extend(r.systems);
        interceptors.extend(r.interceptors);
        modifiers.extend(r.modifiers);
        attributes.extend(r.attributes);
        components.extend(r.components);
        hooks.extend(r.hooks);
        recipes.extend(r.recipes);
        for k in r.recipe_kinds {
            g.recipe_kinds.insert(k.name.clone());
            g.recipe_kind_defs.insert(k.name, k.def);
        }
        categories.extend(r.categories);
        sound_categories.extend(r.sound_categories);
        menus.extend(r.menus);
        g.huds.extend(r.huds);
        if let Some((owner, name)) = r.player_menu {
            if player_menu.as_ref().is_none_or(|(o, _): &(ModId, String)| order(owner) >= order(*o)) {
                player_menu = Some((owner, name));
            }
        }
        g.screens.extend(r.screens);
        g.screen_parts.extend(r.screen_parts);
        if let Some((owner, l)) = r.player_layout {
            if player_layout.as_ref().is_none_or(|(o, _)| order(owner) >= order(*o)) {
                player_layout = Some((owner, l));
            }
        }
        block_entities.extend(r.block_entities);
        attach.extend(r.attach);
    }
    g.entities = entities.kinds;
    g.entity_names = entities.names;
    g.player_kind = g.entity_names[PLAYER];
    g.item_kind = g.entity_names[ITEM];
    g.falling_kind = g.entity_names[FALLING];
    g.virtual_kind = g.entity_names[VIRTUAL];
    g.client_hooks.sort_by_key(|h| order(h.0));
    freeze_particles(&mut g, particles, errors);
    // Volume settings right after the master volume.
    let volumes: Vec<(usize, crate::config::ConfigDef)> = sound_categories.iter().filter_map(|k| k.setting.as_deref()).map(|s| (0, crate::config::volume(s))).collect();
    configs.splice(1..1, volumes);
    sounds.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    for (cell, mut d) in sounds {
        let category = if d.category_name.is_empty() { "core:blocks" } else { &d.category_name };
        match sound_categories.iter().position(|k| k.name == category) {
            Some(i) => d.category = i as u32,
            None => errors.push(format!("sound {}: no category {category}", d.name)),
        }
        if g.sound_names.contains_key(&d.name) {
            errors.push(format!("sound {} registered twice", d.name));
            continue;
        }
        let id = g.sounds.len() as u32;
        write_cell(cell, id);
        g.sound_names.insert(d.name.clone(), id);
        g.sounds.push(d);
    }
    for (dim, mut m) in music {
        if !dim.is_empty() {
            match dim_names.get(&dim) {
                Some(&d) => m.dimension = Some(d),
                None => {
                    errors.push(format!("music {} plays in unknown dimension {dim}", m.name));
                    continue;
                }
            }
        }
        g.music.push(m);
    }
    for (kind, item) in stations {
        if !g.recipe_kinds.contains(&kind) {
            errors.push(format!("recipe station {item}: no recipe kind {kind}"));
        }
        match item_names.get(&item) {
            Some(&i) => g.recipe_stations.entry(kind).or_default().push(i),
            None => errors.push(format!("recipe station {item} for {kind} is not an item")),
        }
    }
    configs.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    for (cell, d) in configs {
        if g.config_names.contains_key(&d.name) {
            errors.push(format!("setting {} registered twice", d.name));
            continue;
        }
        let id = g.configs.len() as u32;
        write_cell(cell, id);
        g.config_names.insert(d.name.clone(), id);
        g.configs.push(d);
    }
    for d in &categories {
        if categories.iter().filter(|o| o.name == d.name).count() > 1 && !errors.iter().any(|e| e.contains(&d.name)) {
            errors.push(format!("entity category {} registered twice", d.name));
        }
    }
    for k in g.entities.iter_mut().filter(|k| !k.category_name.is_empty()) {
        match categories.iter().position(|d| d.name == k.category_name) {
            Some(i) => k.category = i as u32,
            None => errors.push(format!("entity {}: no category {}", k.name, k.category_name)),
        }
    }
    g.categories = categories;
    g.sound_categories = sound_categories;
    for s in spawns {
        let Some(&entity) = g.entity_names.get(&s.entity) else {
            errors.push(format!("spawn rule for unknown entity {}", s.entity));
            continue;
        };
        let tag = |name: &str, what: &str, errors: &mut Vec<String>| -> Result<Option<u32>, ()> {
            if name.is_empty() {
                return Ok(None);
            }
            c.id(name).map(Some).ok_or_else(|| errors.push(format!("spawn rule for {}: no {what} tag {name}", s.entity)))
        };
        let (Ok(biome), Ok(ground)) = (tag(&s.biome, "biome", errors), tag(&s.ground, "block", errors)) else { continue };
        let dimension = match s.dimension.as_str() {
            "" => None,
            d => match dim_names.get(d) {
                Some(&id) => Some(id),
                None => {
                    errors.push(format!("spawn rule for {}: unknown dimension {d}", s.entity));
                    continue;
                }
            },
        };
        let category = g.entities[entity as usize].category;
        g.spawns.push(SpawnRule { entity, category, biome, ground, dimension, light: s.light, y: s.y, group: s.group, weight: s.weight, chance: s.chance });
    }
    // Natural spawning (`spawns`) runs while this is on.
    g.rules.push(("core:mob_spawning".into(), 1));
    g.rules.sort_by(|a, b| a.0.cmp(&b.0));
    for w in g.rules.windows(2).filter(|w| w[0].0 == w[1].0) {
        errors.push(format!("rule {} is registered twice", w[0].0));
    }
    g.rules.dedup_by(|a, b| a.0 == b.0);
    g.mob_spawning_rule = g.rules.iter().position(|r| r.0 == "core:mob_spawning").unwrap_or(0);

    let mut events: Vec<EventDef> = ENGINE_EVENTS.iter().map(|(n, s, size)| EventDef { name: n.to_string(), schema: s.to_string(), size: (*size).into(), handlers: Vec::new() }).collect();
    event_handlers.sort_by_key(|h| order(h.0));
    for (owner, name, schema, cb) in event_handlers {
        match events.iter_mut().find(|e| e.name == name) {
            Some(e) if e.schema != schema => errors.push(format!("event {name} is '{}' but a handler takes '{schema}'", e.schema)),
            Some(e) => e.handlers.push((owner, cb)),
            None => events.push(EventDef { name, schema, size: 0.into(), handlers: vec![(owner, cb)] }),
        }
    }
    events.sort_by(|a, b| a.name.cmp(&b.name));
    g.event_names = events.iter().enumerate().map(|(i, e)| (e.name.clone(), i as u32)).collect();
    g.events = events;
    interceptors.sort_by_key(|h| order(h.0));
    for (owner, name, cb) in interceptors {
        g.interceptors.entry(name).or_default().push((owner, cb));
    }
    modifiers.sort_by_key(|m| (m.2, order(m.0)));
    for (owner, name, stage, cb) in modifiers {
        g.modifiers.entry(name).or_default().push((stage, owner, cb));
    }
    for (name, default, min, max) in attributes {
        if let Some(&a) = g.attr_names.get(&name) {
            // Declared again the same way is fine (mods sharing it); differently is not.
            let d = &g.attributes[a as usize];
            if (d.default, d.min, d.max) != (default, min.min(max), max) {
                errors.push(format!("attribute {name} is declared twice with different values"));
            }
            continue;
        }
        g.attr_names.insert(name.clone(), g.attributes.len() as u32);
        g.attributes.push(AttrDef { hash: name_hash(&name), comp: c.id(&name), name, default, min: min.min(max), max });
    }
    packets.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
    for (owner, name, side, cb) in packets {
        let id = *g.packet_names.entry(name.clone()).or_insert_with(|| {
            g.packets.push(PacketDef::default());
            g.packets.len() as u32 - 1
        });
        let def = &mut g.packets[id as usize];
        let slot = if side == 0 { &mut def.server } else { &mut def.client };
        if slot.is_some() {
            errors.push(format!("packet channel {name} has two {} handlers", if side == 0 { "server" } else { "client" }));
        }
        *slot = Some((owner, cb));
    }

    // Entity storage, indexed by component id like everything else.
    g.components = c.names.iter().map(|n| ComponentDef { name: n.clone(), ..Default::default() }).collect();
    let energy = size_of::<weft_abi::Energy>() as u32;
    components.push(RawComponent {
        owner: ENGINE,
        def: ComponentDef { name: abi::comp::ENERGY.into(), size: energy, flags: weft_abi::component_flags::SAVE | weft_abi::component_flags::SYNC_MENU, default: vec![0; energy as usize], runtime: true },
        schema: String::new(),
    });
    // Variable length (size 0): attribute modifiers, added on demand.
    components.push(RawComponent {
        owner: ENGINE,
        def: ComponentDef { name: MODIFIERS.into(), size: 0, flags: weft_abi::component_flags::SAVE, default: Vec::new(), runtime: true },
        schema: String::new(),
    });
    for rc in components {
        let Some(id) = c.id(&rc.def.name) else { continue };
        g.components[id as usize] = ComponentDef { runtime: true, ..rc.def };
    }
    g.component_names = g.components.iter().enumerate().filter(|(_, d)| d.runtime).map(|(i, d)| (d.name.clone(), i as u32)).collect();
    g.energy = g.component_names[abi::comp::ENERGY];
    g.modifiers_comp = g.component_names[MODIFIERS];
    let with_flag = |f: u32| g.components.iter().enumerate().filter(|(_, d)| d.runtime && d.flags & f != 0).map(|(i, _)| i as u32).collect::<Vec<u32>>();
    g.watched_comps = with_flag(abi::component_flags::SYNC_WATCHERS);
    g.menu_comps = with_flag(abi::component_flags::SYNC_MENU);
    let any_sync = abi::component_flags::SYNC_OWNER | abi::component_flags::SYNC_WATCHERS | abi::component_flags::SYNC_MENU;
    g.synced_comps = g.components.iter().map(|d| d.runtime && d.flags & any_sync != 0).collect();
    systems.sort_by_key(|s| order(s.0));
    for (owner, name, schema, cb) in systems {
        match g.component_names.get(&name) {
            Some(&id) if c.schemas[id as usize] == schema || schema.is_empty() => g.systems.push((owner, id, cb)),
            Some(&id) => errors.push(format!("system over {name}: it is '{}', the system takes '{schema}'", c.schemas[id as usize])),
            None => errors.push(format!("system over {name}: no entity component of that name")),
        }
    }
    for (kind, comp, value) in attach {
        match (g.entity_names.get(&kind), g.component_names.get(&comp)) {
            (Some(&k), Some(&cid)) => {
                let size = g.components[cid as usize].size as usize;
                match value {
                    Some(v) if v.len() != size => errors.push(format!("attaching {comp} to {kind}: a value of {} bytes, the component has {size}", v.len())),
                    Some(v) => g.entities[k as usize].values.push((cid, v.into_boxed_slice())),
                    None => {}
                }
                g.entities[k as usize].components.push(cid);
            }
            _ => errors.push(format!("cannot attach {comp} to {kind}: unknown entity or component")),
        }
    }

    // Hooks run after the kind's own callback, in mod order.
    hooks.sort_by_key(|h: &(ModId, String, u32, Callback)| order(h.0));
    for (owner, kind, hook, cb) in hooks {
        let Some(&e) = g.entity_names.get(&kind) else {
            errors.push(format!("hook on unknown entity {kind}"));
            continue;
        };
        let k = &mut g.entities[e as usize];
        match hook {
            abi::entity_hook::TICK => k.on_tick.push((owner, cb)),
            abi::entity_hook::HURT => k.on_hurt.push((owner, cb)),
            abi::entity_hook::DEATH => k.on_death.push((owner, cb)),
            _ => k.on_interact.push((owner, cb)),
        }
    }

    menus.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    for (i, (cell, m)) in menus.into_iter().enumerate() {
        write_cell(cell, i as u32);
        g.menu_names.insert(m.name.clone(), i as u32);
        g.menus.push(m);
    }
    g.screens.sort_by_key(|s| order(s.owner));
    g.screen_parts.sort_by_key(|p| (p.order, order(p.owner)));
    g.player_layout = player_layout.map(|l| l.1).unwrap_or_default();
    let l = &g.player_layout;
    if l.hotbar > l.slots || l.worn.end > l.slots || l.worn.start < l.hotbar {
        errors.push(format!("player inventory: hotbar {} and worn {:?} must lie within {} slots, worn after the hotbar", l.hotbar, l.worn, l.slots));
        g.player_layout = PlayerLayout::default();
    }
    g.player_menu = player_menu.and_then(|(_, n)| {
        let m = g.menu_names.get(&n).copied();
        if m.is_none() {
            errors.push(format!("player menu {n} is not registered"));
        }
        m
    });

    for (block, entity, slots) in block_entities {
        match (block_names.get(&block), g.entity_names.get(&entity)) {
            (Some(&b), Some(&e)) => {
                if g.block_entities.insert(b, e).is_some_and(|old| old != e) {
                    errors.push(format!("block {block} has two block entities"));
                }
                g.entities[e as usize].slots = slots;
            }
            _ => errors.push(format!("block entity {block} -> {entity}: unknown block or entity")),
        }
    }

    // Items per tag, most preferred first (mod load order, then registration order): a tag as
    // a recipe output gives its first item.
    let mut by_tag: FxHashMap<&str, Vec<u32>> = FxHashMap::default();
    let mut ordered: Vec<u32> = (1..items.len() as u32).collect();
    ordered.sort_by_key(|&i| items[i as usize].order);
    for &i in &ordered {
        for t in c.of(comps::ITEM, i).filter(|&t| c.is_tag(t)) {
            by_tag.entry(c.names[t as usize].as_str()).or_default().push(i);
        }
    }
    let recipes: Vec<RawRecipe> = recipes.into_iter().flat_map(|r| expand_wildcards(r, &by_tag)).collect();
    let ing = |s: &str, errors: &mut Vec<String>| -> Ingredient {
        if let Some(f) = s.strip_prefix("fluid:") {
            Ingredient::Fluid(f.to_string())
        } else if let Some(t) = s.strip_prefix('#') {
            if !by_tag.contains_key(t) {
                // Not an error: the items may come from a mod that is not installed.
                log::warn!("a recipe uses #{t}, but no item has that tag");
            }
            Ingredient::Tag(c.id(t).unwrap_or(u32::MAX))
        } else if s.is_empty() {
            Ingredient::Empty
        } else {
            match item_names.get(s) {
                Some(&i) => Ingredient::Item(i),
                None => {
                    errors.push(format!("recipe uses unknown item {s}"));
                    Ingredient::Empty
                }
            }
        }
    };
    for r in recipes {
        if !g.recipe_kinds.contains(&r.kind) {
            errors.push(format!("recipe for {}: no recipe kind {}", r.output, r.kind));
            continue;
        }
        let output = match r.output.strip_prefix('#') {
            Some(t) => by_tag.get(t).and_then(|v| v.first()).copied(),
            None => item_names.get(&r.output).copied(),
        };
        let Some(output) = output else {
            errors.push(format!("recipe makes unknown item {}", r.output));
            continue;
        };
        let shaped = if r.pattern.is_empty() {
            None
        } else {
            let h = r.pattern.len() as u32;
            let w = r.pattern.iter().map(|p| p.chars().count()).max().unwrap_or(0) as u32;
            let mut cells = Vec::new();
            for row in &r.pattern {
                let chars: Vec<char> = row.chars().collect();
                for x in 0..w as usize {
                    let ch = chars.get(x).copied().unwrap_or(' ');
                    let cell = if ch == ' ' {
                        Ingredient::Empty
                    } else {
                        match r.keys.iter().find(|k| k.0 == ch) {
                            Some((_, item)) => ing(item, errors),
                            None => {
                                errors.push(format!("recipe for {}: key '{ch}' not defined", r.output));
                                Ingredient::Empty
                            }
                        }
                    };
                    cells.push(cell);
                }
            }
            Some((w, h, cells))
        };
        let inputs: Vec<Ingredient> = r.inputs.iter().map(|s| ing(s, errors)).collect();
        let input_counts = (0..inputs.len()).map(|i| r.input_counts.get(i).copied().unwrap_or(1).max(1)).collect();
        let mut outputs = vec![RecipeOutput { what: Ingredient::Item(output), count: r.count, chance: 1.0 }];
        for (name, count, chance) in &r.outputs {
            let what = match name.strip_prefix('#') {
                Some(t) => by_tag.get(t).and_then(|v| v.first()).map(|&i| Ingredient::Item(i)),
                None => Some(ing(name, errors)),
            };
            match what {
                Some(Ingredient::Empty) | None => errors.push(format!("recipe for {}: unknown output {name}", r.output)),
                Some(w) => outputs.push(RecipeOutput { what: w, count: *count, chance: chance.clamp(0.0, 1.0) }),
            }
        }
        let values = r.values.iter().filter_map(|v| c.id(&v.name).map(|id| (id, v.bytes.clone()))).collect();
        let hidden = r.flags & weft_abi::recipe_flags::HIDDEN != 0;
        g.recipes.entry(r.kind).or_default().push(Recipe { shaped, inputs, input_counts, outputs, time: r.time, values, hidden });
    }
    g
}

impl Game {
    /// Entities of `kind` belong to a block (machines, chests): they come and go with it.
    pub fn is_block_entity(&self, kind: u32) -> bool {
        self.block_entities.values().any(|&k| k == kind)
    }

    pub fn component(&self, name: &str) -> Option<u32> {
        self.component_names.get(name).copied()
    }

    /// Finds the recipe whose pattern fits the grid (any position, mirrored allowed) or whose
    /// shapeless inputs match the grid contents exactly.
    pub fn match_grid(&self, kind: &str, grid: &[u32], w: u32, h: u32, tags: &Components) -> Option<&Recipe> {
        let list = self.recipes.get(kind)?;
        // Trim the grid to the bounding box of its items.
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if grid[(y * w + x) as usize] != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 == 0 {
            return None;
        }
        let (bw, bh) = (x1 - x0, y1 - y0);
        let at = |x: u32, y: u32| grid[((y0 + y) * w + x0 + x) as usize];
        let items: Vec<u32> = grid.iter().copied().filter(|&i| i != 0).collect();
        list.iter().find(|r| match &r.shaped {
            Some((rw, rh, cells)) => {
                if *rw != bw || *rh != bh {
                    return false;
                }
                let fits = |mirror: bool| {
                    (0..bh).all(|y| {
                        (0..bw).all(|x| {
                            let cx = if mirror { bw - 1 - x } else { x };
                            cells[(y * rw + cx) as usize].matches(at(x, y), tags)
                        })
                    })
                };
                fits(false) || fits(true)
            }
            None => {
                if r.inputs.len() != items.len() {
                    return false;
                }
                let mut used = vec![false; items.len()];
                r.inputs.iter().all(|ing| {
                    let found = items.iter().enumerate().find(|(i, it)| !used[*i] && ing.matches(**it, tags));
                    match found {
                        Some((i, _)) => {
                            used[i] = true;
                            true
                        }
                        None => false,
                    }
                })
            }
        })
    }

    pub fn process(&self, kind: &str, input: u32, tags: &Components) -> Option<&Recipe> {
        self.recipes.get(kind)?.iter().find(|r| r.inputs.first().is_some_and(|i| i.matches(input, tags)))
    }

    /// For each input of `r`, the stack in `stacks` that holds enough of it (each stack used
    /// once); `None` if one is missing. Fluids are not in stacks and never match.
    pub fn find_inputs(&self, r: &Recipe, stacks: &[weft_abi::ItemStack], tags: &Components) -> Option<Vec<usize>> {
        let mut used = vec![false; stacks.len()];
        let mut out = Vec::with_capacity(r.inputs.len());
        for (ing, &n) in r.inputs.iter().zip(&r.input_counts) {
            if matches!(ing, Ingredient::Empty) {
                continue;
            }
            let i = stacks.iter().enumerate().position(|(i, s)| !used[i] && !s.is_empty() && s.count >= n && ing.matches(s.item, tags))?;
            used[i] = true;
            out.push(i);
        }
        Some(out)
    }
}

/// Sounds the engine plays itself (files in its own assets, packs may replace them).
fn engine_sounds() -> Vec<(usize, SoundDef)> {
    let one = |name: &str, file: &str, category: &str, volume: f32, pitch: [f32; 2]| (0, SoundDef { name: name.into(), files: vec![file.into()], category: 0, category_name: category.into(), volume, pitch, range: 16.0 });
    vec![
        one("core:ui.click", "core:ui/click", "core:ui", 0.6, [1.0, 1.0]),
        one("core:item.pickup", "core:item/pickup", "core:players", 0.4, [1.4, 2.2]),
    ]
}

/// Kinds the engine spawns itself; mods may replace their look by registering the same name.
fn engine_particles() -> Vec<RawParticle> {
    use abi::particle_flags as pf;
    let block = abi::ParticleDesc { life: [0.2, 1.0], scale: [0.05, 0.1], gravity: 16.0, color: 0x999999FF, color_end: 0x999999FF, flags: pf::COLLIDE, ..abi::ParticleDesc::DEFAULT };
    let poof = abi::ParticleDesc { life: [0.4, 1.1], scale: [0.15, 0.3], gravity: -2.0, drag: 0.9, color: 0xD0D0D0FF, color_end: 0x909090FF, ..abi::ParticleDesc::DEFAULT };
    let frames = (0..8).map(|i| format!("core:particle/generic_{}", 7 - i)).collect();
    vec![
        RawParticle { name: abi::particle::BLOCK.into(), cell: 0, textures: Vec::new(), desc: block, shader: String::new() },
        RawParticle { name: abi::particle::POOF.into(), cell: 0, textures: frames, desc: poof, shader: String::new() },
    ]
}

/// Ids by sorted name; a later registration of a name (a mod after the engine) replaces it.
fn freeze_particles(g: &mut Game, raws: Vec<RawParticle>, errors: &mut Vec<String>) {
    let mut by_name: std::collections::BTreeMap<String, RawParticle> = Default::default();
    for p in raws {
        if p.name.is_empty() {
            errors.push("particle without a name".into());
            continue;
        }
        by_name.insert(p.name.clone(), p);
    }
    let mut tex_ids: FxHashMap<String, u32> = FxHashMap::default();
    for (i, (name, p)) in by_name.into_iter().enumerate() {
        let frames = p
            .textures
            .iter()
            .map(|t| {
                *tex_ids.entry(t.clone()).or_insert_with(|| {
                    g.particle_textures.push(t.clone());
                    g.particle_textures.len() as u32 - 1
                })
            })
            .collect();
        let d = p.desc;
        write_cell(p.cell, i as u32);
        let order = |r: [f32; 2]| [r[0].min(r[1]), r[0].max(r[1])];
        g.particle_names.insert(name.clone(), i as u32);
        g.particles.push(ParticleDef {
            name,
            frames,
            life: order(d.life).map(|v| v.max(0.01)),
            scale: order(d.scale),
            gravity: d.gravity,
            drag: d.drag.clamp(0.0, 1.0),
            color: d.color,
            color_end: d.color_end,
            flags: d.flags,
            shader: p.shader,
        });
    }
}

/// A recipe naming `#a/*` tags stands for one recipe per material: `*` takes every value for which
/// the first such tag exists, and the copies whose other tags have no items are dropped. So
/// `#ores/* -> #ingots/*` smelts every mod's ores into the matching ingots.
fn expand_wildcards(r: RawRecipe, by_tag: &FxHashMap<&str, Vec<u32>>) -> Vec<RawRecipe> {
    let names: Vec<&String> = r.keys.iter().map(|k| &k.1).chain(&r.inputs).chain(std::iter::once(&r.output)).chain(r.outputs.iter().map(|o| &o.0)).collect();
    let Some(pattern) = names.iter().find_map(|n| n.strip_prefix('#').filter(|t| t.contains('*'))) else { return vec![r] };
    let (pre, post) = pattern.split_once('*').unwrap();
    let mut values: Vec<&str> = by_tag
        .keys()
        .filter_map(|t| t.strip_prefix(pre).and_then(|rest| rest.strip_suffix(post)))
        .filter(|v| !v.is_empty())
        .collect();
    values.sort_unstable();
    let mut out = Vec::new();
    for v in values {
        let sub = |s: &String| s.replace('*', v);
        let usable = names.iter().all(|n| n.strip_prefix('#').is_none_or(|t| by_tag.contains_key(t.replace('*', v).as_str())));
        if usable {
            out.push(RawRecipe {
                kind: r.kind.clone(),
                pattern: r.pattern.clone(),
                keys: r.keys.iter().map(|(c, n)| (*c, sub(n))).collect(),
                inputs: r.inputs.iter().map(sub).collect(),
                input_counts: r.input_counts.clone(),
                output: sub(&r.output),
                count: r.count,
                time: r.time,
                outputs: r.outputs.iter().map(|o| (sub(&o.0), o.1, o.2)).collect(),
                values: r.values.clone(),
                flags: r.flags,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_recipes_expand_per_material() {
        let mut by_tag: FxHashMap<&str, Vec<u32>> = FxHashMap::default();
        for (t, i) in [("ores/iron", 1), ("ingots/iron", 2), ("ores/tin", 3), ("ingots/tin", 4), ("ores/ruby", 5)] {
            by_tag.entry(t).or_default().push(i);
        }
        let r = RawRecipe { kind: "smelting".into(), inputs: vec!["#ores/*".into()], output: "#ingots/*".into(), count: 1, time: 200, ..Default::default() };
        let out: Vec<(String, String)> = expand_wildcards(r, &by_tag).into_iter().map(|r| (r.inputs[0].clone(), r.output)).collect();
        // Ruby has no ingot, so it is left out.
        assert_eq!(out, vec![("#ores/iron".into(), "#ingots/iron".into()), ("#ores/tin".into(), "#ingots/tin".into())]);
    }
}
