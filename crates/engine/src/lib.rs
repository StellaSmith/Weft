pub mod gpu_layout;
pub mod kv;
pub mod app;
pub mod assets;
pub mod audio;

pub mod bench;
pub mod block_model;
pub mod client;
pub mod commands;
pub mod config;
pub mod components;
pub mod game_screens;
pub mod clipboard;
pub mod content;
pub mod diag;
pub mod host;
pub mod hud;
pub mod input;
pub mod log;
pub mod loot;
pub mod model;
pub mod modloader;
pub mod net;
pub mod packs;
pub mod physics;
pub mod post;
pub mod refs;
pub mod registry;
pub mod saves;
pub mod screens;
pub mod server;
pub mod settings;
pub mod shaders;
pub mod stack;
pub mod ui;
pub mod world;

pub use app::api::{exit_code, set_launch_options};
pub use app::{BackendFactory, Options, run};

/// Background workers (generation, meshing, region ticks) leave two cores to the render and
/// server threads: a full pool starved the render thread during loading and frames missed vsync.
pub fn init_workers() {
    let _ = rayon::ThreadPoolBuilder::new().num_threads(workers()).build_global();
}

fn workers() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(2).max(1)
}

/// The pool for bulk work queued by the thousand (generating and saving cubes). Work that
/// something waits for (region ticks, path searches, meshing) runs on the global pool: queued
/// behind a full generation backlog there, a tick or a frame waited for all of it.
pub fn background() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new().num_threads(workers()).thread_name(|i| format!("background {i}")).start_handler(|_| lower_priority()).build().expect("background pool")
    })
}

/// Meshing: queued by the thousand while loading, so apart from the global pool, where the
/// frame's culling waited behind it.
pub fn meshing() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(workers()).thread_name(|i| format!("mesh {i}")).start_handler(|_| lower_priority()).build().expect("mesh pool"))
}

/// Bulk work runs below normal priority: with every core busy loading, the frame and the
/// server still get theirs when they need them.
fn lower_priority() {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
    }
}
