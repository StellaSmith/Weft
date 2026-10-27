//! How UI classes look, by convention instead of a theme file. Class `button` is the texture
//! `gui/button.png`, its states `gui/button_hover.png`, `_pressed`, `_disabled` (`_focused` for
//! inputs ...), with its `.gui` sidecar for slicing and the `shader` or `animation` that goes with
//! it. A bare class is looked up in the resource packs, then the mods from the last loaded, then
//! the engine; `ns:class` only in that namespace. A class without a texture shows a magenta and
//! black checkerboard, like a missing block texture.
//!
//! Text colors are translations: `color.<class>` (and `color.<class>_hover`, `_disabled`, `_dim`,
//! `_shadow`, `none` for no shadow) holding `{#rrggbb}`, falling back to `color.default`. They
//! merge like every translation, so a pack changes a color with one line.

use std::cell::RefCell;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::assets::Assets;
use crate::ui::lang::Lang;

pub struct Look {
    /// Namespaces a bare class is searched in, first to last.
    namespaces: Vec<String>,
    textures: RefCell<FxHashMap<String, Option<String>>>,
    sidecars: RefCell<FxHashMap<String, Option<String>>>,
    /// `color.<name>` translations, and the names set to `none`.
    colors: FxHashMap<String, u32>,
    nones: FxHashSet<String>,
    assets: Assets,
}

impl Look {
    /// `mods`: mod ids in load order. Made again when the language or the packs change.
    pub fn new(assets: &Assets, mods: &[String], lang: &Lang) -> Look {
        let mut namespaces: Vec<String> = mods.iter().rev().cloned().collect();
        namespaces.push("core".into());
        let (colors, nones) = lang.colors();
        Look { namespaces, textures: Default::default(), sidecars: Default::default(), colors, nones, assets: assets.clone() }
    }

    /// The texture of `class` in `state` ("" for its plain look), `ns:gui/<class>[_<state>]`.
    pub fn texture(&self, class: &str, state: &str) -> Option<String> {
        let key = if state.is_empty() { class.to_string() } else { format!("{class}_{state}") };
        if let Some(t) = self.textures.borrow().get(&key) {
            return t.clone();
        }
        let found = match key.split_once(':') {
            Some((ns, path)) => Some(format!("{ns}:gui/{path}")).filter(|n| self.assets.path(n, "textures", "png").is_some()),
            None => self.namespaces.iter().map(|ns| format!("{ns}:gui/{key}")).find(|n| self.assets.path(n, "textures", "png").is_some()),
        };
        self.textures.borrow_mut().insert(key, found.clone());
        found
    }

    /// A state's texture, else the class's own.
    pub fn texture_or_plain(&self, class: &str, state: &str) -> Option<String> {
        self.texture(class, state).or_else(|| self.texture(class, ""))
    }

    /// A value from the `.gui` sidecar of the class's texture (`shader`, `animation`).
    pub fn sidecar(&self, class: &str, key: &str) -> Option<String> {
        let cache = format!("{class}|{key}");
        if let Some(v) = self.sidecars.borrow().get(&cache) {
            return v.clone();
        }
        let v = self.texture(class, "").and_then(|t| self.assets.read(&t, "textures", "gui")).and_then(|b| {
            let src = String::from_utf8_lossy(&b).into_owned();
            crate::kv::get(&src, key).map(str::to_string)
        });
        self.sidecars.borrow_mut().insert(cache, v.clone());
        v
    }

    fn color(&self, key: &str) -> Option<u32> {
        self.colors.get(key).copied()
    }

    /// Whether the class names a text color of its own.
    pub fn has_text(&self, class: &str) -> bool {
        self.color(class).is_some()
    }

    /// Text color of `class` in `state` ("", "hover", "disabled", "dim").
    pub fn text(&self, class: &str, state: &str) -> u32 {
        let own = (!state.is_empty()).then(|| self.color(&format!("{class}_{state}"))).flatten();
        own.or_else(|| self.color(class)).or_else(|| self.color("default")).unwrap_or(0xFFFFFFFF)
    }

    /// Its text's shadow, `None` for none.
    pub fn shadow(&self, class: &str) -> Option<u32> {
        if self.nones.contains(&format!("{class}_shadow")) {
            return None;
        }
        self.color(&format!("{class}_shadow")).or_else(|| self.color("default_shadow"))
    }

    /// A named color (`color.<name>`), for what the engine draws itself.
    pub fn named(&self, name: &str, default: u32) -> u32 {
        self.color(name).unwrap_or(default)
    }
}
