//! A resource pack made from the player's own copy of Minecraft. Mojang's textures can't ship
//! with the game, but a player who owns Minecraft has them on disk already: on start the game
//! finds the newest client jar of the official launcher or Prism, and builds
//! `resourcepacks/minecraft/` from it, enabling it the first time. The pack never leaves the
//! computer it was made on.
//!
//! What goes where is data of each mod (and the engine for `core:`): `assets/minecraft.txt`
//! lists `ours = theirs` texture paths, and files under `assets/minecraft/` are copied into the
//! pack as they are (a `.ctm` or `.gui` that fits the vanilla picture better).

use std::io::Read;
use std::path::{Path, PathBuf};

pub const PACK: &str = "minecraft";

/// Client jars the launchers keep, newest first.
fn jars() -> Vec<(PathBuf, String)> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        roots.push(appdata.join(".minecraft"));
        roots.push(appdata.join("PrismLauncher"));
    }
    if let Some(h) = &home {
        roots.push(h.join(".minecraft"));
        roots.push(h.join("Library/Application Support/minecraft"));
        roots.push(h.join("Library/Application Support/PrismLauncher"));
        roots.push(h.join(".local/share/PrismLauncher"));
    }
    let mut found: Vec<(std::time::SystemTime, PathBuf, String)> = Vec::new();
    let mut add = |path: PathBuf, version: String| {
        if let Ok(t) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            found.push((t, path, version));
        }
    };
    for root in roots {
        // Official launcher: versions/<v>/<v>.jar.
        for e in std::fs::read_dir(root.join("versions")).into_iter().flatten().flatten() {
            let v = e.file_name().to_string_lossy().into_owned();
            add(e.path().join(format!("{v}.jar")), v);
        }
        // Prism: libraries/com/mojang/minecraft/<v>/minecraft-<v>-client.jar.
        for e in std::fs::read_dir(root.join("libraries/com/mojang/minecraft")).into_iter().flatten().flatten() {
            let v = e.file_name().to_string_lossy().into_owned();
            add(e.path().join(format!("minecraft-{v}-client.jar")), v);
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p, v)| (p, v)).collect()
}

/// Makes or refreshes the pack; true when it was made for the first time (the caller enables it).
/// `mods` are (namespace, assets folder), the engine's `core` among them.
pub fn ensure_pack(run_dir: &Path, mods: &[(String, PathBuf)]) -> bool {
    let maps: Vec<(String, PathBuf, String)> = mods
        .iter()
        .filter_map(|(ns, dir)| std::fs::read_to_string(dir.join("minecraft.txt")).ok().map(|m| (ns.clone(), dir.clone(), m)))
        .collect();
    if maps.is_empty() {
        return false;
    }
    let out = crate::packs::resource_dir(run_dir).join(PACK);
    let fresh = !out.is_dir();
    // Snapshots and modded versions can't be told from releases by name: a jar without the
    // textures is skipped.
    for (jar, version) in jars() {
        let Ok(stamp) = std::fs::metadata(&jar).and_then(|m| m.modified()) else { continue };
        let mut key = format!("{}|{:?}", jar.display(), stamp);
        for (ns, _, m) in &maps {
            key.push_str(ns);
            key.push_str(m);
        }
        let key = format!("{:016x}", fnv(&key));
        let cfg = std::fs::read_to_string(out.join("pack.cfg")).unwrap_or_default();
        if crate::kv::get(&cfg, "source").is_some_and(|k| k == key) {
            return false;
        }
        match build(&jar, &version, &maps, &out, &key) {
            Ok(n) => {
                log::info!("made the {PACK} resource pack from {} ({n} files)", jar.display());
                return fresh;
            }
            Err(e) => log::warn!("{}: {e}", jar.display()),
        }
    }
    false
}

fn build(jar: &Path, version: &str, maps: &[(String, PathBuf, String)], out: &Path, key: &str) -> Result<usize, String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(jar).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    const T: &str = "assets/minecraft/textures/";
    if zip.by_name(&format!("{T}block/dirt.png")).is_err() {
        return Err("no textures in it".into());
    }
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    };
    let tmp = out.with_extension("new");
    let _ = std::fs::remove_dir_all(&tmp);
    let mut files = 0;
    for (ns, dir, map) in maps {
        let base = tmp.join(ns).join("textures");
        for (n, line) in map.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((ours, theirs)) = line.split_once('=') else { continue };
            let (ours, theirs) = (ours.trim(), theirs.trim());
            let mut words = theirs.split_whitespace();
            let (mode, src) = match words.next() {
                Some("glow") => ("glow", words.next().unwrap_or("")),
                Some(s) => ("copy", s),
                None => continue,
            };
            let crop: Vec<u32> = match words.next() {
                Some("crop") => words.filter_map(|w| w.parse().ok()).collect(),
                _ => Vec::new(),
            };
            let Some(png) = read(&format!("{T}{src}.png")) else {
                // Older or newer versions name some textures differently: ours stays.
                log::debug!("{ns}/minecraft.txt:{}: no {src} in this version", n + 1);
                continue;
            };
            let dst = base.join(format!("{ours}.png"));
            let dst_dir = dst.parent().map(Path::to_path_buf).unwrap_or_default();
            std::fs::create_dir_all(&dst_dir).map_err(|e| e.to_string())?;
            if mode == "glow" || crop.len() == 4 {
                let img = crate::assets::decode_png(&png).ok_or(format!("{src}: not a png"))?;
                let (x0, y0, w, h) = if crop.len() == 4 { (crop[0], crop[1], crop[2], crop[3]) } else { (0, 0, img.width, img.height) };
                let mut px = Vec::with_capacity((w * h * 4) as usize);
                for y in y0..(y0 + h).min(img.height) {
                    let row = ((y * img.width + x0) * 4) as usize;
                    px.extend_from_slice(&img.rgba[row..row + (w.min(img.width - x0) * 4) as usize]);
                }
                if mode == "glow" {
                    // White where the texel is bright and warm (fire, embers).
                    for p in px.chunks_exact_mut(4) {
                        let on = p[3] > 0 && p[0] > 150 && p[0] as i32 - p[2] as i32 > 80;
                        p.copy_from_slice(&if on { [255; 4] } else { [0, 0, 0, 255] });
                    }
                }
                crate::assets::encode_png(&dst, w, h, &px).map_err(|e| e.to_string())?;
            } else {
                std::fs::write(&dst, &png).map_err(|e| e.to_string())?;
            }
            files += 1;
            if let Some(meta) = read(&format!("{T}{src}.png.mcmeta")).and_then(|m| String::from_utf8(m).ok()) {
                if meta.contains("\"gui\"") {
                    let _ = std::fs::write(dst.with_extension("png.mcmeta"), &meta);
                }
                if let Some(a) = serde_json::from_str::<serde_json::Value>(&meta).ok().and_then(|v| v.get("animation").cloned()) {
                    let mut s = format!("frametime = {}\n", a.get("frametime").and_then(|f| f.as_u64()).unwrap_or(1));
                    if a.get("interpolate").and_then(|i| i.as_bool()) == Some(true) {
                        s.push_str("interpolate = true\n");
                    }
                    let _ = std::fs::write(dst.with_extension("anim"), s);
                }
            }
        }
        // Files the mod made to go with the vanilla pictures.
        copy_tree(&dir.join("minecraft"), &tmp.join(ns)).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        tmp.join("pack.cfg"),
        format!("name = Minecraft {version}\ndescription = Made from your own copy of Minecraft, stays on this computer\nsource = {key}\n"),
    )
    .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(out);
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(files)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    for e in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            copy_tree(&e.path(), &dst)?;
        } else {
            std::fs::create_dir_all(to)?;
            std::fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}
