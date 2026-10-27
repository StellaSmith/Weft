//! Raw input plus named actions. Every action (engine or mod) is registered here with a default
//! key, so rebinding and conflict detection work on one table.

use glam::Vec2;
use rustc_hash::{FxHashMap, FxHashSet};
use winit::keyboard::KeyCode;

/// The hotbar slot actions, first to ninth.
pub const HOTBAR: [&str; 9] = ["core:hotbar1", "core:hotbar2", "core:hotbar3", "core:hotbar4", "core:hotbar5", "core:hotbar6", "core:hotbar7", "core:hotbar8", "core:hotbar9"];

/// The engine's actions and their default keys.
pub const ENGINE_KEYS: &[(&str, Key)] = {
    use KeyCode::*;
    &[
        ("core:forward", Key::Code(KeyW)),
        ("core:back", Key::Code(KeyS)),
        ("core:left", Key::Code(KeyA)),
        ("core:right", Key::Code(KeyD)),
        ("core:jump", Key::Code(Space)),
        ("core:sneak", Key::Code(ShiftLeft)),
        ("core:sprint", Key::Code(ControlLeft)),
        ("core:fly", Key::Code(KeyF)),
        ("core:inventory", Key::Code(KeyE)),
        ("core:drop", Key::Code(KeyQ)),
        ("core:command", Key::Code(Slash)),
        ("core:chat", Key::Code(KeyT)),
        ("core:debug", Key::Code(F3)),
        ("core:screenshot", Key::Code(F2)),
        ("core:perspective", Key::Code(F5)),
        // Also F3+T, like vanilla.
        ("core:reload", Key::Code(F8)),
        ("core:recipes", Key::Code(KeyR)),
        ("core:uses", Key::Code(KeyU)),
        ("core:attack", Key::Mouse(0)),
        ("core:use", Key::Mouse(1)),
        ("core:pick", Key::Mouse(2)),
        (HOTBAR[0], Key::Code(Digit1)),
        (HOTBAR[1], Key::Code(Digit2)),
        (HOTBAR[2], Key::Code(Digit3)),
        (HOTBAR[3], Key::Code(Digit4)),
        (HOTBAR[4], Key::Code(Digit5)),
        (HOTBAR[5], Key::Code(Digit6)),
        (HOTBAR[6], Key::Code(Digit7)),
        (HOTBAR[7], Key::Code(Digit8)),
        (HOTBAR[8], Key::Code(Digit9)),
    ]
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Code(KeyCode),
    Mouse(u8),
}

/// Modifier bits of a `Chord`.
pub const CTRL: u8 = 1;
pub const SHIFT: u8 = 2;
pub const ALT: u8 = 4;

/// A key and the modifiers held with it (`ctrl+T`). A modifier key is also a key of its own
/// (sneak on Shift).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub mods: u8,
}

impl From<Key> for Chord {
    fn from(key: Key) -> Chord {
        Chord { key, mods: 0 }
    }
}

/// The `Chord` bit of a modifier key, 0 for others.
pub fn modifier(k: Key) -> u8 {
    match k {
        Key::Code(KeyCode::ControlLeft | KeyCode::ControlRight) => CTRL,
        Key::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight) => SHIFT,
        Key::Code(KeyCode::AltLeft | KeyCode::AltRight) => ALT,
        _ => 0,
    }
}

pub struct Bind {
    pub action: String,
    pub default: Chord,
    pub key: Chord,
}

pub struct Input {
    pub captured: bool,
    pub mouse_delta: Vec2,
    pub cursor: Vec2,
    pub scroll: f32,
    pub sensitivity: f32,
    pub typed: String,
    held: FxHashSet<Key>,
    pressed: FxHashSet<Key>,
    released: FxHashSet<Key>,
    repeated: FxHashSet<Key>,
    pub binds: Vec<Bind>,
    lookup: FxHashMap<String, Chord>,
}

impl Default for Input {
    fn default() -> Self {
        let mut i = Input {
            captured: false,
            mouse_delta: Vec2::ZERO,
            cursor: Vec2::ZERO,
            scroll: 0.0,
            sensitivity: 1.0,
            typed: String::new(),
            held: FxHashSet::default(),
            pressed: FxHashSet::default(),
            released: FxHashSet::default(),
            repeated: FxHashSet::default(),
            binds: Vec::new(),
            lookup: FxHashMap::default(),
        };
        for &(name, key) in ENGINE_KEYS {
            i.register(name, key.into());
        }
        i
    }
}

impl Input {
    pub fn register(&mut self, action: &str, default: Chord) {
        if self.lookup.contains_key(action) {
            return;
        }
        self.binds.push(Bind { action: action.to_string(), default, key: default });
        self.lookup.insert(action.to_string(), default);
    }

    pub fn rebind(&mut self, action: &str, key: Chord) {
        if let Some(b) = self.binds.iter_mut().find(|b| b.action == action) {
            b.key = key;
            self.lookup.insert(action.to_string(), key);
        }
    }

    /// Actions sharing a key with another action.
    pub fn conflicts(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (i, a) in self.binds.iter().enumerate() {
            for b in &self.binds[i + 1..] {
                if a.key == b.key {
                    out.push((a.action.clone(), b.action.clone()));
                }
            }
        }
        out
    }

    pub fn key_of(&self, action: &str) -> Option<Chord> {
        self.lookup.get(action).copied()
    }

    pub fn held(&self, action: &str) -> bool {
        self.lookup.get(action).is_some_and(|&c| self.active(c, &self.held))
    }

    pub fn pressed(&self, action: &str) -> bool {
        self.lookup.get(action).is_some_and(|&c| self.active(c, &self.pressed))
    }

    /// Modifiers held now (`Chord` bits).
    pub fn mods(&self) -> u8 {
        self.held.iter().fold(0, |m, &k| m | modifier(k))
    }

    /// The chord is down: its key in `keys`, its modifiers held, and no binding of the same key
    /// with more modifiers, all held, takes it over (`ctrl+T` rather than `T`).
    fn active(&self, c: Chord, keys: &FxHashSet<Key>) -> bool {
        if !keys.contains(&c.key) {
            return false;
        }
        let held = self.mods();
        held & c.mods == c.mods && !self.binds.iter().any(|b| b.key.key == c.key && b.key.mods != c.mods && b.key.mods & c.mods == c.mods && held & b.key.mods == b.key.mods)
    }

    pub fn held_key(&self, k: Key) -> bool {
        self.held.contains(&k)
    }

    pub fn pressed_key(&self, k: Key) -> bool {
        self.pressed.contains(&k)
    }

    pub fn pressed_list(&self) -> Vec<Key> {
        self.pressed.iter().copied().collect()
    }

    /// Keys let go of this frame.
    pub fn released_list(&self) -> Vec<Key> {
        self.released.iter().copied().collect()
    }

    pub fn held_button(&self, b: u8) -> bool {
        self.held.contains(&Key::Mouse(b))
    }

    pub fn pressed_button(&self, b: u8) -> bool {
        self.pressed.contains(&Key::Mouse(b))
    }

    /// Pressed this frame, or held long enough for the system's key repeat to fire.
    pub fn pressed_or_repeated(&self, k: Key) -> bool {
        self.pressed.contains(&k) || self.repeated.contains(&k)
    }

    pub fn key_repeat(&mut self, k: Key) {
        self.repeated.insert(k);
    }

    pub fn key_down(&mut self, k: Key) {
        if self.held.insert(k) {
            self.pressed.insert(k);
        }
    }

    pub fn key_up(&mut self, k: Key) {
        if self.held.remove(&k) {
            self.released.insert(k);
        }
    }

    pub fn release_all(&mut self) {
        self.held.clear();
    }

    /// Clears per-frame state after the frame consumed it.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.repeated.clear();
        self.mouse_delta = Vec2::ZERO;
        self.scroll = 0.0;
        self.typed.clear();
    }
}

pub fn key_name(k: Key) -> String {
    match k {
        Key::Mouse(0) => "LMB".into(),
        Key::Mouse(1) => "RMB".into(),
        Key::Mouse(2) => "MMB".into(),
        Key::Mouse(n) => format!("Mouse{}", n + 1),
        Key::Code(c) => {
            let s = format!("{c:?}");
            s.strip_prefix("Key").or_else(|| s.strip_prefix("Digit")).unwrap_or(&s).to_string()
        }
    }
}
