use std::path::PathBuf;

use voxide_engine::Options;
use voxide_render::{Backend, BackendDesc, Error};

#[global_allocator]
static ALLOC: voxide_engine::diag::CountingAlloc = voxide_engine::diag::CountingAlloc;

fn vulkan(desc: &BackendDesc) -> Result<Box<dyn Backend>, Error> {
    Ok(Box::new(voxide_vulkan::VulkanBackend::new(desc)?))
}

/// Where `run` (and, in the source tree, `mods`) live: the working directory if it has either,
/// otherwise the nearest folder above the executable that does (a development build runs from
/// `target/<profile>/`).
fn game_root() -> PathBuf {
    let has = |d: &std::path::Path| d.join("run").is_dir() || d.join("mods").is_dir();
    let cwd = std::env::current_dir().unwrap_or_default();
    if has(&cwd) {
        return cwd;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors().skip(1).find(|d| has(d)).map(PathBuf::from).unwrap_or(cwd)
}

fn main() {
    let root = game_root();
    let mut opts = Options {
        run_dir: root.join("run"),
        // Mod sources of the game itself; players' mods go into `run/mods`, added below.
        mod_dirs: vec![root.join("mods")],
        gpu_debug: false,
        shot: None,
        shot_delay: 0.0,
        camera: None,
        world: None,
        seed: None,
        screen: None,
        size: (1600, 900),
        dev: cfg!(debug_assertions),
        script: None,
        no_vsync: false,
        view: None,
    };
    let mut verbose = false;
    let mut log_spec = String::new();
    let mut pack: Option<PathBuf> = None;
    let mut dist: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_default();
        match a.as_str() {
            "--run-dir" => opts.run_dir = PathBuf::from(value()),
            "--mods" => opts.mod_dirs.push(PathBuf::from(value())),
            "--gpu-debug" => opts.gpu_debug = true,
            "--verbose" => verbose = true,
            "--log" => log_spec = value(),
            "--shot" => opts.shot = Some(PathBuf::from(value())),
            "--shot-delay" => opts.shot_delay = value().parse().unwrap_or(0.0),
            "--camera" => {
                let v: Vec<f64> = value().split(',').filter_map(|s| s.parse().ok()).collect();
                if v.len() == 5 {
                    opts.camera = Some([v[0], v[1], v[2], v[3], v[4]]);
                }
            }
            "--world" => opts.world = Some(value()),
            "--seed" => opts.seed = value().parse().ok(),
            "--screen" => opts.screen = Some(value()),
            "--script" => opts.script = Some(PathBuf::from(value())),
            "--no-vsync" => opts.no_vsync = true,
            "--view" => opts.view = value().parse().ok(),
            "--pack-mod" => pack = Some(PathBuf::from(value())),
            "--dist" => dist = Some(PathBuf::from(value())),
            "--size" => {
                let v: Vec<u32> = value().split('x').filter_map(|s| s.parse().ok()).collect();
                if v.len() == 2 {
                    opts.size = (v[0], v[1]);
                }
            }
            other => eprintln!("unknown argument {other}"),
        }
    }
    let player_mods = opts.run_dir.join("mods");
    if let Some(dir) = pack {
        match voxide_engine::modloader::pack(&dir, &player_mods) {
            Ok(path) => println!("packed {}", path.display()),
            Err(e) => eprintln!("cannot pack {}: {e}", dir.display()),
        }
        return;
    }
    if let Some(out) = dist {
        match make_dist(&root, &out) {
            Ok(()) => println!("game ready in {}", out.display()),
            Err(e) => eprintln!("cannot make {}: {e}", out.display()),
        }
        return;
    }
    voxide_engine::log::init(Some(&opts.run_dir.join("logs").join("latest.log")), verbose, &log_spec);
    voxide_engine::diag::install_crash_handler(&opts.run_dir);
    // Release builds load the game's own mods the way players get them: each packed into one
    // file in `run/mods` (again only when it changed).
    if !cfg!(debug_assertions) && root.join("mods").is_dir() {
        for dir in subdirs(&root.join("mods")) {
            match voxide_engine::modloader::pack_if_stale(&dir, &player_mods) {
                Ok((path, true)) => log::info!("packed {}", path.display()),
                Ok(_) => {}
                Err(e) => log::warn!("cannot pack {}: {e}", dir.display()),
            }
        }
        let sources = root.join("mods");
        opts.mod_dirs.retain(|d| *d != sources);
    }
    opts.mod_dirs.push(player_mods);
    voxide_engine::init_workers();
    if let Err(e) = voxide_engine::run(vulkan, opts) {
        log::error!("{e}");
        std::process::exit(1);
    }
}

fn subdirs(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// A folder to hand to someone: the executable, the engine's assets and every mod packed.
fn make_dist(root: &std::path::Path, out: &std::path::Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    std::fs::copy(&exe, out.join(exe.file_name().unwrap())).map_err(|e| e.to_string())?;
    copy_dir(&root.join("crates").join("engine").join("assets"), &out.join("assets").join("core"))?;
    for dir in subdirs(&root.join("mods")) {
        voxide_engine::modloader::pack(&dir, &out.join("run").join("mods"))?;
    }
    Ok(())
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
