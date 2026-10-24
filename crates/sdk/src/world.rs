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
