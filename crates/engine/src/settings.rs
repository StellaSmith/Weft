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
            debug_groups: 0b11111,
            name: std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "Player".into()),
            binds: Vec::new(),
            relays: Vec::new(),
        };
        let Ok(src) = std::fs::read_to_string(&path) else { return s };
        for line in src.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "language" => s.language = v.to_string(),
                "gui_scale" => s.gui_scale = v.parse().unwrap_or(0),
                "view_distance" => s.view_distance = v.parse().unwrap_or(12),
                "vsync" => s.vsync = v == "true",
                "fov" => s.fov = v.parse().unwrap_or(70.0),
                "sensitivity" => s.sensitivity = v.parse().unwrap_or(1.0),
                "brightness" => s.brightness = v.parse().unwrap_or(0.5),
                "debug_groups" => s.debug_groups = v.parse().unwrap_or(0b11111),
                "name" => s.name = v.to_string(),
                "relay" => s.relays.push(v.to_string()),
                _ => {
                    if let Some(action) = k.strip_prefix("key.") {
                        s.binds.push((action.to_string(), v.to_string()));
                    }
                }
            }
        }
        s
    }

    pub fn saves_dir(&self) -> PathBuf {
        self.run_dir.join("saves")
    }

    pub fn save(&self) {
        let mut out = format!(
            "language = {}\ngui_scale = {}\nview_distance = {}\nvsync = {}\nfov = {}\nsensitivity = {}\nbrightness = {}\nname = {}\ndebug_groups = {}\n",
            self.language, self.gui_scale, self.view_distance, self.vsync, self.fov, self.sensitivity, self.brightness, self.name, self.debug_groups
        );
        for r in &self.relays {
            out.push_str(&format!("relay = {r}\n"));
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
    ALL.iter().find(|c| format!("{c:?}") == s).map(|c| Key::Code(*c))
}
