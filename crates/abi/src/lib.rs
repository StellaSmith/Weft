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
    /// Target block if any (`has_target`).
    pub pos: [i32; 3],
    pub face: u32,
    pub has_target: u32,
    pub handled: u32,
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
    pub drop_item: unsafe extern "C" fn(world: *mut WorldCtx, pos: *const [f32; 3], item: u32, count: u32, damage: u32),

    /// Looks up a frozen id by full name ("base:stone"), `u32::MAX` if missing.
    pub find_block: unsafe extern "C" fn(name: Str) -> u32,
    pub find_item: unsafe extern "C" fn(name: Str) -> u32,
    /// State id -> first state id of its block.
    pub block_of: unsafe extern "C" fn(state: u32) -> u32,
    pub state_flags: unsafe extern "C" fn(state: u32) -> u32,
}
