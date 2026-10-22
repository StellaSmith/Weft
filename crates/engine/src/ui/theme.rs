//! Themes: `ui/theme.theme` files from every mod and resource pack, merged (packs win). Values are
//! looked up by `class.property`, falling back to `default.property`.
//!
//! ```text
//! button.texture = base:gui/button
//! button.border = 4
//! button.hover = base:gui/button_hover
//! button.text = #ffffff
//! ```

use rustc_hash::FxHashMap;

use crate::assets::Assets;

#[derive(Default)]
pub struct Theme {
    values: FxHashMap<String, String>,
}

impl Theme {
    pub fn load(assets: &Assets) -> Theme {
        let mut values = FxHashMap::default();
        let mut files = assets.list("ui", "theme");
        // Engine defaults first so mods override them.
        files.sort_by_key(|(name, _)| !name.starts_with("core:"));
        for (_, path) in files {
            let Ok(src) = std::fs::read_to_string(&path) else { continue };
            for line in src.lines() {
                if line.trim_start().starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    values.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        Theme { values }
    }

    pub fn get(&self, class: &str, prop: &str) -> Option<&str> {
        self.values
            .get(&format!("{class}.{prop}"))
            .or_else(|| self.values.get(&format!("default.{prop}")))
            .map(String::as_str)
    }

    pub fn color(&self, class: &str, prop: &str, default: u32) -> u32 {
        self.get(class, prop)
            .and_then(|v| {
                let h = v.trim_start_matches('#');
                let c = u32::from_str_radix(h, 16).ok()?;
                Some(if h.len() <= 6 { c << 8 | 0xFF } else { c })
            })
            .unwrap_or(default)
    }

    pub fn num(&self, class: &str, prop: &str, default: f32) -> f32 {
        self.get(class, prop).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
}
