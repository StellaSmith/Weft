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
}

pub fn state_flags(state: u32) -> u32 {
    unsafe { (host().state_flags)(state) }
}

pub fn find_block(name: &str) -> Option<u32> {
    let id = unsafe { (host().find)(abi::entry_kind::BLOCK, Str::new(name), std::ptr::null_mut()) };
    (id != u32::MAX).then_some(id)
}

pub fn find_item(name: &str) -> Option<u32> {
    let id = unsafe { (host().find)(abi::entry_kind::ITEM, Str::new(name), std::ptr::null_mut()) };
    (id != u32::MAX).then_some(id)
}

pub fn block_of(state: u32) -> u32 {
    unsafe { (host().block_of)(state) }
}

/// Full name of a block state (`base:furnace#4` for a state past the first), item, entity kind ...
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

pub fn find_entity(name: &str) -> Option<u32> {
    let id = unsafe { (host().find)(abi::entry_kind::ENTITY, Str::new(name), std::ptr::null_mut()) };
    (id != u32::MAX).then_some(id)
}

pub fn find_menu(name: &str) -> Option<u32> {
    let id = unsafe { (host().find)(abi::entry_kind::MENU, Str::new(name), std::ptr::null_mut()) };
    (id != u32::MAX).then_some(id)
}

pub fn find_dimension(name: &str) -> Option<u32> {
    let id = unsafe { (host().find)(abi::entry_kind::DIMENSION, Str::new(name), std::ptr::null_mut()) };
    (id != u32::MAX).then_some(id)
}

impl World {
    /// Current value of a world rule (`Registry::rule`), by full name.
    pub fn rule(&self, name: &str) -> i64 {
        unsafe { (host().rule)(self.raw, Str::new(name)) }
    }
}

impl crate::Registry {
    /// A world rule players change with `/rule <name> <value>` (saved with the world), like
    /// vanilla's game rules: `r.rule("mob_spawning", 1)`, read with `w.rule("base:mob_spawning")`.
    pub fn rule(&mut self, name: &str, default: i64) {
        let name = crate::qualify(self.ns, name);
        unsafe { (host().reg_rule)(self.raw, Str::new(&name), default) }
    }
}
