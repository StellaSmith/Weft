//! Mod settings: declared once, edited by players in Settings > Mod Settings (a screen built from
//! the declarations), saved in `config/<ns>.txt`, read with one call.
//!
//! ```ignore
//! static MAX_WISPS: Config<i32> = Config::named("max_wisps");
//! r.config(&MAX_WISPS).int(8, 0, 64).synced();
//! if count < MAX_WISPS.get() { .. }
//! ```
//! Names show as `config.<ns>.<key>` (tooltip `.tooltip`, choices `.<choice>`) from the language
//! files.

use std::marker::PhantomData;

use crate::abi::{self, Slice, Str, config_flags as cf, config_kind as ck};
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

    /// Changes and saves it (clamped to its range).
    pub fn set(&self, v: T) {
        unsafe { (host().config_set)(self.h.id(), v.to_f64()) }
    }
}

pub struct ConfigBuilder<'a> {
    raw: *mut abi::Registrar,
    name: String,
    choices: Vec<String>,
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
    /// The server's value counts for everyone on it.
    pub fn synced(mut self) -> Self {
        self.desc.flags |= cf::SYNCED;
        self
    }
    /// Only read at start: the screen says a restart is needed.
    pub fn restart(mut self) -> Self {
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
        let desc = abi::ConfigDesc { name: Str::new(&self.name), cell: self.cell, choices: Slice::new(&choices), ..self.desc };
        unsafe { (host().reg_config)(self.raw, &desc) }
    }
}

impl Registry {
    /// A setting, see `ConfigBuilder` (a bool `false` unless told otherwise).
    pub fn config<'a, T: ConfigValue>(&'a mut self, config: &'static Config<T>) -> ConfigBuilder<'a> {
        ConfigBuilder { raw: self.raw, name: config.h.name().to_string(), choices: Vec::new(), cell: config.h.cell(), desc: abi::ConfigDesc::DEFAULT, _p: PhantomData }
    }
}
