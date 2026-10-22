//! Finds mod libraries, loads them and runs registration in parallel.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use rayon::prelude::*;
use voxide_abi::{self as abi, ModId};

use crate::content::{self, ModMeta, RawRegistrar};

pub struct LoadedMod {
    pub id: String,
    pub dir: PathBuf,
    pub assets: PathBuf,
    _lib: libloading::Library,
    decl: &'static abi::ModDecl,
}

static NAMES: OnceLock<Vec<String>> = OnceLock::new();

pub fn mod_id_name(m: ModId) -> &'static str {
    NAMES.get().and_then(|n| n.get(m.0 as usize)).map_or("?", |s| s.as_str())
}

/// Mod roots: every subdirectory of a mods dir is one mod; its library is either inside it or, for
/// development builds, next to the engine executable (`target/<profile>/`).
pub fn discover(mod_dirs: &[PathBuf]) -> Vec<(String, PathBuf, PathBuf)> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let mut found = Vec::new();
    for dir in mod_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if !path.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let file = libloading::library_filename(&name);
            let lib = [Some(path.join(&file)), exe_dir.as_ref().map(|d| d.join(&file))]
                .into_iter()
                .flatten()
                .find(|p| p.exists());
            match lib {
                Some(lib) => found.push((name, path, lib)),
                None => log::warn!("mod dir {} has no {}", path.display(), file.to_string_lossy()),
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.0 == b.0);
    found
}

/// Loads every library, then calls all `init`s in parallel and freezes the registries.
pub fn load(mod_dirs: &[PathBuf], cache: &Path) -> Result<Vec<LoadedMod>, String> {
    let start = Instant::now();
    let shadow = cache.join("mods");
    std::fs::create_dir_all(&shadow).ok();
    if let Ok(entries) = std::fs::read_dir(&shadow) {
        for e in entries.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }

    let mut mods = Vec::new();
    for (name, dir, lib_path) in discover(mod_dirs) {
        // Windows locks loaded libraries; loading a copy keeps the original rebuildable.
        let copy = shadow.join(format!("{}-{}", std::process::id(), lib_path.file_name().unwrap().to_string_lossy()));
        let load_from = if std::fs::copy(&lib_path, &copy).is_ok() { copy } else { lib_path.clone() };
        let lib = unsafe { libloading::Library::new(&load_from) }
            .map_err(|e| format!("loading mod {name} from {}: {e}", lib_path.display()))?;
        let entry: libloading::Symbol<unsafe extern "C" fn() -> *const abi::ModDecl> =
            unsafe { lib.get(abi::ENTRY_SYMBOL) }.map_err(|_| format!("{} is not a Voxide mod", lib_path.display()))?;
        let decl = unsafe { &*entry() };
        if decl.abi_version != abi::ABI_VERSION {
            return Err(format!("mod {name} targets ABI {} but the engine has {}", decl.abi_version, abi::ABI_VERSION));
        }
        let id = unsafe { decl.id.as_str() }.to_string();
        if id != name {
            log::warn!("mod directory {name} contains mod '{id}'");
        }
        mods.push(LoadedMod { id, assets: dir.join("assets"), dir, _lib: lib, decl });
    }
    mods.sort_by(|a, b| a.id.cmp(&b.id));
    let _ = NAMES.set(mods.iter().map(|m| m.id.clone()).collect());

    let metas: Vec<ModMeta> = mods
        .iter()
        .map(|m| unsafe {
            ModMeta {
                id: m.id.clone(),
                name: m.decl.name.as_str().to_string(),
                version: m.decl.version.as_str().to_string(),
                deps: m.decl.deps.as_slice().iter().map(|d| d.as_str().to_string()).collect(),
            }
        })
        .collect();
    for m in &metas {
        for d in &m.deps {
            if !metas.iter().any(|o| &o.id == d) {
                return Err(format!("mod {} requires missing mod {d}", m.id));
            }
        }
    }

    let regs: Vec<RawRegistrar> = mods
        .par_iter()
        .enumerate()
        .map(|(i, m)| {
            let mut reg = RawRegistrar::new(ModId(i as u32));
            let t = Instant::now();
            let r = unsafe { (m.decl.init)(&crate::host::HOST, ModId(i as u32), &mut reg as *mut _ as *mut abi::Registrar) };
            if r != 0 {
                reg.errors.push("init panicked".into());
            }
            log::debug!("{} registered in {:.1} ms", m.id, t.elapsed().as_secs_f64() * 1000.0);
            reg
        })
        .collect();

    let content = content::freeze(metas, regs).map_err(|e| e.join("\n"))?;
    log::info!(
        "{} mods, {} blocks ({} states), {} items, {} dimensions in {:.1} ms",
        mods.len(),
        content.blocks.len(),
        content.states.len(),
        content.items.len(),
        content.dims.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    content::set(content);
    Ok(mods)
}
