use std::path::PathBuf;

use voxide_engine::Options;
use voxide_render::{Backend, BackendDesc, Error};

fn vulkan(desc: &BackendDesc) -> Result<Box<dyn Backend>, Error> {
    Ok(Box::new(voxide_vulkan::VulkanBackend::new(desc)?))
}

fn main() {
    let mut opts = Options {
        run_dir: PathBuf::from("run"),
        mod_dirs: vec![PathBuf::from("mods")],
        gpu_debug: cfg!(debug_assertions),
        vsync: true,
        shot: None,
        shot_delay: 0.0,
        camera: None,
        seed: 1,
        view_radius: 12,
        size: (1600, 900),
    };
    let mut verbose = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--run-dir" => opts.run_dir = args.next().map(PathBuf::from).unwrap_or(opts.run_dir),
            "--mods" => opts.mod_dirs.push(args.next().map(PathBuf::from).unwrap_or_default()),
            "--gpu-debug" => opts.gpu_debug = true,
            "--no-gpu-debug" => opts.gpu_debug = false,
            "--no-vsync" => opts.vsync = false,
            "--verbose" => verbose = true,
            "--shot" => opts.shot = args.next().map(PathBuf::from),
            "--shot-delay" => opts.shot_delay = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0),
            "--camera" => {
                let v: Vec<f64> = args.next().unwrap_or_default().split(',').filter_map(|s| s.parse().ok()).collect();
                if v.len() == 5 {
                    opts.camera = Some([v[0], v[1], v[2], v[3], v[4]]);
                }
            }
            "--seed" => opts.seed = args.next().and_then(|s| s.parse().ok()).unwrap_or(opts.seed),
            "--view" => opts.view_radius = args.next().and_then(|s| s.parse().ok()).unwrap_or(opts.view_radius),
            "--size" => {
                let v: Vec<u32> = args.next().unwrap_or_default().split('x').filter_map(|s| s.parse().ok()).collect();
                if v.len() == 2 {
                    opts.size = (v[0], v[1]);
                }
            }
            other => eprintln!("unknown argument {other}"),
        }
    }
    voxide_engine::log::init(Some(&opts.run_dir.join("logs").join("latest.log")), verbose);
    if let Err(e) = voxide_engine::run(vulkan, opts) {
        log::error!("{e}");
        std::process::exit(1);
    }
}
