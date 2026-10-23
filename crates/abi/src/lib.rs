//! The stable boundary between the engine and mods. Everything here is `repr(C)` so a mod built
//! with any rustc (or another language) keeps working. Rules:
//! - [`Host`] only grows at the end, `size` tells a mod how much of it exists.
//! - Descriptor structs passed to the host are read during the call only.
//! - Strings are UTF-8 `(ptr, len)`, never NUL terminated.
#![no_std]

use core::ffi::c_void;
use core::sync::atomic::AtomicU32;

pub const ABI_VERSION: u32 = 1;
/// Symbol every mod library exports: `extern "C" fn() -> *const ModDecl`.
pub const ENTRY_SYMBOL: &[u8] = b"voxide_mod_v1\0";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Str {
    pub ptr: *const u8,
    pub len: usize,
}

unsafe impl Send for Str {}
unsafe impl Sync for Str {}

impl Str {
    pub const EMPTY: Str = Str { ptr: core::ptr::null(), len: 0 };

    pub const fn new(s: &str) -> Str {
        Str { ptr: s.as_ptr(), len: s.len() }
    }

    /// # Safety
    /// The pointer must reference valid UTF-8 for the chosen lifetime.
    pub unsafe fn as_str<'a>(&self) -> &'a str {
        if self.len == 0 {
            return "";
        }
        unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(self.ptr, self.len)) }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Slice<T> {
    pub ptr: *const T,
    pub len: usize,
}

unsafe impl<T: Sync> Sync for Slice<T> {}
unsafe impl<T: Send> Send for Slice<T> {}

impl<T> Slice<T> {
    pub const EMPTY: Slice<T> = Slice { ptr: core::ptr::null(), len: 0 };

    pub const fn new(s: &[T]) -> Slice<T> {
        Slice { ptr: s.as_ptr(), len: s.len() }
    }

    /// # Safety
    /// Must point to `len` valid elements for the chosen lifetime.
    pub unsafe fn as_slice<'a>(&self) -> &'a [T] {
        if self.len == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

/// A mod-side function plus its captured state. `args` points to the callback-specific args struct.
/// Returns 0 on success, anything else means the callback failed (the SDK catches panics).
pub type CallbackFn = unsafe extern "C" fn(data: *const c_void, args: *mut c_void) -> u32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Callback {
    pub func: Option<CallbackFn>,
    pub data: *const c_void,
}

unsafe impl Send for Callback {}
unsafe impl Sync for Callback {}

impl Callback {
    pub const NONE: Callback = Callback { func: None, data: core::ptr::null() };

    pub fn is_some(&self) -> bool {
        self.func.is_some()
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModId(pub u32);

/// Opaque per-mod registration container.
#[repr(C)]
pub struct Registrar {
    _p: [u8; 0],
}

/// Opaque view of one world (dimension) handed to callbacks. Only valid during the call.
#[repr(C)]
pub struct WorldCtx {
    _p: [u8; 0],
}

#[repr(C)]
pub struct ModDecl {
    pub abi_version: u32,
    pub id: Str,
    pub name: Str,
    pub version: Str,
    /// Mod ids this one must load after. Registration itself is parallel, this only orders
    /// `post_init` and callback priority.
    pub deps: Slice<Str>,
    pub init: unsafe extern "C" fn(host: *const Host, me: ModId, reg: *mut Registrar) -> u32,
}

unsafe impl Sync for ModDecl {}

// ---------------------------------------------------------------- content descriptors

pub mod block_flags {
    /// Entities collide with it.
    pub const SOLID: u32 = 1;
    /// Full opaque cube: hides neighbour faces, blocks light, gets ambient occlusion.
    pub const OPAQUE: u32 = 2;
    /// Placing a block into it replaces it (air, grass, water).
    pub const REPLACEABLE: u32 = 4;
    pub const LIQUID: u32 = 8;
    pub const CLIMBABLE: u32 = 16;
    /// Raycasts pass through (air, liquids for normal interaction).
    pub const NOT_TARGETABLE: u32 = 32;
    /// Receives random ticks.
    pub const RANDOM_TICKS: u32 = 64;
    /// Hide faces between two blocks of this kind (glass). Without it see-through blocks keep
    /// their inner faces, which is what makes leaves look dense.
    pub const CULL_SAME: u32 = 128;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    None = 0,
    Cube = 1,
    /// Two diagonal quads (plants).
    Cross = 2,
    /// Cube with height from the state's `level` (0..8), surface faces only against other liquids.
    Liquid = 3,
    /// `boxes` with their own textures.
    Boxes = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Opaque = 0,
    /// Alpha tested (leaves, glass, plants).
    Cutout = 1,
    Translucent = 2,
}

/// Faces in order: -X, +X, -Y, +Y, -Z, +Z.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModelBox {
    /// In 1/16 block units.
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub textures: [Str; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Prop {
    pub key: Str,
    pub value: Slice<u8>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StateDesc {
    pub flags: u32,
    /// Emitted light per colour channel. 15 is a normal full-strength source (torch is 14); higher
    /// values are allowed and simply reach further (up to 255).
    pub light: [u8; 3],
    /// Extra per-channel absorption on top of `opacity`, for coloured glass.
    pub filter: [u8; 3],
    /// How much light it absorbs per block, 0 for air/glass, 15 or more blocks it fully.
    pub opacity: u8,
    /// 0 none, 1 grass, 2 foliage, 3 water (biome colours).
    pub tint: u8,
    /// Which faces the tint applies to, bit per face in -X, +X, -Y, +Y, -Z, +Z order.
    pub tint_faces: u8,
    pub layer: Layer,
    /// Liquid level 0..7 (0 = source), meaningful for `Model::Liquid`.
    pub level: u8,
    pub model: Model,
    pub textures: [Str; 6],
    pub boxes: Slice<ModelBox>,
    /// Collision/selection box in 1/16 units; ignored for full cubes.
    pub collision: Slice<[f32; 6]>,
    /// Seconds to break by hand at speed 1; negative = unbreakable.
    pub hardness: f32,
    pub friction: f32,
    /// Movement speed multiplier while inside (cobweb, liquids).
    pub drag: f32,
}

#[repr(C)]
pub struct BlockDesc {
    pub name: Str,
    /// Receives the id of the first state once registries are frozen.
    pub cell: *const AtomicU32,
    pub states: Slice<StateDesc>,
    /// Tool class tag that speeds up mining ("pickaxe"), empty = any.
    pub tool: Str,
    /// Minimum harvest level of the tool for the block to drop anything.
    pub harvest_level: u32,
    pub tags: Slice<Str>,
    pub props: Slice<Prop>,
    /// Item dropped when broken, empty = the item with the same name if it exists.
    pub drop: Str,
    pub drop_count: u32,
    /// `UseArgs`; return `handled = 1` to consume the click.
    pub on_use: Callback,
    /// `BlockEventArgs`.
    pub on_placed: Callback,
    pub on_broken: Callback,
    pub on_random_tick: Callback,
    pub on_neighbor: Callback,
    pub on_scheduled: Callback,
    /// `PlaceArgs`: choose the state from the player's facing/hit face.
    pub placement: Callback,
    /// Non-zero: replaces a block of the same name registered by a mod this one depends on.
    pub replace: u32,
}

#[repr(C)]
pub struct ItemDesc {
    pub name: Str,
    pub cell: *const AtomicU32,
    pub max_stack: u32,
    /// 0 = not damageable.
    pub durability: u32,
    /// Texture for the flat icon. Empty with `block` set means a 3D block icon.
    pub texture: Str,
    /// Block placed on use, empty = none.
    pub block: Str,
    pub tool: Str,
    pub tool_level: u32,
    pub tool_speed: f32,
    pub attack: f32,
    pub tags: Slice<Str>,
    pub props: Slice<Prop>,
    /// `ItemUseArgs`.
    pub on_use: Callback,
}

#[repr(C)]
pub struct BiomeDesc {
    pub name: Str,
    pub cell: *const AtomicU32,
    pub grass: u32,
    pub foliage: u32,
    pub water: u32,
    pub temperature: f32,
    pub rainfall: f32,
}

#[repr(C)]
pub struct GenArgs {
    pub seed: u64,
    pub cube: [i32; 3],
    /// 4096 state ids, index = (y * 16 + z) * 16 + x. Pre-filled with 0 (air).
    pub blocks: *mut u32,
    /// 256 biome ids per column, index = z * 16 + x.
    pub biomes: *mut u8,
}

#[repr(C)]
pub struct SpawnArgs {
    pub seed: u64,
    /// Output, block position the player stands on top of.
    pub pos: [i32; 3],
}

#[repr(C)]
pub struct DimensionDesc {
    pub name: Str,
    pub cell: *const AtomicU32,
    /// `GenArgs`, called in parallel from worker threads, must be a pure function of its args.
    pub generator: Callback,
    /// `SpawnArgs`.
    pub spawn: Callback,
    /// Inclusive cube-y bounds, `i32::MIN`/`i32::MAX` for unbounded.
    pub min_cube_y: i32,
    pub max_cube_y: i32,
    /// Cubes at or above this y are guaranteed empty by the generator, so sky light there is full
    /// without generating them. `i32::MAX` if unknown.
    pub sky_cube_y: i32,
    pub sky_light: u32,
    pub gravity: f32,
    /// Ticks per day, 0 = time does not advance.
    pub day_length: u32,
    pub fixed_time: u32,
    pub sky_color: u32,
    pub fog_color: u32,
    /// Light level everything gets at least, 0..15.
    pub ambient: u32,
}

// ---------------------------------------------------------------- callback args

#[repr(C)]
pub struct BlockEventArgs {
    pub world: *mut WorldCtx,
    pub pos: [i32; 3],
    pub state: u32,
    /// Neighbour position for `on_neighbor`, otherwise unused.
    pub other: [i32; 3],
    pub entity: u64,
}

#[repr(C)]
pub struct UseArgs {
    pub world: *mut WorldCtx,
    pub pos: [i32; 3],
    pub state: u32,
    pub face: u32,
    pub player: u64,
    pub handled: u32,
}

#[repr(C)]
pub struct PlaceArgs {
    pub pos: [i32; 3],
    pub face: u32,
    /// Player look direction.
    pub look: [f32; 3],
    /// In/out: starts as the first state.
    pub state: u32,
}

#[repr(C)]
pub struct ItemUseArgs {
    pub world: *mut WorldCtx,
    pub player: u64,
    /// In/out: the held stack; change it to consume (eating, placing).
    pub stack: ItemStack,
    /// Target block if any (`has_target`).
    pub pos: [i32; 3],
    pub face: u32,
    pub has_target: u32,
    pub handled: u32,
}

// ---------------------------------------------------------------- items, entities, menus

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Hash)]
pub struct ItemStack {
    pub item: u32,
    pub count: u32,
    pub damage: u32,
    /// Reserved for item data handles; 0 for plain stacks.
    pub extra: u32,
}

impl ItemStack {
    pub const EMPTY: ItemStack = ItemStack { item: 0, count: 0, damage: 0, extra: 0 };

    pub fn new(item: u32, count: u32) -> ItemStack {
        ItemStack { item, count, damage: 0, extra: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.item == 0 || self.count == 0
    }
}

pub mod entity_flags {
    pub const FALL_DAMAGE: u32 = 1;
    /// Other entities push it away.
    pub const PUSHABLE: u32 = 2;
    pub const NO_GRAVITY: u32 = 4;
    pub const INVULNERABLE: u32 = 8;
    /// Burns in daylight (undead).
    pub const BURNS_IN_SUN: u32 = 16;
    pub const SWIMS: u32 = 32;
}

/// Which body part a model piece is, so the engine can animate it (walk cycle, head look).
pub mod part_role {
    pub const STATIC: u32 = 0;
    pub const HEAD: u32 = 1;
    pub const ARM_LEFT: u32 = 2;
    pub const ARM_RIGHT: u32 = 3;
    pub const LEG_LEFT: u32 = 4;
    pub const LEG_RIGHT: u32 = 5;
    /// Quadrupeds: front legs swing opposite to back legs.
    pub const LEG_FRONT_LEFT: u32 = 6;
    pub const LEG_FRONT_RIGHT: u32 = 7;
    /// Arms held forward (zombies).
    pub const ARM_FORWARD_LEFT: u32 = 8;
    pub const ARM_FORWARD_RIGHT: u32 = 9;
    /// Legs that sweep sideways while walking (spiders), in opposite phase.
    pub const LEG_SWEEP_A: u32 = 10;
    pub const LEG_SWEEP_B: u32 = 11;
}

/// Box in model pixels (1/16 block) relative to the part pivot; `uv` is the top-left texel of the
/// classic box unwrap.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModelCuboid {
    pub origin: [f32; 3],
    pub size: [f32; 3],
    pub uv: [f32; 2],
    pub inflate: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModelPart {
    /// Pivot in model pixels, y up from the feet.
    pub pivot: [f32; 3],
    pub role: u32,
    pub cuboids: Slice<ModelCuboid>,
    /// Resting rotation in radians about x, y, z, applied x first; the role's animation adds to it.
    pub rest: [f32; 3],
}

#[repr(C)]
pub struct EntityDesc {
    pub name: Str,
    pub cell: *const AtomicU32,
    pub width: f32,
    pub height: f32,
    pub eye: f32,
    pub max_health: f32,
    /// Walking speed in blocks per second.
    pub speed: f32,
    pub flags: u32,
    /// 0 misc, 1 hostile, 2 passive. Spawning rules are up to mods; this feeds caps and profiling.
    pub category: u32,
    pub texture: Str,
    pub texture_size: [u32; 2],
    pub parts: Slice<ModelPart>,
    /// `EntityTickArgs`: every entity of this kind in one call.
    pub on_tick: Callback,
    /// `EntityEventArgs`.
    pub on_hurt: Callback,
    pub on_death: Callback,
    pub on_interact: Callback,
    /// Non-zero: replaces an entity kind of the same name from a mod this one depends on.
    pub replace: u32,
}

#[repr(C)]
pub struct EntityTickArgs {
    pub world: *mut WorldCtx,
    pub kind: u32,
    pub ids: *const u64,
    pub count: usize,
}

#[repr(C)]
pub struct EntityEventArgs {
    pub world: *mut WorldCtx,
    pub entity: u64,
    /// Attacker / interacting player, 0 if none.
    pub other: u64,
    /// In/out for `on_hurt`: change it to reduce damage (armour).
    pub amount: f32,
    pub cause: u32,
    pub handled: u32,
}

pub mod damage {
    pub const GENERIC: u32 = 0;
    pub const FALL: u32 = 1;
    pub const ATTACK: u32 = 2;
    pub const FIRE: u32 = 3;
    pub const STARVE: u32 = 4;
    pub const DROWN: u32 = 5;
    pub const VOID: u32 = 6;
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct EntityState {
    pub kind: u32,
    pub flags: u32,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: u32,
    pub in_liquid: u32,
    pub health: f32,
    pub max_health: f32,
    pub age: u64,
    /// Blocks fallen since last on ground.
    pub fall: f32,
    /// Distance walked this tick (hunger exhaustion).
    pub moved: f32,
}

pub mod component_flags {
    pub const SAVE: u32 = 1;
    /// Sent to the player that owns it (HUD values like hunger).
    pub const SYNC_OWNER: u32 = 2;
}

#[repr(C)]
pub struct ComponentDesc {
    pub name: Str,
    pub cell: *const AtomicU32,
    pub size: u32,
    pub flags: u32,
    /// Initial bytes, `size` long.
    pub default: Slice<u8>,
    /// Entity kinds ("core:player") that get this component when spawned.
    pub attach_to: Slice<Str>,
}

/// Recipe ingredients are item names or `#tag`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RecipeKey {
    pub ch: u32,
    pub item: Str,
}

#[repr(C)]
pub struct RecipeDesc {
    /// Recipe type, e.g. "base:crafting", "base:smelting". Any mod can add types.
    pub kind: Str,
    /// Shaped recipes: rows of key characters. Empty for shapeless/processing.
    pub pattern: Slice<Str>,
    pub keys: Slice<RecipeKey>,
    /// Shapeless/processing inputs.
    pub inputs: Slice<Str>,
    pub output: Str,
    pub count: u32,
    /// Processing time in ticks, meaning is up to the recipe type.
    pub time: u32,
}

pub mod slot_flags {
    /// Can only be taken from (crafting result).
    pub const OUTPUT: u32 = 1;
    pub const ARMOR_HEAD: u32 = 2;
    pub const ARMOR_CHEST: u32 = 4;
    pub const ARMOR_LEGS: u32 = 8;
    pub const ARMOR_FEET: u32 = 16;
}

#[repr(C)]
pub struct MenuDesc {
    pub name: Str,
    pub cell: *const AtomicU32,
    /// Slots in the menu's own temporary inventory (crafting grids), returned to the player on close.
    pub temp_slots: u32,
    /// Synced integers (progress values) the screen can bind to.
    pub data_count: u32,
    /// `MenuArgs`, server: add slots with `Host::menu_slots`.
    pub setup: Callback,
    /// `MenuArgs`, server: a slot changed (recompute crafting output).
    pub on_change: Callback,
    /// `MenuArgs` with `slot` set, server: an OUTPUT slot was taken (consume ingredients).
    pub on_take: Callback,
    /// `MenuArgs`, server, every tick while open: refresh data values.
    pub on_tick: Callback,
    /// `UiArgs`, client: build the screen. Slot widgets refer to menu slot indices.
    pub screen: Callback,
}

#[repr(C)]
pub struct MenuCtx {
    _p: [u8; 0],
}

#[repr(C)]
pub struct MenuArgs {
    pub world: *mut WorldCtx,
    pub menu: *mut MenuCtx,
    pub player: u64,
    /// Entity whose inventory the menu shows (block entity), 0 for the player's own.
    pub owner: u64,
    pub slot: u32,
}

// ---------------------------------------------------------------- mod UI

#[repr(C)]
pub struct UiCtx {
    _p: [u8; 0],
}

/// Client-side game view for HUD bindings.
#[repr(C)]
pub struct ClientCtx {
    _p: [u8; 0],
}

/// Item prop: the item shows on an entity model while worn. Bytes are `Worn` followed by the
/// UTF-8 name of a texture laid out like the wearer's skin.
pub const WORN: &str = "core:worn";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Worn {
    /// Bit `1 << role` for every model part role it covers, see `worn_parts`.
    pub roles: u32,
    /// Model pixels added on every side, so layers sit above the skin and each other.
    pub inflate: f32,
}

pub mod worn_parts {
    use super::part_role as r;
    pub const HEAD: u32 = 1 << r::HEAD;
    pub const BODY: u32 = 1 << r::STATIC;
    pub const ARMS: u32 = 1 << r::ARM_LEFT | 1 << r::ARM_RIGHT | 1 << r::ARM_FORWARD_LEFT | 1 << r::ARM_FORWARD_RIGHT;
    pub const LEGS: u32 = 1 << r::LEG_LEFT | 1 << r::LEG_RIGHT | 1 << r::LEG_FRONT_LEFT | 1 << r::LEG_FRONT_RIGHT;
}

pub mod ui_kind {
    pub const CONTAINER: u32 = 0;
    pub const PANEL: u32 = 1;
    pub const TEXT: u32 = 2;
    pub const BUTTON: u32 = 3;
    /// Menu slot `slot`.
    pub const SLOT: u32 = 4;
    /// Bound value 0..1: `bind`, or menu data `data / data_max` (max index, or `max` constant).
    pub const PROGRESS: u32 = 5;
    pub const IMAGE: u32 = 6;
    pub const SPACER: u32 = 7;
    /// Row of `count` icons (full/half/empty textures from the theme `class`) driven by `bind`;
    /// `data` 1 fills from the right.
    pub const ICONS: u32 = 8;
    /// The local player's model with what it wears, turning towards the mouse.
    pub const ENTITY: u32 = 9;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UiArg {
    /// 0 text, 1 translation key, 2 integer, 3 float, 4 item id.
    pub kind: u32,
    pub text: Str,
    pub num: f64,
}

#[repr(C)]
pub struct UiNode {
    pub kind: u32,
    pub class: Str,
    /// Translation key (or texture for IMAGE).
    pub text: Str,
    pub args: Slice<UiArg>,
    /// NaN = automatic.
    pub width: f32,
    pub height: f32,
    pub grow: f32,
    pub gap: f32,
    pub pad: [f32; 4],
    pub row: u32,
    /// 0 start, 1 center, 2 end, 3 stretch.
    pub align: u32,
    /// 0 start, 1 center, 2 end, 3 space between.
    pub justify: u32,
    pub slot: u32,
    pub data: u32,
    pub data_max: u32,
    pub max: f32,
    pub count: u32,
    /// `UiBindArgs`: fills `value` (and text via `Host::ui_text`) every frame.
    pub bind: Callback,
    pub on_click: Callback,
    pub tooltip: Str,
}

#[repr(C)]
pub struct UiArgs {
    pub ui: *mut UiCtx,
    pub client: *mut ClientCtx,
}

#[repr(C)]
pub struct UiBindArgs {
    pub client: *mut ClientCtx,
    /// Out: numeric value.
    pub value: f32,
    /// Pass to `Host::ui_text` to set text.
    pub text: *mut UiCtx,
}

// ---------------------------------------------------------------- host

pub mod level {
    pub const ERROR: u32 = 1;
    pub const WARN: u32 = 2;
    pub const INFO: u32 = 3;
    pub const DEBUG: u32 = 4;
}

#[repr(C)]
pub struct Host {
    /// `size_of::<Host>()` of the engine build, see crate docs.
    pub size: usize,
    pub log: unsafe extern "C" fn(me: ModId, level: u32, msg: Str),

    pub reg_block: unsafe extern "C" fn(reg: *mut Registrar, desc: *const BlockDesc),
    pub reg_item: unsafe extern "C" fn(reg: *mut Registrar, desc: *const ItemDesc),
    pub reg_biome: unsafe extern "C" fn(reg: *mut Registrar, desc: *const BiomeDesc),
    pub reg_dimension: unsafe extern "C" fn(reg: *mut Registrar, desc: *const DimensionDesc),

    pub get_block: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [i32; 3]) -> u32,
    /// Returns 0 if the position is not loaded.
    pub set_block: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [i32; 3], state: u32) -> u32,
    pub schedule_tick: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [i32; 3], delay: u32),
    pub world_time: unsafe extern "C" fn(world: *mut WorldCtx) -> u64,
    /// Deterministic per-world random stream.
    pub random: unsafe extern "C" fn(world: *mut WorldCtx) -> u64,

    /// Looks up a frozen id by full name ("base:stone"), `u32::MAX` if missing.
    pub find_block: unsafe extern "C" fn(name: Str) -> u32,
    pub find_item: unsafe extern "C" fn(name: Str) -> u32,
    /// State id -> first state id of its block.
    pub block_of: unsafe extern "C" fn(state: u32) -> u32,
    pub state_flags: unsafe extern "C" fn(state: u32) -> u32,

    pub reg_entity: unsafe extern "C" fn(reg: *mut Registrar, desc: *const EntityDesc),
    pub reg_component: unsafe extern "C" fn(reg: *mut Registrar, desc: *const ComponentDesc),
    /// Extra callbacks on an entity kind owned by someone else: hook 0 tick, 1 hurt, 2 death, 3 interact.
    pub reg_entity_hook: unsafe extern "C" fn(reg: *mut Registrar, kind: Str, hook: u32, cb: Callback),
    pub reg_recipe: unsafe extern "C" fn(reg: *mut Registrar, desc: *const RecipeDesc),
    pub reg_menu: unsafe extern "C" fn(reg: *mut Registrar, desc: *const MenuDesc),
    /// HUD part, `UiArgs`, rebuilt when the HUD is.
    pub reg_hud: unsafe extern "C" fn(reg: *mut Registrar, cb: Callback),
    /// Menu opened by the inventory key.
    pub set_player_menu: unsafe extern "C" fn(reg: *mut Registrar, menu: Str),
    /// Block entity kind created (with an inventory of `slots`) when the block is placed, removed
    /// (dropping its items) when broken.
    pub set_block_entity: unsafe extern "C" fn(reg: *mut Registrar, block: Str, entity: Str, slots: u32),

    pub light: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [i32; 3]) -> u32,
    /// 0..1 sun strength for the world's time of day.
    pub daylight: unsafe extern "C" fn(world: *mut WorldCtx) -> f32,

    pub entity_spawn: unsafe extern "C" fn(world: *mut WorldCtx, kind: u32, pos: *const [f64; 3]) -> u64,
    pub entity_remove: unsafe extern "C" fn(world: *mut WorldCtx, id: u64),
    pub entity_get: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, out: *mut EntityState) -> u32,
    /// Writes pos, vel, yaw, pitch and health from `state`.
    pub entity_set: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, state: *const EntityState),
    /// AI steering: horizontal direction and speed factor applied by the physics, optional jump.
    pub entity_walk: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, dir: *const [f32; 2], speed: f32, jump: u32),
    pub entity_damage: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, amount: f32, source: u64, cause: u32) -> u32,
    /// Fills `out` with ids near `pos` (all kinds if `kind` is u32::MAX), returns how many matched.
    pub entities_near: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [f64; 3], radius: f64, kind: u32, out: *mut u64, max: usize) -> usize,
    pub component: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, comp: u32) -> *mut u8,
    pub player_count: unsafe extern "C" fn(world: *mut WorldCtx) -> u32,
    pub player_at: unsafe extern "C" fn(world: *mut WorldCtx, index: u32) -> u64,

    pub inv_size: unsafe extern "C" fn(world: *mut WorldCtx, owner: u64) -> u32,
    pub inv_get: unsafe extern "C" fn(world: *mut WorldCtx, owner: u64, slot: u32) -> ItemStack,
    pub inv_set: unsafe extern "C" fn(world: *mut WorldCtx, owner: u64, slot: u32, stack: ItemStack),
    /// Merges into existing stacks first; returns what did not fit.
    pub inv_insert: unsafe extern "C" fn(world: *mut WorldCtx, owner: u64, stack: ItemStack) -> ItemStack,
    pub selected_slot: unsafe extern "C" fn(world: *mut WorldCtx, player: u64) -> u32,
    pub spawn_item: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [f64; 3], stack: ItemStack),
    pub block_entity: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [i32; 3]) -> u64,
    pub open_menu: unsafe extern "C" fn(world: *mut WorldCtx, player: u64, menu: u32, owner: u64),

    /// During `MenuDesc::setup`: add `count` slots of `owner`'s inventory (0 = the menu's temp slots,
    /// u64::MAX = the player) starting at `start`.
    pub menu_slots: unsafe extern "C" fn(menu: *mut MenuCtx, owner: u64, start: u32, count: u32, flags: u32),
    pub menu_get: unsafe extern "C" fn(menu: *mut MenuCtx, slot: u32) -> ItemStack,
    pub menu_set: unsafe extern "C" fn(menu: *mut MenuCtx, slot: u32, stack: ItemStack),
    pub menu_set_data: unsafe extern "C" fn(menu: *mut MenuCtx, index: u32, value: i32),

    /// Matches a grid against recipes of `kind`; returns the output stack (empty if none).
    pub recipe_match: unsafe extern "C" fn(kind: Str, grid: *const ItemStack, width: u32, height: u32) -> ItemStack,
    /// Processing recipe for one input item: output stack, time written to `time_out`.
    pub recipe_process: unsafe extern "C" fn(kind: Str, input: u32, time_out: *mut u32) -> ItemStack,
    /// Typed per-item property bytes registered with `prop`, null if absent.
    pub item_prop: unsafe extern "C" fn(item: u32, key: Str, len: *mut usize) -> *const u8,
    /// Returns the placed block state (0 if none).
    pub item_info: unsafe extern "C" fn(item: u32, max_stack: *mut u32, durability: *mut u32) -> u32,
    pub item_has_tag: unsafe extern "C" fn(item: u32, tag: Str) -> u32,

    pub ui_open: unsafe extern "C" fn(ui: *mut UiCtx, node: *const UiNode),
    pub ui_close: unsafe extern "C" fn(ui: *mut UiCtx),
    /// From a text binding: set the translation key and args shown.
    pub ui_text: unsafe extern "C" fn(ui: *mut UiCtx, key: Str, args: Slice<UiArg>),
    pub client_player: unsafe extern "C" fn(client: *mut ClientCtx) -> u64,
    /// Synced component of an entity the client knows (own player always), null if unknown.
    pub client_component: unsafe extern "C" fn(client: *mut ClientCtx, id: u64, comp: u32) -> *const u8,
    pub client_entity: unsafe extern "C" fn(client: *mut ClientCtx, id: u64, out: *mut EntityState) -> u32,
    pub client_menu_data: unsafe extern "C" fn(client: *mut ClientCtx, index: u32) -> i32,

    /// Component id by full name ("core:energy"), u32::MAX if missing.
    pub find_component: unsafe extern "C" fn(name: Str) -> u32,
    /// Energy contract (`core:energy` component): returns how much was (or would be, when
    /// `simulate` is non-zero) accepted / taken, respecting capacity and rate limits.
    pub energy_insert: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, amount: i64, simulate: u32) -> i64,
    pub energy_extract: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, amount: i64, simulate: u32) -> i64,
    /// Gives every entity of `kind` the component `component` (any mod's or the engine's) at spawn.
    pub reg_attach: unsafe extern "C" fn(reg: *mut Registrar, kind: Str, component: Str),
    /// HUD element `name` placed at `anchor` (see `hud_anchor`), `order` 0 nearest the anchor's
    /// edge. Players can move any element to another anchor in `config/hud.txt`.
    pub reg_hud_element: unsafe extern "C" fn(reg: *mut Registrar, name: Str, anchor: Str, order: i32, cb: Callback),
    /// Chat command `/name`, arguments described by `usage` (see the engine's command syntax);
    /// `cb` gets `CommandArgs` with one value per named parameter.
    pub reg_command: unsafe extern "C" fn(reg: *mut Registrar, name: Str, usage: Str, cb: Callback),
    /// Text shown to the player who ran the command.
    pub command_reply: unsafe extern "C" fn(args: *mut CommandArgs, text: Str),
    /// Non-zero for a player in creative mode (no damage, hunger or item use costs).
    pub is_creative: unsafe extern "C" fn(world: *mut WorldCtx, entity: u64) -> u32,
    /// Non-zero when the local player is in creative mode (client side, for HUDs and screens).
    pub client_creative: unsafe extern "C" fn(client: *mut ClientCtx) -> u32,
    pub reg_ore: unsafe extern "C" fn(reg: *mut Registrar, desc: *const OreDesc),
    /// Creative inventory tab shown with `icon` (an item name); items join it with the tag
    /// `tab/<name>`. Tabs appear in mod load order, then registration order.
    pub reg_creative_tab: unsafe extern "C" fn(reg: *mut Registrar, name: Str, icon: Str),
}

/// Ore veins a mod adds to any dimension, its own or another mod's. The engine scatters them after
/// the dimension's generator, so generators never need to know about each other's ores.
#[repr(C)]
pub struct OreDesc {
    /// Full dimension name.
    pub dimension: Str,
    pub block: Str,
    /// Space separated block names or `#tags` the ore may replace.
    pub replace: Str,
    pub min_y: i32,
    pub max_y: i32,
    /// Veins started per cube.
    pub veins: u32,
    /// Blocks per vein.
    pub size: u32,
}

/// One command argument; kinds in `command_value`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CommandValue {
    pub kind: u32,
    pub int: i64,
    pub float: f64,
    /// Text, or the full registry name for ids.
    pub text: Str,
    pub pos: [f64; 3],
}

pub mod command_value {
    /// Optional parameter left out.
    pub const NONE: u32 = 0;
    pub const INT: u32 = 1;
    pub const FLOAT: u32 = 2;
    pub const TEXT: u32 = 3;
    pub const POS: u32 = 4;
    /// `int` is the registry id (item, block state, entity kind, dimension), `text` its name.
    pub const ID: u32 = 5;
}

#[repr(C)]
pub struct CommandArgs {
    pub world: *mut WorldCtx,
    /// Entity id of the player who ran it.
    pub player: u64,
    pub values: *const CommandValue,
    pub count: u32,
    /// Engine side reply buffer, written through `Host::command_reply`.
    pub reply: *mut core::ffi::c_void,
}

/// Places for HUD elements. `HOTBAR_LEFT`/`HOTBAR_RIGHT` stack upwards above the hotbar's halves.
pub mod hud_anchor {
    pub const TOP_LEFT: &str = "top_left";
    pub const TOP_RIGHT: &str = "top_right";
    pub const BOTTOM_LEFT: &str = "bottom_left";
    pub const BOTTOM_RIGHT: &str = "bottom_right";
    pub const HOTBAR: &str = "hotbar";
    pub const HOTBAR_LEFT: &str = "hotbar_left";
    pub const HOTBAR_RIGHT: &str = "hotbar_right";
}

/// Layout of the engine's `core:energy` component. Blocks tagged `core:conductor` connect the
/// block entities next to them into a network the engine balances every tick.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Energy {
    pub stored: i64,
    pub capacity: i64,
    /// Rate limits per tick; a network pulls from entities with `max_out > 0` and feeds entities
    /// with `max_in > 0`.
    pub max_in: i64,
    pub max_out: i64,
}
