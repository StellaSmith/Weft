pub mod app;
pub mod assets;
pub mod autopilot;
pub mod bench;
pub mod client;
pub mod commands;
pub mod game_screens;
pub mod clipboard;
pub mod content;
pub mod host;
pub mod hud;
pub mod input;
pub mod log;
pub mod modloader;
pub mod net;
pub mod packs;
pub mod physics;
pub mod registry;
pub mod saves;
pub mod screens;
pub mod server;
pub mod settings;
pub mod ui;
pub mod world;

pub use app::{BackendFactory, Options, run};

/// Background workers (generation, meshing, region ticks) leave two cores to the render and
/// server threads: a full pool starved the render thread during loading and frames missed vsync.
pub fn init_workers() {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let _ = rayon::ThreadPoolBuilder::new().num_threads(cores.saturating_sub(2).max(1)).build_global();
}
