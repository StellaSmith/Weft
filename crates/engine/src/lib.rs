pub mod gpu_layout;
pub mod kv;
pub mod app;
pub mod assets;
pub mod audio;

pub mod bench;
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
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(workers()).thread_name(|i| format!("background {i}")).build().expect("background pool"))
}
