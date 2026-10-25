//! Safe Rust API for writing Voxide mods. The base game uses exactly this crate.

pub use voxide_abi as abi;

pub mod ai;
mod cache;
pub mod client;
mod component;
pub use voxide_noise as noise;
pub mod terrain;
mod command;
mod config;
mod content;
mod effects;
mod extend;
pub mod facing;
mod game;
mod ui;
mod world;

pub use abi::{ItemStack, hud_anchor, tool_tier, worn_parts};
pub use cache::ColumnCache;
pub use client::{Client, Packet, launch_option};
pub use command::Command;
pub use config::*;
pub use component::*;
pub use content::*;
pub use effects::*;
pub use extend::*;
pub use facing::{Dir, Facings, Toward};
pub use game::*;
pub use ui::*;
pub use world::*;

use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use abi::{Callback, Host, ModId, Str};

static HOST: AtomicPtr<Host> = AtomicPtr::new(std::ptr::null_mut());
static ME: AtomicU32 = AtomicU32::new(u32::MAX);
static MOD_ID: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

pub fn host() -> &'static Host {
    let p = HOST.load(Ordering::Relaxed);
    assert!(!p.is_null(), "voxide host used before mod init");
    unsafe { &*p }
}

pub fn mod_id() -> &'static str {
    MOD_ID.get().copied().unwrap_or("?")
}

fn me() -> ModId {
    ModId(ME.load(Ordering::Relaxed))
}

#[doc(hidden)]
pub fn __log(level: u32, target: &str, args: std::fmt::Arguments) {
    let msg = std::fmt::format(args);
    unsafe { (host().log_at)(me(), level, Str::new(target), Str::new(&msg)) }
}

/// Whether a record at `level` (`abi::level::*`) under this mod's `target` (`""` for the mod
/// itself) would be written: players switch these with `--log mod:<id>::<target>=debug`.
pub fn log_enabled(level: u32, target: &str) -> bool {
    unsafe { (host().log_enabled)(me(), level, Str::new(target)) != 0 }
}

/// `info!("..")` logs as `mod:<id>`, `info!(target: "pipes", "..")` as `mod:<id>::pipes`; the
/// message is only formatted when that target is enabled at that level.
#[macro_export]
macro_rules! __log_macro {
    ($lvl:expr, target: $t:expr, $($a:tt)*) => {
        if $lvl <= $crate::abi::level::INFO || $crate::log_enabled($lvl, $t) { $crate::__log($lvl, $t, format_args!($($a)*)) }
    };
    ($lvl:expr, $($a:tt)*) => { $crate::__log_macro!($lvl, target: "", $($a)*) };
}
#[macro_export]
macro_rules! error { ($($t:tt)*) => { $crate::__log_macro!($crate::abi::level::ERROR, $($t)*) } }
#[macro_export]
macro_rules! warn { ($($t:tt)*) => { $crate::__log_macro!($crate::abi::level::WARN, $($t)*) } }
#[macro_export]
macro_rules! info { ($($t:tt)*) => { $crate::__log_macro!($crate::abi::level::INFO, $($t)*) } }
#[macro_export]
macro_rules! debug { ($($t:tt)*) => { $crate::__log_macro!($crate::abi::level::DEBUG, $($t)*) } }
#[macro_export]
macro_rules! trace { ($($t:tt)*) => { $crate::__log_macro!($crate::abi::level::TRACE, $($t)*) } }

/// Declares the mod entry point.
///
/// ```ignore
/// voxide_sdk::declare_mod!(id: "mymod", name: "My Mod", version: "1.0.0", deps: ["base"], init: init);
/// ```
#[macro_export]
macro_rules! declare_mod {
    (id: $id:literal, name: $name:literal, version: $ver:literal, deps: [$($dep:literal),* $(,)?], init: $init:path $(,)?) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn voxide_mod() -> *const $crate::abi::ModDecl {
            static DEPS: &[$crate::abi::Str] = &[$($crate::abi::Str::new($dep)),*];
            unsafe extern "C" fn __voxide_init(
                host: *const $crate::abi::Host,
                me: $crate::abi::ModId,
                reg: *mut $crate::abi::Registrar,
            ) -> u32 {
                unsafe { $crate::__init(host, me, reg, $id, $init) }
            }
            static DECL: $crate::abi::ModDecl = $crate::abi::ModDecl {
                abi_version: $crate::abi::ABI_VERSION,
                id: $crate::abi::Str::new($id),
                name: $crate::abi::Str::new($name),
                version: $crate::abi::Str::new($ver),
                deps: $crate::abi::Slice::new(DEPS),
                init: __voxide_init,
            };
            &DECL
        }
    };
}

#[doc(hidden)]
pub unsafe fn __init(
    host: *const Host,
    me: ModId,
    reg: *mut abi::Registrar,
    id: &'static str,
    f: fn(&mut Registry),
) -> u32 {
    HOST.store(host as *mut Host, Ordering::Relaxed);
    ME.store(me.0, Ordering::Relaxed);
    let _ = MOD_ID.set(id);
    install_panic_hook();
    let mut r = Registry { raw: reg, ns: id };
    guard(|| f(&mut r))
}

fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let loc = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
            let msg = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            __log(abi::level::ERROR, "", format_args!("panicked{loc}: {msg}"));
        }));
    });
}

/// Runs `f`, turning a panic into an error code so it never unwinds into the engine.
pub fn guard(f: impl FnOnce()) -> u32 {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Wraps a closure into an ABI callback. The closure is leaked: callbacks live as long as the mod.
pub fn callback<A, F>(f: F) -> Callback
where
    F: Fn(&mut A) + Send + Sync + 'static,
{
    unsafe extern "C" fn tramp<A, F: Fn(&mut A)>(data: *const c_void, args: *mut c_void) -> u32 {
        let f = unsafe { &*(data as *const F) };
        let args = unsafe { &mut *(args as *mut A) };
        guard(|| f(args))
    }
    Callback { func: Some(tramp::<A, F>), data: Box::into_raw(Box::new(f)) as *const c_void }
}

/// Prefixes the mod namespace unless the name already has one.
pub(crate) fn qualify(ns: &str, name: &str) -> String {
    if name.contains(':') { name.to_string() } else { format!("{ns}:{name}") }
}
