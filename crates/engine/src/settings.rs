//! The client's options as the engine reads them: typed views of its `core:` settings
//! (`config::engine_configs`), which live in `config/core.txt` like every mod's. Setting one
//! saves it; the client applies what changed by itself (`Running::apply_settings`).

use std::path::{Path, PathBuf};

use crate::config;
use crate::input::{Chord, Input, Key};

#[derive(Clone)]
pub struct Settings {
    pub run_dir: PathBuf,
    /// An automated run: it saves nothing and never takes the real mouse.
    pub read_only: bool,
}

/// The largest GUI scale a window shows whole (vanilla's 320x240 per step); auto uses it.
pub fn max_gui_scale(window: (u32, u32)) -> u32 {
    (window.0 / 320).min(window.1 / 240).max(1)
}

fn id(name: &str) -> u32 {
    config::by_name(name).unwrap_or(u32::MAX)
}

fn number(name: &str) -> f64 {
    config::get(id(name))
}

fn list(name: &str) -> Vec<String> {
    config::text(id(name)).split(',').map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect()
}

impl Settings {
    pub fn new(run_dir: &Path) -> Settings {
        Settings { run_dir: run_dir.to_path_buf(), read_only: false }
    }

    pub fn language(&self) -> String {
        config::text(id("core:language"))
    }
    /// 0 = automatic.
    pub fn gui_scale(&self) -> u32 {
        number("core:gui_scale") as u32
    }
    pub fn view_distance(&self) -> i32 {
        number("core:view_distance") as i32
    }
    pub fn lod_distance(&self) -> i32 {
        number("core:lod_distance") as i32
    }
    pub fn vsync(&self) -> bool {
        number("core:vsync") != 0.0
    }
    pub fn fov(&self) -> f32 {
        number("core:fov") as f32
    }
    pub fn sensitivity(&self) -> f32 {
        number("core:sensitivity") as f32
    }
    /// 0 moody .. 1 bright.
    pub fn brightness(&self) -> f32 {
        number("core:brightness") as f32
    }
    /// Bit per debug overlay group shown (F3 + number).
    pub fn debug_groups(&self) -> u32 {
        number("core:debug_groups") as u32
    }
    pub fn name(&self) -> String {
        config::text(id("core:name"))
    }
    /// Extra iroh relay URLs; empty uses the public defaults.
    pub fn relays(&self) -> Vec<String> {
        list("core:relays")
    }
    /// Enabled resource packs, highest priority first.
    pub fn resource_packs(&self) -> Vec<String> {
        list("core:resource_packs")
    }
    pub fn shader_pack(&self) -> Option<String> {
        Some(config::text(id("core:shader_pack"))).filter(|p| !p.is_empty())
    }

    pub fn set(&self, name: &str, v: f64) {
        config::set(id(&format!("core:{name}")), v);
    }
    pub fn set_text(&self, name: &str, v: &str) {
        config::set_text(id(&format!("core:{name}")), v);
    }
    pub fn set_resource_packs(&self, packs: &[String]) {
        self.set_text("resource_packs", &packs.join(","));
    }

    pub fn saves_dir(&self) -> PathBuf {
        self.run_dir.join("saves")
    }

    /// Every key as its `core:key.<action>` setting says.
    pub fn apply_binds(&self, input: &mut Input) {
        let actions: Vec<String> = input.binds.iter().map(|b| b.action.clone()).collect();
        for action in actions {
            match parse_chord(&config::text(id(&format!("core:key.{action}")))) {
                Some(k) => input.rebind(&action, k),
                None => log::warn!("key binding {action}: unknown key {}", config::text(id(&format!("core:key.{action}")))),
            }
        }
    }

    pub fn set_bind(&self, action: &str, key: Chord) {
        config::set_text(id(&format!("core:key.{action}")), &chord_id(key));
    }
}

/// The old `config/settings.txt` joins `config/core.txt` once (lists were one line per entry).
pub fn migrate(run_dir: &Path) {
    let (old, new) = (run_dir.join("config").join("settings.txt"), run_dir.join("config").join("core.txt"));
    let Ok(src) = std::fs::read_to_string(&old) else { return };
    let mut out = std::fs::read_to_string(&new).unwrap_or_default();
    let (mut packs, mut relays) = (Vec::new(), Vec::new());
    for (_, k, v) in crate::kv::pairs(&src) {
        match k {
            "resource_pack" => packs.push(v),
            "relay" => relays.push(v),
            _ => out.push_str(&format!("{k} = {v}
")),
        }
    }
    out.push_str(&format!("resource_packs = {}
relays = {}
", packs.join(","), relays.join(",")));
    if std::fs::write(&new, out).is_ok() {
        let _ = std::fs::remove_file(&old);
    }
}

pub fn system_language() -> String {
    let lang = std::env::var("LANG").unwrap_or_default().to_lowercase();
    if lang.starts_with("ru") || sys_locale_ru() { "ru_ru".into() } else { "en_us".into() }
}

#[cfg(windows)]
fn sys_locale_ru() -> bool {
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // Primary language id 0x19 is Russian.
    unsafe { GetUserDefaultUILanguage() & 0x3FF == 0x19 }
}

#[cfg(not(windows))]
fn sys_locale_ru() -> bool {
    false
}

pub fn key_id(k: Key) -> String {
    match k {
        Key::Mouse(b) => format!("mouse{b}"),
        Key::Code(c) => format!("{c:?}"),
    }
}

const MODIFIERS: [(u8, &str); 3] = [(crate::input::CTRL, "ctrl"), (crate::input::SHIFT, "shift"), (crate::input::ALT, "alt")];

/// `ctrl+shift+KeyT`: the modifiers, then the key.
pub fn chord_id(c: Chord) -> String {
    let mut out = String::new();
    for (bit, name) in MODIFIERS {
        if c.mods & bit != 0 {
            out += name;
            out.push('+');
        }
    }
    out + &key_id(c.key)
}

/// A key with modifiers in front (`ctrl+T`, `shift+alt+F6`), or a key alone.
pub fn parse_chord(s: &str) -> Option<Chord> {
    let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let key = parse_key(parts.pop()?)?;
    let mut mods = 0;
    for p in parts {
        mods |= MODIFIERS.iter().find(|m| m.1.eq_ignore_ascii_case(p) || (m.1 == "ctrl" && p.eq_ignore_ascii_case("control")))?.0;
    }
    Some(Chord { key, mods })
}

pub fn parse_key(s: &str) -> Option<Key> {
    if let Some(b) = s.strip_prefix("mouse") {
        return b.parse().ok().map(Key::Mouse);
    }
    use winit::keyboard::KeyCode::*;
    const ALL: &[winit::keyboard::KeyCode] = &[
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS,
        KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ, Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7,
        Digit8, Digit9, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, Space, Enter, Tab, Backspace, Escape,
        ShiftLeft, ShiftRight, ControlLeft, ControlRight, AltLeft, AltRight, ArrowUp, ArrowDown, ArrowLeft,
        ArrowRight, Minus, Equal, BracketLeft, BracketRight, Backslash, Semicolon, Quote, Comma, Period, Slash,
        Backquote, CapsLock, Insert, Delete, Home, End, PageUp, PageDown, Numpad0, Numpad1, Numpad2, Numpad3,
        Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9,
    ];
    // Short names ("R", "5", "space", "lshift") as well as winit's ("KeyR", "Digit5", "ShiftLeft").
    let short = match s.to_ascii_lowercase().as_str() {
        l if l.len() == 1 && l.as_bytes()[0].is_ascii_alphabetic() => format!("Key{}", l.to_ascii_uppercase()),
        l if l.len() == 1 && l.as_bytes()[0].is_ascii_digit() => format!("Digit{l}"),
        "lshift" | "shift" => "ShiftLeft".into(),
        "rshift" => "ShiftRight".into(),
        "lctrl" | "ctrl" | "control" => "ControlLeft".into(),
        "rctrl" => "ControlRight".into(),
        "lalt" | "alt" => "AltLeft".into(),
        "ralt" => "AltRight".into(),
        "esc" => "Escape".into(),
        "up" => "ArrowUp".into(),
        "down" => "ArrowDown".into(),
        "left" => "ArrowLeft".into(),
        "right" => "ArrowRight".into(),
        _ => s.into(),
    };
    ALL.iter().find(|c| format!("{c:?}").eq_ignore_ascii_case(&short)).map(|c| Key::Code(*c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    #[test]
    fn key_names() {
        assert_eq!(parse_key("R"), Some(Key::Code(KeyCode::KeyR)));
        assert_eq!(parse_key("KeyR"), Some(Key::Code(KeyCode::KeyR)));
        assert_eq!(parse_key("5"), Some(Key::Code(KeyCode::Digit5)));
        assert_eq!(parse_key("space"), Some(Key::Code(KeyCode::Space)));
        assert_eq!(parse_key("LShift"), Some(Key::Code(KeyCode::ShiftLeft)));
        assert_eq!(parse_key("F6"), Some(Key::Code(KeyCode::F6)));
        assert_eq!(parse_key("mouse3"), Some(Key::Mouse(3)));
        assert_eq!(parse_key("Rr"), None);
        assert_eq!(parse_chord("ctrl+T"), Some(Chord { key: Key::Code(KeyCode::KeyT), mods: crate::input::CTRL }));
        assert_eq!(parse_chord("Shift + Alt + F6"), Some(Chord { key: Key::Code(KeyCode::F6), mods: crate::input::SHIFT | crate::input::ALT }));
        assert_eq!(parse_chord("LShift"), Some(Chord { key: Key::Code(KeyCode::ShiftLeft), mods: 0 }));
        assert_eq!(parse_chord("meta+T"), None);
        let c = parse_chord("ctrl+shift+R").unwrap();
        assert_eq!(parse_chord(&chord_id(c)), Some(c));
    }
}
