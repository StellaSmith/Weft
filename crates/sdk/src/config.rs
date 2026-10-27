//! Settings: declared once, read with one call, edited by players in Settings > Mod Settings (a
//! screen built from the declarations). Where a value lives is its scope: the player's own
//! (`config/<ns>.txt`, the default), the server's (`.server()`, its players get it and every
//! change), or the world's (`.world()`, saved with it, changed with `/rule`: game rules).
//! The engine's own settings are the same thing, in `settings::*`.
//!
//! ```ignore
//! static MAX_WISPS: Config<i32> = Config::named("max_wisps");
//! r.config(&MAX_WISPS).int(8, 0, 64).server();
//! if count < MAX_WISPS.get() { .. }
//! ```
//! Names show as `config.<ns>.<key>` (tooltip `.tooltip`, choices `.<choice>`) from the language
//! files.

use std::marker::PhantomData;

use crate::abi::{self, Slice, Str, config_flags as cf, config_kind as ck, config_scope as scope};
use crate::content::{Handle, kind};
use crate::{Registry, host};

/// Types a setting can be read as.
pub trait ConfigValue: Copy {
    fn from_f64(v: f64) -> Self;
    fn to_f64(self) -> f64;
}

macro_rules! numbers {
    ($($t:ty),*) => {$(
        impl ConfigValue for $t {
            fn from_f64(v: f64) -> Self { v as $t }
            fn to_f64(self) -> f64 { self as f64 }
        }
    )*};
}
numbers!(i32, i64, u32, f32, f64);

impl ConfigValue for bool {
    fn from_f64(v: f64) -> Self {
        v != 0.0
    }
    fn to_f64(self) -> f64 {
        self as u8 as f64
    }
}

pub struct Config<T> {
    h: Handle<kind::Config>,
    _t: PhantomData<fn() -> T>,
}

impl<T: ConfigValue> Config<T> {
    /// This mod's setting (`"sparks"`) or another's (`"othermod:sparks"`).
    pub const fn named(name: &'static str) -> Self {
        Config { h: Handle::named(name), _t: PhantomData }
    }

    pub fn id(&self) -> u32 {
        self.h.id()
    }

    pub fn get(&self) -> T {
        T::from_f64(unsafe { (host().config_value)(self.h.id()) })
    }

    /// Changes and saves it (clamped to its range); whoever reads it sees the change.
    pub fn set(&self, v: T) {
        unsafe { (host().config_set)(self.h.id(), v.to_f64()) }
    }
}

/// A text setting (`ConfigBuilder::text`), the player's own.
pub struct TextConfig {
    h: Handle<kind::Config>,
}

impl TextConfig {
    pub const fn named(name: &'static str) -> Self {
        TextConfig { h: Handle::named(name) }
    }
    pub fn get(&self) -> String {
        unsafe { (host().config_text)(self.h.id()).as_str().to_string() }
    }
    pub fn set(&self, v: &str) {
        unsafe { (host().config_set_text)(self.h.id(), Str::new(v)) }
    }
}

/// The engine's settings, read and changed like a mod's; the client puts changes into effect
/// by itself.
pub mod settings {
    use super::{Config, TextConfig};
    pub static LANGUAGE: TextConfig = TextConfig::named("core:language");
    /// 0 = automatic.
    pub static GUI_SCALE: Config<u32> = Config::named("core:gui_scale");
    pub static VIEW_DISTANCE: Config<i32> = Config::named("core:view_distance");
    pub static VSYNC: Config<bool> = Config::named("core:vsync");
    pub static FOV: Config<f32> = Config::named("core:fov");
    pub static SENSITIVITY: Config<f32> = Config::named("core:sensitivity");
    pub static BRIGHTNESS: Config<f32> = Config::named("core:brightness");
    /// Bit per F3 overlay group shown.
    pub static DEBUG_GROUPS: Config<u32> = Config::named("core:debug_groups");
    pub static NAME: TextConfig = TextConfig::named("core:name");
    /// Enabled resource packs, highest priority first, comma separated.
    pub static RESOURCE_PACKS: TextConfig = TextConfig::named("core:resource_packs");
    pub static SHADER_PACK: TextConfig = TextConfig::named("core:shader_pack");
    pub static VOLUME_MASTER: Config<u32> = Config::named("core:volume_master");
    /// A world's: mobs appear by themselves.
    pub static MOB_SPAWNING: Config<bool> = Config::named("core:mob_spawning");
}

pub struct ConfigBuilder<'a> {
    raw: *mut abi::Registrar,
    name: String,
    choices: Vec<String>,
    text: String,
    cell: *const std::sync::atomic::AtomicU32,
    desc: abi::ConfigDesc,
    _p: PhantomData<&'a mut Registry>,
}

impl ConfigBuilder<'_> {
    pub fn bool(mut self, default: bool) -> Self {
        (self.desc.kind, self.desc.default) = (ck::BOOL, default as u8 as f64);
        self
    }
    pub fn int(mut self, default: i64, min: i64, max: i64) -> Self {
        (self.desc.kind, self.desc.default, self.desc.min, self.desc.max, self.desc.step) = (ck::INT, default as f64, min as f64, max as f64, 1.0);
        self
    }
    pub fn float(mut self, default: f64, min: f64, max: f64) -> Self {
        (self.desc.kind, self.desc.default, self.desc.min, self.desc.max) = (ck::FLOAT, default, min, max);
        self
    }
    /// Values the slider snaps to.
    pub fn step(mut self, step: f64) -> Self {
        self.desc.step = step;
        self
    }
    /// One of `choices`, read as its index.
    pub fn choice(mut self, default: u32, choices: &[&str]) -> Self {
        (self.desc.kind, self.desc.default) = (ck::CHOICE, default as f64);
        self.choices = choices.iter().map(|c| c.to_string()).collect();
        self
    }
    /// Text, read with a `TextConfig`; the player's own.
    pub fn text(mut self, default: &str) -> Self {
        (self.desc.kind, self.text) = (ck::TEXT, default.to_string());
        self
    }
    /// The server's value counts for everyone on it, and its changes reach them.
    pub fn server(mut self) -> Self {
        self.desc.scope = scope::SERVER;
        self
    }
    /// Saved with the world and changed there with `/rule` (vanilla's game rules); its players
    /// get it like the server's.
    pub fn world(mut self) -> Self {
        self.desc.scope = scope::WORLD;
        self
    }
    /// Only read at start: the screen says a restart is needed.
    pub fn restart_required(mut self) -> Self {
        self.desc.flags |= cf::RESTART;
        self
    }
}

impl Drop for ConfigBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let choices: Vec<Str> = self.choices.iter().map(|c| Str::new(c)).collect();
        let desc = abi::ConfigDesc { name: Str::new(&self.name), cell: self.cell, choices: Slice::new(&choices), text: Str::new(&self.text), ..self.desc };
        unsafe { (host().reg_config)(self.raw, &desc) }
    }
}

impl Registry {
    /// A setting, see `ConfigBuilder` (a bool `false` unless told otherwise).
    pub fn config<'a, T: ConfigValue>(&'a mut self, config: &'static Config<T>) -> ConfigBuilder<'a> {
        ConfigBuilder { raw: self.raw, name: config.h.name().to_string(), choices: Vec::new(), text: String::new(), cell: config.h.cell(), desc: abi::ConfigDesc::DEFAULT, _p: PhantomData }
    }

    /// A text setting, see `ConfigBuilder::text`.
    pub fn text_config<'a>(&'a mut self, config: &'static TextConfig, default: &str) -> ConfigBuilder<'a> {
        ConfigBuilder { raw: self.raw, name: config.h.name().to_string(), choices: Vec::new(), text: String::new(), cell: config.h.cell(), desc: abi::ConfigDesc::DEFAULT, _p: PhantomData }.text(default)
    }
}
