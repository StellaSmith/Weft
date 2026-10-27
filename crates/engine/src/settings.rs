use std::path::{Path, PathBuf};

use crate::input::{Input, Key};

#[derive(Clone)]
pub struct Settings {
    pub run_dir: PathBuf,
    pub path: PathBuf,
    pub language: String,
    /// 0 = automatic.
    pub gui_scale: u32,
    pub view_distance: i32,
    pub vsync: bool,
    pub fov: f32,
    pub sensitivity: f32,
    /// 0 moody .. 1 bright.
    pub brightness: f32,
    /// Bit per debug overlay group shown (F3 + number).
    pub debug_groups: u32,
    pub name: String,
    /// action -> key name, only for actions the player rebound.
    pub binds: Vec<(String, String)>,
    /// Extra iroh relay URLs; empty uses the public defaults.
    pub relays: Vec<String>,
    /// Enabled resource packs, highest priority first.
    pub resource_packs: Vec<String>,
    pub shader_pack: Option<String>,
    /// Command line values for this run only, with the file's: (given, saved). Saving keeps the
    /// file's value unless the player changed the setting since.
    pub cli_vsync: Option<(bool, bool)>,
    pub cli_view: Option<(i32, i32)>,
    pub read_only: bool,
}

/// The largest GUI scale a window shows whole (vanilla's 320x240 per step); auto uses it.
pub fn max_gui_scale(window: (u32, u32)) -> u32 {
    (window.0 / 320).min(window.1 / 240).max(1)
}

impl Settings {
    pub fn load(run_dir: &Path) -> Settings {
        let path = run_dir.join("config").join("settings.txt");
        let mut s = Settings {
            run_dir: run_dir.to_path_buf(),
            path: path.clone(),
            language: system_language(),
            gui_scale: 0,
            view_distance: 12,
            vsync: true,
            fov: 70.0,
            sensitivity: 1.0,
            brightness: 0.5,
            debug_groups: u32::MAX,
            name: std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "Player".into()),
            binds: Vec::new(),
            relays: Vec::new(),
            resource_packs: Vec::new(),
            shader_pack: None,
            cli_vsync: None,
            cli_view: None,
            read_only: false,
        };
        let Ok(src) = std::fs::read_to_string(&path) else { return s };
        for (_, k, v) in crate::kv::pairs(&src) {
            if !s.set(k, v) {
                if let Some(action) = k.strip_prefix("key.") {
                    s.binds.push((action.to_string(), v.to_string()));
                }
            }
        }
        s
    }

    /// One `key = value` line of settings.txt; false for keys it does not know.
    pub fn set(&mut self, k: &str, v: &str) -> bool {
        match k {
            "language" => self.language = v.to_string(),
            "gui_scale" => self.gui_scale = v.parse().unwrap_or(0),
            "view_distance" => self.view_distance = v.parse().unwrap_or(12),
            "vsync" => self.vsync = v == "true",
            "fov" => self.fov = v.parse().unwrap_or(70.0),
            "sensitivity" => self.sensitivity = v.parse().unwrap_or(1.0),
            "brightness" => self.brightness = v.parse().unwrap_or(0.5),
            "debug_groups" => self.debug_groups = v.parse().unwrap_or(u32::MAX),
            "name" => self.name = v.to_string(),
            "relay" => self.relays.push(v.to_string()),
            "resource_pack" => self.resource_packs.push(v.to_string()),
            "resource_packs" => self.resource_packs = v.split(',').map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect(),
            "shader_pack" => self.shader_pack = Some(v.to_string()).filter(|v| !v.is_empty()),
            _ => return false,
        }
        true
    }

    /// The value `set` takes for `k`, as text.
    pub fn get(&self, k: &str) -> Option<String> {
        Some(match k {
            "language" => self.language.clone(),
            "gui_scale" => self.gui_scale.to_string(),
            "view_distance" => self.view_distance.to_string(),
            "vsync" => self.vsync.to_string(),
            "fov" => self.fov.to_string(),
            "sensitivity" => self.sensitivity.to_string(),
            "brightness" => self.brightness.to_string(),
            "debug_groups" => self.debug_groups.to_string(),
            "name" => self.name.clone(),
            "resource_packs" => self.resource_packs.join(","),
            "shader_pack" => self.shader_pack.clone().unwrap_or_default(),
            _ => return None,
        })
    }

    pub fn saves_dir(&self) -> PathBuf {
        self.run_dir.join("saves")
    }

    pub fn save(&self) {
        // Automated runs change settings for themselves only.
        if self.read_only {
            return;
        }
        let vsync = match self.cli_vsync {
            Some((given, saved)) if self.vsync == given => saved,
            _ => self.vsync,
        };
        let view = match self.cli_view {
            Some((given, saved)) if self.view_distance == given => saved,
            _ => self.view_distance,
        };
        let mut out = format!(
            "language = {}\ngui_scale = {}\nview_distance = {}\nvsync = {}\nfov = {}\nsensitivity = {}\nbrightness = {}\nname = {}\ndebug_groups = {}\n",
            self.language, self.gui_scale, view, vsync, self.fov, self.sensitivity, self.brightness, self.name, self.debug_groups
        );
        for r in &self.relays {
            out.push_str(&format!("relay = {r}\n"));
        }
        for p in &self.resource_packs {
            out.push_str(&format!("resource_pack = {p}\n"));
        }
        if let Some(p) = &self.shader_pack {
            out.push_str(&format!("shader_pack = {p}\n"));
        }
        for (a, k) in &self.binds {
            out.push_str(&format!("key.{a} = {k}\n"));
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&self.path, out) {
            log::error!("saving settings: {e}");
        }
    }

    pub fn apply_binds(&self, input: &mut Input) {
        for (action, key) in &self.binds {
            if let Some(k) = parse_key(key) {
                input.rebind(action, k);
            }
        }
    }

    pub fn set_bind(&mut self, action: &str, key: Key) {
        self.binds.retain(|(a, _)| a != action);
        self.binds.push((action.to_string(), key_id(key)));
    }
}

fn system_language() -> String {
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
    }
}
