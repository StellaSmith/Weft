use crate::abi::{self, Str};
use crate::host;

/// A dimension as seen from a callback. Only valid for the duration of that callback.
#[derive(Clone, Copy)]
pub struct World {
    raw: *mut abi::WorldCtx,
}

impl World {
    pub(crate) fn from_raw(raw: *mut abi::WorldCtx) -> World {
        World { raw }
    }

    pub(crate) fn raw(&self) -> *mut abi::WorldCtx {
        self.raw
    }

    pub fn get(&self, pos: [i32; 3]) -> u32 {
        unsafe { (host().get_block)(self.raw, &pos) }
    }

    /// False if the position is not loaded.
    pub fn set(&self, pos: [i32; 3], state: u32) -> bool {
        unsafe { (host().set_block)(self.raw, &pos, state) != 0 }
    }

    /// A block of moving structure `entity` (in its own coordinates; block (0, 0, 0) is centered
    /// on the entity); `None` if the entity is not one.
    pub fn structure_block(&self, entity: u64, pos: [i32; 3]) -> Option<u32> {
        let s = unsafe { (host().structure_block)(self.raw, entity, &pos) };
        (s != u32::MAX).then_some(s)
    }

    /// Sets a block of moving structure `entity`, like `set` in the world.
    pub fn set_structure_block(&self, entity: u64, pos: [i32; 3], state: u32) -> bool {
        unsafe { (host().set_structure_block)(self.raw, entity, &pos, state) != 0 }
    }

    pub fn schedule(&self, pos: [i32; 3], delay_ticks: u32) {
        unsafe { (host().schedule_tick)(self.raw, &pos, delay_ticks) }
    }

    pub fn time(&self) -> u64 {
        unsafe { (host().world_time)(self.raw) }
    }

    pub fn random(&self) -> u64 {
        unsafe { (host().random)(self.raw) }
    }

    /// Uniform in `0..n`.
    pub fn random_below(&self, n: u32) -> u32 {
        ((self.random() >> 32) * n as u64 >> 32) as u32
    }

    pub fn flags(&self, pos: [i32; 3]) -> u32 {
        state_flags(self.get(pos))
    }

    /// The value of property `name` of the block at `pos`: `w.prop(pos, "lit")`.
    pub fn prop(&self, pos: [i32; 3], name: &str) -> Option<&'static str> {
        state_prop(self.get(pos), name)
    }

    /// Changes one property of the block at `pos`, keeping the others (its facing stays):
    /// `w.set_prop(pos, "lit", "true")`. False if the block has no such property or value.
    pub fn set_prop(&self, pos: [i32; 3], name: &str, value: &str) -> bool {
        match state_with(self.get(pos), name, value) {
            Some(s) => {
                self.set(pos, s);
                true
            }
            None => false,
        }
    }
}

/// The value of property `name` in `state` (`None` if its block has no such property).
pub fn state_prop(state: u32, name: &str) -> Option<&'static str> {
    let v = unsafe { (host().state_prop)(state, Str::new(name)).as_str() };
    (!v.is_empty()).then_some(v)
}

/// `state` with property `name` set to `value`, its other properties kept.
pub fn state_with(state: u32, name: &str, value: &str) -> Option<u32> {
    let s = unsafe { (host().state_with)(state, Str::new(name), Str::new(value)) };
    (s != u32::MAX).then_some(s)
}

pub fn state_flags(state: u32) -> u32 {
    unsafe { (host().state_flags)(state) }
}

pub fn block_of(state: u32) -> u32 {
    unsafe { (host().block_of)(state) }
}

/// Full name of a block state (`base:furnace[facing=north,lit=false]`), item, entity kind ...
/// by its id; `kind` is `abi::entry_kind`. Empty if unknown.
pub fn name_of(kind: u32, id: u32) -> &'static str {
    unsafe { (host().name_of)(kind, id).as_str() }
}

pub fn block_name(state: u32) -> &'static str {
    name_of(abi::entry_kind::BLOCK, state)
}

pub fn item_name(item: u32) -> &'static str {
    name_of(abi::entry_kind::ITEM, item)
}


