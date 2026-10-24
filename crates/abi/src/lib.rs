//! The stable boundary between the engine and mods. Everything here is `repr(C)` so a mod built
//! with any rustc (or another language) keeps working. Rules:
//! - [`Host`] only grows at the end, `size` tells a mod how much of it exists.
//! - Every descriptor a mod hands to the engine starts with `size: u32`, the struct size the mod
//!   was built with. Descriptors only grow at the end; the engine reads what the mod has and uses
//!   the `DEFAULT` of the fields it does not know. Structs passed in slices carry their size too,
//!   so the engine walks them with the mod's stride.
//! - Structs the engine writes into mod memory (`EntityState`) start with `size` as well.
//! - Callback args only grow at the end; older mods read the part they know.
//! - Descriptor structs passed to the host are read during the call only.
//! - Strings are UTF-8 `(ptr, len)`, never NUL terminated.
#![no_std]

use core::ffi::c_void;
use core::sync::atomic::AtomicU32;

/// Version 2: sized descriptors, components instead of props and fixed item fields.
pub const ABI_VERSION: u32 = 2;
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

/// A descriptor that starts with its own size (see the crate docs).
///
/// # Safety
/// The type must be `repr(C)` with `size: u32` as its first field.
pub unsafe trait Versioned: Copy {
    const DEFAULT: Self;
}

/// Reads a sized descriptor written by a mod built against any older or equal ABI: the bytes it
/// has over the defaults of the rest.
///
/// # Safety
/// `p` must point to a descriptor of type `T` (of the mod's version).
pub unsafe fn read_sized<T: Versioned>(p: *const T) -> T {
    let mut out = T::DEFAULT;
    unsafe {
        let theirs = *(p as *const u32) as usize;
        let n = theirs.min(size_of::<T>());
        core::ptr::copy_nonoverlapping(p as *const u8, &mut out as *mut T as *mut u8, n);
        *(&mut out as *mut T as *mut u32) = size_of::<T>() as u32;
    }
    out
}

/// Calls `f` with every element of a slice of sized descriptors, stepping by the mod's size.
///
/// # Safety
/// `s` must hold `len` descriptors of type `T` laid out with the size their first one names.
pub unsafe fn each_sized<T: Versioned>(s: Slice<T>, mut f: impl FnMut(T)) {
    if s.len == 0 {
        return;
    }
    unsafe {
        let stride = (*(s.ptr as *const u32) as usize).max(4);
        for i in 0..s.len {
            f(read_sized((s.ptr as *const u8).add(i * stride) as *const T));
        }
    }
}

/// Writes a struct into memory a mod owns, at most the size it says it has.
///
/// # Safety
/// `out` must point to a `T` of the mod's version whose first field is its size.
pub unsafe fn write_sized<T: Versioned>(out: *mut T, value: &T) {
    unsafe {
        let theirs = *(out as *const u32) as usize;
        let n = theirs.min(size_of::<T>());
        core::ptr::copy_nonoverlapping((value as *const T as *const u8).add(4), (out as *mut u8).add(4), n.saturating_sub(4));
    }
}

macro_rules! versioned {
    ($t:ident { $($f:ident: $v:expr),* $(,)? }) => {
        unsafe impl Versioned for $t {
            const DEFAULT: Self = $t { size: size_of::<$t>() as u32, $($f: $v),* };
        }
    };
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

// ---------------------------------------------------------------- components

/// One component value on a content entry. `schema` names the value type (see the SDK's `Value`):
/// empty for a tag, `u32`, `f32`, `str` (a `Str`, copied by the engine), `{a:u32,b:f32}` for
/// structs. Every use of a name must agree on the schema; the engine checks at freeze.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CompValue {
    pub name: Str,
    pub schema: Str,
    pub value: Slice<u8>,
}

/// Registry kinds for `Host::find`; the first `COUNT` also have component tables (blocks per state).
pub mod entry_kind {
    pub const BLOCK: u32 = 0;
    pub const ITEM: u32 = 1;
    pub const ENTITY: u32 = 2;
    pub const BIOME: u32 = 3;
    pub const DIMENSION: u32 = 4;
    pub const MENU: u32 = 5;
    pub const COUNT: usize = 5;
}

/// What a `SetDesc` applies to.
pub mod set_target {
    /// The block (all states, or one with `name#3`) and the item of that name, whichever exist.
    pub const NAME: u32 = 0;
    /// Blocks and items carrying a tag; `*` in it matches any text (`ingots/*`).
    pub const TAGGED: u32 = 1;
    pub const ENTITY: u32 = 2;
    pub const BIOME: u32 = 3;
    pub const DIMENSION: u32 = 4;
}

/// Sets (or with `remove` takes away) a component on entries any mod registered. Applied after
/// every mod registered, in mod load order; the last one wins and overrides are logged.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SetDesc {
    pub size: u32,
    pub target_kind: u32,
    pub target: Str,
    pub value: CompValue,
    pub remove: u32,
}
versioned!(SetDesc { target_kind: 0, target: Str::EMPTY, value: CompValue { name: Str::EMPTY, schema: Str::EMPTY, value: Slice::EMPTY }, remove: 0 });

/// Frozen components of one entry kind, readable directly by mods: whether entry `e` has
/// component `c` is bit `c % 64` of `bits[e * words + c / 64]`; its value (if the component has
/// one on this kind) is at `columns[c].data + e * columns[c].stride`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KindTable {
    pub entries: u32,
    pub words: u32,
    pub bits: *const u64,
    /// One per component id; `stride` 0 when no entry of this kind has a value for it.
    pub columns: *const Column,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Column {
    pub data: *const u8,
    pub stride: u32,
}

#[repr(C)]
pub struct ComponentTable {
    pub size: u32,
    /// Number of component ids.
    pub components: u32,
    pub kinds: [KindTable; entry_kind::COUNT],
}

unsafe impl Sync for ComponentTable {}
unsafe impl Send for ComponentTable {}

/// Component names the engine itself reads. Items: `max_stack: u32` (default 64), `durability: u32`,
/// `attack_damage: f32` (default 1), `tool_level: u32` with a `tools/<class>` tag and
/// `mining_speed: f32` (default 1). Blocks: `mineable/<class>` tags; `needs_tool` means nothing
/// drops without a tool of a matching class and at least `harvest_level: u32`. Stacks: `damage: u32`.
pub mod comp {
    pub const MAX_STACK: &str = "max_stack";
    pub const DURABILITY: &str = "durability";
    pub const ATTACK_DAMAGE: &str = "attack_damage";
    pub const MINING_SPEED: &str = "mining_speed";
    pub const TOOL_LEVEL: &str = "tool_level";
    pub const HARVEST_LEVEL: &str = "harvest_level";
    pub const NEEDS_TOOL: &str = "needs_tool";
    pub const DAMAGE: &str = "damage";
    /// Blocks connecting energy networks.
    pub const CONDUCTOR: &str = "conductor";
    /// Entity component, layout `Energy`.
    pub const ENERGY: &str = "energy";
    /// Item shown on the wearer's model: `Worn`, plus `worn_texture: str` in the wearer's skin layout.
    pub const WORN: &str = "worn";
    pub const WORN_TEXTURE: &str = "worn_texture";
}

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
    pub size: u32,
    /// In 1/16 block units.
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub textures: [Str; 6],
}
versioned!(ModelBox { min: [0.0; 3], max: [16.0; 3], textures: [Str::EMPTY; 6] });

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StateDesc {
    pub size: u32,
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
    /// Face (-X, +X, -Y, +Y, -Z, +Z) whose neighbour holds the block up, 255 = none. Unsupported,
    /// it cannot be placed and breaks off (dropping itself) when the support goes.
    pub support_side: u8,
    /// Block tag the supporting block needs; empty = any solid block.
    pub support: Str,
    /// Components of this state only, on top of the block's.
    pub components: Slice<CompValue>,
}
versioned!(StateDesc {
    flags: block_flags::SOLID | block_flags::OPAQUE,
    light: [0; 3],
    filter: [0; 3],
    opacity: 15,
    tint: 0,
    tint_faces: 0x3F,
    layer: Layer::Opaque,
    level: 0,
    model: Model::Cube,
    textures: [Str::EMPTY; 6],
    boxes: Slice::EMPTY,
    collision: Slice::EMPTY,
    hardness: 1.0,
    friction: 0.6,
    drag: 1.0,
    support_side: 255,
    support: Str::EMPTY,
    components: Slice::EMPTY,
});

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BlockDesc {
    pub size: u32,
    pub name: Str,
    /// Receives the id of the first state once registries are frozen (null = no handle).
    pub cell: *const AtomicU32,
    pub states: Slice<StateDesc>,
    /// Components of every state (tags, mining, fuel ...). Its item gets them too unless it sets
    /// its own.
    pub components: Slice<CompValue>,
    /// Item dropped when broken, empty = the item with the same name if it exists, `-` = none.
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
versioned!(BlockDesc {
    name: Str::EMPTY,
    cell: core::ptr::null(),
    states: Slice::EMPTY,
    components: Slice::EMPTY,
    drop: Str::EMPTY,
    drop_count: 1,
    on_use: Callback::NONE,
    on_placed: Callback::NONE,
    on_broken: Callback::NONE,
    on_random_tick: Callback::NONE,
    on_neighbor: Callback::NONE,
    on_scheduled: Callback::NONE,
    placement: Callback::NONE,
    replace: 0,
});

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ItemDesc {
    pub size: u32,
    pub name: Str,
    pub cell: *const AtomicU32,
    /// Texture for the flat icon. Empty with `block` set means a 3D block icon.
    pub texture: Str,
    /// Block placed on use, empty = none.
    pub block: Str,
    /// Stack size, durability, tool and weapon numbers, food ... (see `comp`).
    pub components: Slice<CompValue>,
    /// `ItemUseArgs`.
    pub on_use: Callback,
}
versioned!(ItemDesc { name: Str::EMPTY, cell: core::ptr::null(), texture: Str::EMPTY, block: Str::EMPTY, components: Slice::EMPTY, on_use: Callback::NONE });

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BiomeDesc {
    pub size: u32,
    pub name: Str,
    pub cell: *const AtomicU32,
    pub grass: u32,
    pub foliage: u32,
    pub water: u32,
    pub temperature: f32,
    pub rainfall: f32,
    pub components: Slice<CompValue>,
}
versioned!(BiomeDesc { name: Str::EMPTY, cell: core::ptr::null(), grass: 0x8db360, foliage: 0x77ab2f, water: 0x3f76e4, temperature: 0.8, rainfall: 0.4, components: Slice::EMPTY });

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
#[derive(Clone, Copy)]
pub struct DimensionDesc {
    pub size: u32,
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
    pub components: Slice<CompValue>,
}
versioned!(DimensionDesc {
    name: Str::EMPTY,
    cell: core::ptr::null(),
    generator: Callback::NONE,
    spawn: Callback::NONE,
    min_cube_y: i32::MIN,
    max_cube_y: i32::MAX,
    sky_cube_y: i32::MAX,
    sky_light: 1,
    gravity: 32.0,
    day_length: 24000,
    fixed_time: 6000,
    sky_color: 0x78a7ff,
    fog_color: 0xc0d8ff,
    ambient: 0,
    components: Slice::EMPTY,
});

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

/// `data` is 0 for a plain stack, otherwise a handle to its stack components (damage taken,
/// charge, contents ...). Handles are interned: two stacks with equal data have equal handles,
/// so stacks merge exactly when `item` and `data` match. Read and change data through the host.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Hash)]
pub struct ItemStack {
    pub item: u32,
    pub count: u32,
    pub data: u32,
}

impl ItemStack {
    pub const EMPTY: ItemStack = ItemStack { item: 0, count: 0, data: 0 };

    pub fn new(item: u32, count: u32) -> ItemStack {
        ItemStack { item, count, data: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.item == 0 || self.count == 0
    }

    /// Same item and data, so the two can share a slot.
    pub fn stacks_with(&self, other: &ItemStack) -> bool {
        self.item == other.item && self.data == other.data
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
    /// Not a kind flag: set in `EntityState::flags` when its last move ran into a wall.
    pub const BLOCKED: u32 = 1 << 31;
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
    pub size: u32,
    pub origin: [f32; 3],
    pub extent: [f32; 3],
    pub uv: [f32; 2],
    pub inflate: f32,
}
versioned!(ModelCuboid { origin: [0.0; 3], extent: [1.0; 3], uv: [0.0; 2], inflate: 0.0 });

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModelPart {
    pub size: u32,
    /// Pivot in model pixels, y up from the feet.
    pub pivot: [f32; 3],
    pub role: u32,
    pub cuboids: Slice<ModelCuboid>,
    /// Resting rotation in radians about x, y, z, applied x first; the role's animation adds to it.
    pub rest: [f32; 3],
}
versioned!(ModelPart { pivot: [0.0; 3], role: 0, cuboids: Slice::EMPTY, rest: [0.0; 3] });

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EntityDesc {
    pub size: u32,
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
    pub components: Slice<CompValue>,
}
versioned!(EntityDesc {
    name: Str::EMPTY,
    cell: core::ptr::null(),
    width: 0.6,
    height: 1.8,
    eye: 1.6,
    max_health: 20.0,
    speed: 3.0,
    flags: entity_flags::FALL_DAMAGE | entity_flags::PUSHABLE,
    category: 0,
    texture: Str::EMPTY,
    texture_size: [64, 32],
    parts: Slice::EMPTY,
    on_tick: Callback::NONE,
    on_hurt: Callback::NONE,
    on_death: Callback::NONE,
    on_interact: Callback::NONE,
    replace: 0,
    components: Slice::EMPTY,
});

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
#[derive(Clone, Copy, Debug)]
pub struct EntityState {
    pub size: u32,
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
versioned!(EntityState {
    kind: 0,
    flags: 0,
    pos: [0.0; 3],
    vel: [0.0; 3],
    yaw: 0.0,
    pitch: 0.0,
    on_ground: 0,
    in_liquid: 0,
    health: 0.0,
    max_health: 0.0,
    age: 0,
    fall: 0.0,
    moved: 0.0,
});

impl Default for EntityState {
    fn default() -> Self {
        Self::DEFAULT
    }
}

pub mod component_flags {
    pub const SAVE: u32 = 1;
    /// Sent to the player that owns it (HUD values like hunger).
    pub const SYNC_OWNER: u32 = 2;
}

/// Per-entity data storage (hunger, machine state): created with `default` on spawn of the kinds
/// in `attach_to`. The name lives in the same namespace as content components.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ComponentDesc {
    pub size: u32,
    pub name: Str,
    pub schema: Str,
    /// Value size in bytes.
    pub value_size: u32,
    pub flags: u32,
    /// Initial bytes, `value_size` long.
    pub default: Slice<u8>,
    /// Entity kinds ("core:player") that get this component when spawned.
    pub attach_to: Slice<Str>,
}
versioned!(ComponentDesc { name: Str::EMPTY, schema: Str::EMPTY, value_size: 0, flags: 0, default: Slice::EMPTY, attach_to: Slice::EMPTY });

/// Recipe ingredients are item names or `#tag`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RecipeKey {
    pub ch: u32,
    pub item: Str,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RecipeDesc {
    pub size: u32,
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
versioned!(RecipeDesc { kind: Str::EMPTY, pattern: Slice::EMPTY, keys: Slice::EMPTY, inputs: Slice::EMPTY, output: Str::EMPTY, count: 1, time: 0 });

pub mod slot_flags {
    /// Can only be taken from (crafting result).
    pub const OUTPUT: u32 = 1;
    pub const ARMOR_HEAD: u32 = 2;
    pub const ARMOR_CHEST: u32 = 4;
    pub const ARMOR_LEGS: u32 = 8;
    pub const ARMOR_FEET: u32 = 16;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MenuDesc {
    pub size: u32,
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
versioned!(MenuDesc {
    name: Str::EMPTY,
    cell: core::ptr::null(),
    temp_slots: 0,
    data_count: 0,
    setup: Callback::NONE,
    on_change: Callback::NONE,
    on_take: Callback::NONE,
    on_tick: Callback::NONE,
    screen: Callback::NONE,
});

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

/// Layout of the `worn` item component; see `comp::WORN`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Bound value 0..1: `bind`, or menu data `data / data_max` (max index, or `max` constant),
    /// filling in direction `dir`.
    pub const PROGRESS: u32 = 5;
    pub const IMAGE: u32 = 6;
    pub const SPACER: u32 = 7;
    /// Row of `count` icons (full/half/empty textures from the theme `class`) driven by `bind`;
    /// `data` 1 fills from the right.
    pub const ICONS: u32 = 8;
    /// The local player's model with what it wears, turning towards the mouse.
    pub const ENTITY: u32 = 9;
}

/// Fill directions of progress bars.
pub mod ui_dir {
    pub const RIGHT: u32 = 0;
    pub const UP: u32 = 1;
    pub const LEFT: u32 = 2;
    pub const DOWN: u32 = 3;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UiArg {
    pub size: u32,
    /// 0 text, 1 translation key, 2 integer, 3 float, 4 item id.
    pub kind: u32,
    pub text: Str,
    pub num: f64,
}
versioned!(UiArg { kind: 0, text: Str::EMPTY, num: 0.0 });

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UiNode {
    pub size: u32,
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
    /// `ui_dir` for progress bars.
    pub dir: u32,
}
versioned!(UiNode {
    kind: 0,
    class: Str::EMPTY,
    text: Str::EMPTY,
    args: Slice::EMPTY,
    width: f32::NAN,
    height: f32::NAN,
    grow: 0.0,
    gap: 0.0,
    pad: [0.0; 4],
    row: 0,
    align: 0,
    justify: 0,
    slot: 0,
    data: u32::MAX,
    data_max: u32::MAX,
    max: 0.0,
    count: 0,
    bind: Callback::NONE,
    on_click: Callback::NONE,
    tooltip: Str::EMPTY,
    dir: 0,
});

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
    pub const TRACE: u32 = 5;
}

#[repr(C)]
pub struct Host {
    /// `size_of::<Host>()` of the engine build, see crate docs.
    pub size: usize,
    pub log: unsafe extern "C" fn(me: ModId, level: u32, msg: Str),
    /// Like `log` under a sub-target of the mod's own: `mod:<id>::<target>` (`mod:<id>` if empty),
    /// so players can filter it with `--log`/`/log`.
    pub log_at: unsafe extern "C" fn(me: ModId, level: u32, target: Str, msg: Str),
    /// Non-zero when a record at `level` for that target would be written; check before building
    /// costly diagnostics.
    pub log_enabled: unsafe extern "C" fn(me: ModId, level: u32, target: Str) -> u32,

    pub reg_block: unsafe extern "C" fn(reg: *mut Registrar, desc: *const BlockDesc),
    pub reg_item: unsafe extern "C" fn(reg: *mut Registrar, desc: *const ItemDesc),
    pub reg_biome: unsafe extern "C" fn(reg: *mut Registrar, desc: *const BiomeDesc),
    pub reg_dimension: unsafe extern "C" fn(reg: *mut Registrar, desc: *const DimensionDesc),
    pub reg_entity: unsafe extern "C" fn(reg: *mut Registrar, desc: *const EntityDesc),
    pub reg_component: unsafe extern "C" fn(reg: *mut Registrar, desc: *const ComponentDesc),
    /// Extra callbacks on an entity kind owned by someone else: hook 1 hurt, 2 death, 3 interact
    /// (`EntityEventArgs`); tick hooks go through `reg_entity_tick_hook`.
    pub reg_entity_hook: unsafe extern "C" fn(reg: *mut Registrar, kind: Str, hook: u32, cb: Callback),
    /// `EntityTickArgs` with every entity of `kind` once per tick.
    pub reg_entity_tick_hook: unsafe extern "C" fn(reg: *mut Registrar, kind: Str, cb: Callback),
    pub reg_recipe: unsafe extern "C" fn(reg: *mut Registrar, desc: *const RecipeDesc),
    pub reg_menu: unsafe extern "C" fn(reg: *mut Registrar, desc: *const MenuDesc),
    /// Menu opened by the inventory key.
    pub set_player_menu: unsafe extern "C" fn(reg: *mut Registrar, menu: Str),
    /// Block entity kind created (with an inventory of `slots`) when the block is placed, removed
    /// (dropping its items) when broken.
    pub set_block_entity: unsafe extern "C" fn(reg: *mut Registrar, block: Str, entity: Str, slots: u32),
    /// Gives every entity of `kind` the component `component` (any mod's or the engine's) at spawn.
    pub reg_attach: unsafe extern "C" fn(reg: *mut Registrar, kind: Str, component: Str),
    /// HUD element `name` placed at `anchor` (see `hud_anchor`), `order` 0 nearest the anchor's
    /// edge. Players can move any element to another anchor in `config/hud.txt`.
    pub reg_hud_element: unsafe extern "C" fn(reg: *mut Registrar, name: Str, anchor: Str, order: i32, cb: Callback),
    /// Chat command `/name`, arguments described by `usage` (see the engine's command syntax);
    /// `cb` gets `CommandArgs` with one value per named parameter.
    pub reg_command: unsafe extern "C" fn(reg: *mut Registrar, name: Str, usage: Str, cb: Callback),
    pub reg_ore: unsafe extern "C" fn(reg: *mut Registrar, desc: *const OreDesc),
    /// Creative inventory tab shown with `icon` (an item name); items join it with the tag
    /// `tab/<name>`. Tabs appear in mod load order, then registration order.
    pub reg_creative_tab: unsafe extern "C" fn(reg: *mut Registrar, name: Str, icon: Str),
    /// Sets or removes a component (or tag) on entries of any mod, see `SetDesc`.
    pub reg_set: unsafe extern "C" fn(reg: *mut Registrar, desc: *const SetDesc),

    /// Looks up a frozen id by full name ("base:stone"), `u32::MAX` if missing. Blocks give their
    /// first state and write the state count to `count` (others write 1). `kind` is `entry_kind`.
    pub find: unsafe extern "C" fn(kind: u32, name: Str, count: *mut u32) -> u32,
    /// State id -> first state id of its block.
    pub block_of: unsafe extern "C" fn(state: u32) -> u32,
    pub state_flags: unsafe extern "C" fn(state: u32) -> u32,
    /// Component id by name, `u32::MAX` if nothing uses it. A non-empty `schema` that differs from
    /// the registered one is an error (logged) and also gives `u32::MAX`.
    pub find_component: unsafe extern "C" fn(name: Str, schema: Str) -> u32,
    /// The frozen component tables (null before freeze); stable for the whole session.
    pub components: unsafe extern "C" fn() -> *const ComponentTable,
    /// Value bytes of stack component `comp` on stack data `data`, null if absent.
    pub stack_get: unsafe extern "C" fn(data: u32, comp: u32, len: *mut usize) -> *const u8,
    /// Stack data with `comp` set to `value` (or removed when `remove` is non-zero).
    pub stack_with: unsafe extern "C" fn(data: u32, comp: u32, value: Slice<u8>, remove: u32) -> u32,

    pub get_block: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [i32; 3]) -> u32,
    /// Returns 0 if the position is not loaded.
    pub set_block: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [i32; 3], state: u32) -> u32,
    pub schedule_tick: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [i32; 3], delay: u32),
    pub world_time: unsafe extern "C" fn(world: *mut WorldCtx) -> u64,
    /// Deterministic per-world random stream.
    pub random: unsafe extern "C" fn(world: *mut WorldCtx) -> u64,
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
    /// The block an entity looks at from its eyes within `max` blocks, liquids too if `liquids`:
    /// its state + 1 (0 = nothing), position and face hit.
    pub entity_look_at: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, max: f64, liquids: u32, pos: *mut [i32; 3], face: *mut u32) -> u32,
    /// Fills `out` with ids near `pos` (all kinds if `kind` is u32::MAX), returns how many matched.
    pub entities_near: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [f64; 3], radius: f64, kind: u32, out: *mut u64, max: usize) -> usize,
    /// An entity's component bytes, null if it has none. `write` non-zero marks it changed (sent
    /// to its owner, saved); reads pass 0, or every read would be synced.
    pub component: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, comp: u32, write: u32) -> *mut u8,
    pub player_count: unsafe extern "C" fn(world: *mut WorldCtx) -> u32,
    pub player_at: unsafe extern "C" fn(world: *mut WorldCtx, index: u32) -> u64,
    /// Non-zero for a player in creative mode (no damage, hunger or item use costs).
    pub is_creative: unsafe extern "C" fn(world: *mut WorldCtx, entity: u64) -> u32,

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
    pub menu_set_data: unsafe extern "C" fn(menu: *mut MenuCtx, index: u32, value: i64),

    /// Matches a grid against recipes of `kind`; returns the output stack (empty if none).
    pub recipe_match: unsafe extern "C" fn(kind: Str, grid: *const ItemStack, width: u32, height: u32) -> ItemStack,
    /// Processing recipe for one input item: output stack, time written to `time_out`.
    pub recipe_process: unsafe extern "C" fn(kind: Str, input: u32, time_out: *mut u32) -> ItemStack,
    /// The block state an item places, 0 if none.
    pub item_block: unsafe extern "C" fn(item: u32) -> u32,

    pub ui_open: unsafe extern "C" fn(ui: *mut UiCtx, node: *const UiNode),
    pub ui_close: unsafe extern "C" fn(ui: *mut UiCtx),
    /// From a text binding: set the translation key and args shown.
    pub ui_text: unsafe extern "C" fn(ui: *mut UiCtx, key: Str, args: Slice<UiArg>),
    pub client_player: unsafe extern "C" fn(client: *mut ClientCtx) -> u64,
    /// Synced component of an entity the client knows (own player always), null if unknown.
    pub client_component: unsafe extern "C" fn(client: *mut ClientCtx, id: u64, comp: u32) -> *const u8,
    pub client_entity: unsafe extern "C" fn(client: *mut ClientCtx, id: u64, out: *mut EntityState) -> u32,
    pub client_menu_data: unsafe extern "C" fn(client: *mut ClientCtx, index: u32) -> i64,
    /// Non-zero when the local player is in creative mode (client side, for HUDs and screens).
    pub client_creative: unsafe extern "C" fn(client: *mut ClientCtx) -> u32,

    /// Energy contract (`energy` component): returns how much was (or would be, when `simulate`
    /// is non-zero) accepted / taken, respecting capacity and rate limits.
    pub energy_insert: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, amount: i64, simulate: u32) -> i64,
    pub energy_extract: unsafe extern "C" fn(world: *mut WorldCtx, id: u64, amount: i64, simulate: u32) -> i64,
    /// Text shown to the player who ran the command.
    pub command_reply: unsafe extern "C" fn(args: *mut CommandArgs, text: Str),
    /// Non-zero when a player in this world shows gizmo group `group` (`/gizmo <group>`).
    pub gizmo_on: unsafe extern "C" fn(world: *mut WorldCtx, group: Str) -> u32,
    /// Debug shape for players showing `group`: `kind` 0 line from `a` to `b`, 1 box between
    /// corners `a` and `b`, 2 `text` at `a`; `color` is 0xRRGGBBAA, kept for `ticks`.
    pub gizmo: unsafe extern "C" fn(world: *mut WorldCtx, group: Str, kind: u32, a: *const [f64; 3], b: *const [f64; 3], color: u32, ticks: u32, text: Str),
}

/// Ore veins a mod adds to any dimension, its own or another mod's. The engine scatters them after
/// the dimension's generator, so generators never need to know about each other's ores.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OreDesc {
    pub size: u32,
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
    pub blocks: u32,
}
versioned!(OreDesc { dimension: Str::EMPTY, block: Str::EMPTY, replace: Str::EMPTY, min_y: i32::MIN, max_y: i32::MAX, veins: 4, blocks: 8 });

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
    /// Selected entities: `text` holds their ids separated by spaces, `int` how many.
    pub const TARGETS: u32 = 6;
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

/// Layout of the engine's `energy` entity component. Blocks tagged `conductor` connect the
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_descriptors_get_defaults() {
        // A mod built when OreDesc ended after `max_y` (veins and blocks unknown to it).
        let mut d = OreDesc::DEFAULT;
        d.min_y = -5;
        d.veins = 99;
        d.size = (core::mem::offset_of!(OreDesc, max_y) + 4) as u32;
        let read = unsafe { read_sized(&d) };
        assert_eq!((read.min_y, read.veins, read.blocks, read.size), (-5, 4, 8, size_of::<OreDesc>() as u32));
    }

    #[test]
    fn slices_step_by_the_mods_size() {
        // An older UiArg without `num`.
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Old {
            size: u32,
            kind: u32,
            text: Str,
        }
        let n = size_of::<Old>() as u32;
        let old = [Old { size: n, kind: 2, text: Str::EMPTY }, Old { size: n, kind: 3, text: Str::EMPTY }];
        let mut seen = [(0u32, 1.0f64); 2];
        let mut i = 0;
        unsafe {
            each_sized(Slice { ptr: old.as_ptr() as *const UiArg, len: 2 }, |a| {
                seen[i] = (a.kind, a.num);
                i += 1;
            })
        };
        assert_eq!(seen, [(2, 0.0), (3, 0.0)]);
    }
}
