//! Settings mods (and the engine, namespace `core`) declare: typed, ranged values read with one
//! atomic load, stored per namespace in `config/<ns>.txt`, edited on a screen built from the
//! declarations. `SYNCED` ones follow the server: a joining player gets its values.

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use voxide_abi::{config_flags as cf, config_kind as ck};

use crate::content::content;

#[derive(Clone, Debug)]
pub struct ConfigDef {
    pub name: String,
    pub kind: u32,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub choices: Vec<String>,
    pub flags: u32,
}

impl ConfigDef {
    pub fn synced(&self) -> bool {
        self.flags & cf::SYNCED != 0
    }

    /// `v` made valid for this setting.
    pub fn fit(&self, v: f64) -> f64 {
        if !v.is_finite() {
            return self.default;
        }
        match self.kind {
            ck::BOOL => (v != 0.0) as u8 as f64,
            ck::CHOICE => v.round().clamp(0.0, self.choices.len().saturating_sub(1) as f64),
            _ => {
                let mut v = v.clamp(self.min.min(self.max), self.max.max(self.min));
                if self.step > 0.0 {
                    v = self.min + ((v - self.min) / self.step).round() * self.step;
                }
                if self.kind == ck::INT { v.round() } else { v }
            }
        }
    }

    fn parse(&self, s: &str) -> Option<f64> {
        let s = s.trim();
        match self.kind {
            ck::BOOL => match s {
                "true" | "on" | "1" => Some(1.0),
                "false" | "off" | "0" => Some(0.0),
                _ => None,
            },
            ck::CHOICE => self.choices.iter().position(|c| c == s).map(|i| i as f64),
            _ => s.parse().ok(),
        }
    }

    pub fn show(&self, v: f64) -> String {
        match self.kind {
            ck::BOOL => (v != 0.0).to_string(),
            ck::CHOICE => self.choices.get(v as usize).cloned().unwrap_or_default(),
            ck::INT => format!("{}", v as i64),
            _ => {
                let s = format!("{v:.3}");
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            }
        }
    }
}

/// Value bits by config id.
static VALUES: OnceLock<Vec<AtomicU64>> = OnceLock::new();
static RUN_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

fn values() -> &'static [AtomicU64] {
    VALUES.get_or_init(|| content().game.configs.iter().map(|d| AtomicU64::new(d.default.to_bits())).collect())
}

pub fn get(id: u32) -> f64 {
    values().get(id as usize).map_or(0.0, |v| f64::from_bits(v.load(Ordering::Relaxed)))
}

/// Sets a value without saving (server values arriving, loading files).
pub fn put(id: u32, v: f64) {
    if let (Some(slot), Some(d)) = (values().get(id as usize), content().game.configs.get(id as usize)) {
        slot.store(d.fit(v).to_bits(), Ordering::Relaxed);
    }
}

/// Sets a value and saves its namespace's file.
pub fn set(id: u32, v: f64) {
    put(id, v);
    let (Some(dir), Some(d)) = (RUN_DIR.get(), content().game.configs.get(id as usize)) else { return };
    if let Some((ns, _)) = d.name.split_once(':') {
        save(dir, ns);
    }
}

pub fn by_name(name: &str) -> Option<u32> {
    content().game.config_names.get(name).copied()
}

/// Reads every namespace's `config/<ns>.txt`; missing lines keep their defaults, unknown or bad
/// ones are reported.
pub fn load(run_dir: &Path) {
    let _ = RUN_DIR.set(run_dir.to_path_buf());
    let c = content();
    for (id, d) in c.game.configs.iter().enumerate() {
        put(id as u32, d.default);
    }
    let mut namespaces: Vec<&str> = c.game.configs.iter().filter_map(|d| d.name.split_once(':').map(|x| x.0)).collect();
    namespaces.dedup();
    for ns in namespaces {
        let Ok(src) = std::fs::read_to_string(file(run_dir, ns)) else { continue };
        for (_, k, v) in crate::kv::pairs(&src) {
            let name = format!("{ns}:{k}");
            match by_name(&name).map(|id| (id, &c.game.configs[id as usize])) {
                Some((id, d)) => match d.parse(v) {
                    Some(v) => put(id, v),
                    None => log::warn!("config {name}: cannot use '{}', keeping {}", v.trim(), d.show(d.default)),
                },
                None => log::warn!("config/{ns}.txt: no setting {}", k.trim()),
            }
        }
    }
}

fn file(run_dir: &Path, ns: &str) -> std::path::PathBuf {
    run_dir.join("config").join(format!("{ns}.txt"))
}

/// Writes a namespace's settings, every one of them so players see what exists.
pub fn save(run_dir: &Path, ns: &str) {
    let c = content();
    let mut out = String::new();
    for (id, d) in c.game.configs.iter().enumerate() {
        let Some(key) = d.name.strip_prefix(ns).and_then(|k| k.strip_prefix(':')) else { continue };
        out.push_str(&format!("{key} = {}\n", d.show(get(id as u32))));
    }
    let path = file(run_dir, ns);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    if let Err(e) = std::fs::write(&path, out) {
        log::error!("saving {}: {e}", path.display());
    }
}

/// Values of the synced settings, for players joining this server.
pub fn synced() -> Vec<(u32, f64)> {
    content().game.configs.iter().enumerate().filter(|(_, d)| d.synced()).map(|(i, _)| (i as u32, get(i as u32))).collect()
}

/// Settings the engine itself reads; each sound category adds its volume (`volume`).
pub fn engine_configs() -> Vec<ConfigDef> {
    vec![volume("core:volume_master")]
}

/// A volume setting in percent.
pub fn volume(name: &str) -> ConfigDef {
    ConfigDef { name: name.into(), kind: ck::INT, default: 100.0, min: 0.0, max: 100.0, step: 1.0, choices: Vec::new(), flags: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_fit_their_setting() {
        let d = ConfigDef { name: "t:x".into(), kind: ck::INT, default: 5.0, min: 0.0, max: 10.0, step: 2.0, choices: vec![], flags: 0 };
        assert_eq!((d.fit(3.2), d.fit(99.0), d.fit(f64::NAN)), (4.0, 10.0, 5.0));
        let c = ConfigDef { kind: ck::CHOICE, choices: vec!["low".into(), "high".into()], ..d.clone() };
        assert_eq!((c.parse("high"), c.parse("mid"), c.show(0.0)), (Some(1.0), None, "low".into()));
        let b = ConfigDef { kind: ck::BOOL, ..d };
        assert_eq!((b.parse("on"), b.show(1.0)), (Some(1.0), "true".into()));
    }
}
