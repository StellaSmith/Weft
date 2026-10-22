//! Engine side of the mod ABI: the function table mods call, and the wrapper the engine uses to
//! call mod callbacks (profiling and fault isolation live there).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use voxide_abi::{self as abi, Callback, Host, ModId, Str};

use crate::content::{self, RawRegistrar, content, try_content};

pub static HOST: Host = Host {
    size: size_of::<Host>(),
    log: host_log,
    reg_block,
    reg_item,
    reg_biome,
    reg_dimension,
    get_block,
    set_block,
    schedule_tick,
    world_time,
    random,
    drop_item,
    find_block,
    find_item,
    block_of,
    state_flags,
};

// ---------------------------------------------------------------- profiling per mod

pub const MAX_MODS: usize = 256;

pub struct ModStats {
    pub nanos: AtomicU64,
    pub calls: AtomicU64,
    pub faults: AtomicU32,
}

static STATS: [ModStats; MAX_MODS] =
    [const { ModStats { nanos: AtomicU64::new(0), calls: AtomicU64::new(0), faults: AtomicU32::new(0) } }; MAX_MODS];

pub fn stats(m: ModId) -> &'static ModStats {
    &STATS[(m.0 as usize).min(MAX_MODS - 1)]
}

/// Calls a mod callback. Time is charged to `owner`; a failing callback is logged with the mod's
/// name and the engine carries on.
pub fn invoke<A>(owner: ModId, cb: Callback, args: &mut A) -> bool {
    let Some(f) = cb.func else { return true };
    let start = Instant::now();
    let r = unsafe { f(cb.data, args as *mut A as *mut c_void) };
    let s = stats(owner);
    s.nanos.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    s.calls.fetch_add(1, Ordering::Relaxed);
    if r != 0 {
        let n = s.faults.fetch_add(1, Ordering::Relaxed);
        if n < 10 {
            let name = try_content().map_or("?".to_string(), |c| c.mod_name(owner).to_string());
            log::error!("callback of mod '{name}' failed (see its panic above), continuing");
        }
        return false;
    }
    true
}

// ---------------------------------------------------------------- world access from callbacks

/// What a callback can do to the world it was called from.
pub trait WorldAccess {
    fn get(&mut self, pos: [i32; 3]) -> u32;
    fn set(&mut self, pos: [i32; 3], state: u32) -> bool;
    fn schedule(&mut self, pos: [i32; 3], delay: u32);
    fn time(&mut self) -> u64;
    fn random(&mut self) -> u64;
    fn drop_item(&mut self, pos: [f32; 3], item: u32, count: u32, damage: u32);
}

/// `WorldCtx` pointers handed to mods point at one of these.
pub struct CallCtx<'a> {
    pub world: &'a mut dyn WorldAccess,
}

impl<'a> CallCtx<'a> {
    pub fn raw(&mut self) -> *mut abi::WorldCtx {
        self as *mut CallCtx as *mut abi::WorldCtx
    }
}

unsafe fn ctx<'a>(w: *mut abi::WorldCtx) -> &'a mut CallCtx<'a> {
    unsafe { &mut *(w as *mut CallCtx) }
}

unsafe extern "C" fn get_block(w: *mut abi::WorldCtx, pos: *const [i32; 3]) -> u32 {
    unsafe { ctx(w).world.get(*pos) }
}

unsafe extern "C" fn set_block(w: *mut abi::WorldCtx, pos: *const [i32; 3], state: u32) -> u32 {
    if state as usize >= content().states.len() {
        log::error!("mod tried to set invalid state {state}");
        return 0;
    }
    unsafe { ctx(w).world.set(*pos, state) as u32 }
}

unsafe extern "C" fn schedule_tick(w: *mut abi::WorldCtx, pos: *const [i32; 3], delay: u32) {
    unsafe { ctx(w).world.schedule(*pos, delay) }
}

unsafe extern "C" fn world_time(w: *mut abi::WorldCtx) -> u64 {
    unsafe { ctx(w).world.time() }
}

unsafe extern "C" fn random(w: *mut abi::WorldCtx) -> u64 {
    unsafe { ctx(w).world.random() }
}

unsafe extern "C" fn drop_item(w: *mut abi::WorldCtx, pos: *const [f32; 3], item: u32, count: u32, damage: u32) {
    unsafe { ctx(w).world.drop_item(*pos, item, count, damage) }
}

// ---------------------------------------------------------------- lookups

unsafe extern "C" fn find_block(name: Str) -> u32 {
    let name = unsafe { name.as_str() };
    try_content().and_then(|c| c.find_state(name)).unwrap_or(u32::MAX)
}

unsafe extern "C" fn find_item(name: Str) -> u32 {
    let name = unsafe { name.as_str() };
    try_content().and_then(|c| c.item_names.get(name).copied()).unwrap_or(u32::MAX)
}

unsafe extern "C" fn block_of(state: u32) -> u32 {
    let c = content();
    c.states.get(state as usize).map_or(0, |s| c.blocks[s.block as usize].first_state)
}

unsafe extern "C" fn state_flags(state: u32) -> u32 {
    content().states.get(state as usize).map_or(0, |s| s.flags)
}

// ---------------------------------------------------------------- registration

unsafe extern "C" fn host_log(me: ModId, level: u32, msg: Str) {
    let msg = unsafe { msg.as_str() };
    let name = crate::modloader::mod_id_name(me);
    let target = format!("mod:{name}");
    let lvl = match level {
        abi::level::ERROR => log::Level::Error,
        abi::level::WARN => log::Level::Warn,
        abi::level::DEBUG => log::Level::Debug,
        _ => log::Level::Info,
    };
    log::log!(target: &target, lvl, "{msg}");
}

unsafe fn reg<'a>(r: *mut abi::Registrar) -> &'a mut RawRegistrar {
    unsafe { &mut *(r as *mut RawRegistrar) }
}

unsafe extern "C" fn reg_block(r: *mut abi::Registrar, d: *const abi::BlockDesc) {
    unsafe {
        let b = content::copy_block(&*d);
        let r = reg(r);
        if b.states.is_empty() {
            r.errors.push(format!("block {} has no states", b.name));
            return;
        }
        r.blocks.push(b);
    }
}

unsafe extern "C" fn reg_item(r: *mut abi::Registrar, d: *const abi::ItemDesc) {
    unsafe { reg(r).items.push(content::copy_item(&*d)) }
}

unsafe extern "C" fn reg_biome(r: *mut abi::Registrar, d: *const abi::BiomeDesc) {
    unsafe {
        let d = &*d;
        reg(r).biomes.push((
            content::BiomeDef {
                name: d.name.as_str().to_string(),
                grass: d.grass,
                foliage: d.foliage,
                water: d.water,
                temperature: d.temperature,
                rainfall: d.rainfall,
            },
            d.cell as usize,
        ));
    }
}

unsafe extern "C" fn reg_dimension(r: *mut abi::Registrar, d: *const abi::DimensionDesc) {
    unsafe {
        let r = reg(r);
        let owner = r.owner;
        r.dims.push(content::copy_dimension(owner, &*d));
    }
}
