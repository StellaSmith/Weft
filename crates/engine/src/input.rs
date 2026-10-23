//! Raw input plus named actions. Every action (engine or mod) is registered here with a default
//! key, so rebinding and conflict detection work on one table.

use glam::Vec2;
use rustc_hash::{FxHashMap, FxHashSet};
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Code(KeyCode),
    Mouse(u8),
}

pub struct Bind {
    pub action: String,
    pub default: Key,
    pub key: Key,
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
    repeated: FxHashSet<Key>,
    pub binds: Vec<Bind>,
    lookup: FxHashMap<String, Key>,
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
            repeated: FxHashSet::default(),
            binds: Vec::new(),
            lookup: FxHashMap::default(),
        };
        use KeyCode::*;
        for (name, key) in [
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
            ("core:attack", Key::Mouse(0)),
            ("core:use", Key::Mouse(1)),
            ("core:pick", Key::Mouse(2)),
            ("core:hotbar1", Key::Code(Digit1)),
            ("core:hotbar2", Key::Code(Digit2)),
            ("core:hotbar3", Key::Code(Digit3)),
            ("core:hotbar4", Key::Code(Digit4)),
            ("core:hotbar5", Key::Code(Digit5)),
            ("core:hotbar6", Key::Code(Digit6)),
            ("core:hotbar7", Key::Code(Digit7)),
            ("core:hotbar8", Key::Code(Digit8)),
            ("core:hotbar9", Key::Code(Digit9)),
        ] {
            i.register(name, key);
        }
        i
    }
}

impl Input {
    pub fn register(&mut self, action: &str, default: Key) {
        if self.lookup.contains_key(action) {
            return;
        }
        self.binds.push(Bind { action: action.to_string(), default, key: default });
        self.lookup.insert(action.to_string(), default);
    }

    pub fn rebind(&mut self, action: &str, key: Key) {
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

    pub fn key_of(&self, action: &str) -> Option<Key> {
        self.lookup.get(action).copied()
    }

    pub fn held(&self, action: &str) -> bool {
        self.lookup.get(action).is_some_and(|k| self.held.contains(k))
    }

    pub fn pressed(&self, action: &str) -> bool {
        self.lookup.get(action).is_some_and(|k| self.pressed.contains(k))
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
        self.held.remove(&k);
    }

    pub fn release_all(&mut self) {
        self.held.clear();
    }

    /// Clears per-frame state after the frame consumed it.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
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
