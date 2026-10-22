//! Translations. Only the selected language and the English fallback are ever loaded; the
//! language list reads just the first line of each file.
//!
//! File format (`lang/<code>.lang` in any mod or pack): `key = value` per line, `#` comments, the
//! first entry should be `language.name`. Values are templates, see `text.rs`.

use std::cell::RefCell;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::assets::Assets;

pub const FALLBACK: &str = "en_us";

pub struct Lang {
    pub code: String,
    entries: FxHashMap<String, String>,
    fallback: FxHashMap<String, String>,
    pub dev: bool,
    reported: RefCell<FxHashSet<String>>,
}

fn parse(src: &str, into: &mut FxHashMap<String, String>) {
    for line in src.lines() {
        let line = line.trim_start_matches('\u{feff}');
        if line.trim_start().starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            into.insert(k.trim().to_string(), v.trim().replace("\\n", "\n"));
        }
    }
}

fn load_code(assets: &Assets, code: &str) -> FxHashMap<String, String> {
    let mut map = FxHashMap::default();
    for (name, path) in assets.list("lang", "lang") {
        if name.ends_with(&format!(":{code}")) {
            if let Ok(src) = std::fs::read_to_string(&path) {
                parse(&src, &mut map);
            }
        }
    }
    map
}

impl Lang {
    pub fn load(assets: &Assets, code: &str, dev: bool) -> Lang {
        let fallback = load_code(assets, FALLBACK);
        let entries = if code == FALLBACK { FxHashMap::default() } else { load_code(assets, code) };
        Lang { code: code.to_string(), entries, fallback, dev, reported: RefCell::new(FxHashSet::default()) }
    }

    /// Template for a key. A missing key falls back to English; if that is missing too, development
    /// builds show the key itself (marked) so it gets noticed.
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(v) = self.entries.get(key).or_else(|| self.fallback.get(key)) {
            return v;
        }
        if self.reported.borrow_mut().insert(key.to_string()) {
            log::warn!("missing translation: {key}");
        }
        key
    }

    pub fn has(&self, key: &str) -> bool {
        self.entries.contains_key(key) || self.fallback.contains_key(key)
    }

    pub fn is_missing_in_current(&self, key: &str) -> bool {
        self.code != FALLBACK && !self.entries.contains_key(key)
    }

    /// (code, display name) for every language any mod ships, reading only the first line.
    pub fn available(assets: &Assets) -> Vec<(String, String)> {
        let mut out: FxHashMap<String, String> = FxHashMap::default();
        for (name, path) in assets.list("lang", "lang") {
            let code = name.rsplit(':').next().unwrap_or("").to_string();
            if out.get(&code).is_some_and(|n| n != &code) {
                continue;
            }
            let first = std::fs::read_to_string(&path).ok().and_then(|s| {
                s.lines().find(|l| l.trim_start().starts_with("language.name")).and_then(|l| l.split_once('=')).map(|(_, v)| v.trim().to_string())
            });
            out.insert(code.clone(), first.unwrap_or(code));
        }
        let mut v: Vec<_> = out.into_iter().collect();
        v.sort();
        v
    }
}
