use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::abi::{self, ItemStack, Slice, Str};
use crate::ui::{Ui, ui_callback};
use crate::{Registry, World, callback, host, qualify};

pub use abi::{EntityState, component_flags, damage, entity_flags, part_role, slot_flags};

/// Typed handle to a component; the value type must be plain old data (`Copy`, `repr(C)`).
pub struct Component<T: Copy> {
    id: AtomicU32,
    _p: PhantomData<fn() -> T>,
}

impl<T: Copy> Component<T> {
    pub const fn new() -> Self {
        Component { id: AtomicU32::new(u32::MAX), _p: PhantomData }
    }
    pub fn id(&self) -> u32 {
        self.id.load(Ordering::Relaxed)
    }
}

impl<T: Copy> Default for Component<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub type Entity = crate::Handle;
pub type Menu = crate::Handle;

// ---------------------------------------------------------------- world helpers

impl World {
    pub fn light(&self, pos: [i32; 3]) -> u32 {
        unsafe { (host().light)(self.raw(), &pos) }
    }
    /// Sky light level at `pos` (0..15).
    pub fn sky_light(&self, pos: [i32; 3]) -> u32 {
        self.light(pos) & 0xFF
    }
    /// Strongest block light channel at `pos`.
    pub fn block_light(&self, pos: [i32; 3]) -> u32 {
        let l = self.light(pos);
        ((l >> 8) & 0xFF).max((l >> 16) & 0xFF).max((l >> 24) & 0xFF)
    }
    pub fn daylight(&self) -> f32 {
        unsafe { (host().daylight)(self.raw()) }
    }
    pub fn spawn(&self, kind: &Entity, pos: [f64; 3]) -> u64 {
        unsafe { (host().entity_spawn)(self.raw(), kind.id(), &pos) }
    }
    pub fn remove(&self, id: u64) {
        unsafe { (host().entity_remove)(self.raw(), id) }
    }
    pub fn entity(&self, id: u64) -> Option<EntityState> {
        let mut s = EntityState::default();
        (unsafe { (host().entity_get)(self.raw(), id, &mut s) } != 0).then_some(s)
    }
    pub fn set_entity(&self, id: u64, s: &EntityState) {
        unsafe { (host().entity_set)(self.raw(), id, s) }
    }
    /// Steer an AI entity this tick: direction on the ground plane, speed factor, jump.
    pub fn walk(&self, id: u64, dir: [f32; 2], speed: f32, jump: bool) {
        unsafe { (host().entity_walk)(self.raw(), id, &dir, speed, jump as u32) }
    }
    pub fn damage(&self, id: u64, amount: f32, source: u64, cause: u32) -> bool {
        unsafe { (host().entity_damage)(self.raw(), id, amount, source, cause) != 0 }
    }
    /// Entity ids within `radius`; `kind` None = any kind.
    pub fn near(&self, pos: [f64; 3], radius: f64, kind: Option<&Entity>) -> Vec<u64> {
        let k = kind.map_or(u32::MAX, |k| k.id());
        let mut buf = vec![0u64; 64];
        let n = unsafe { (host().entities_near)(self.raw(), &pos, radius, k, buf.as_mut_ptr(), buf.len()) };
        if n > buf.len() {
            buf.resize(n, 0);
            unsafe { (host().entities_near)(self.raw(), &pos, radius, k, buf.as_mut_ptr(), buf.len()) };
        }
        buf.truncate(n);
        buf
    }
    pub fn component<T: Copy>(&self, id: u64, c: &Component<T>) -> Option<&mut T> {
        let p = unsafe { (host().component)(self.raw(), id, c.id()) };
        (!p.is_null()).then(|| unsafe { &mut *(p as *mut T) })
    }
    pub fn players(&self) -> Vec<u64> {
        let n = unsafe { (host().player_count)(self.raw()) };
        (0..n).map(|i| unsafe { (host().player_at)(self.raw(), i) }).collect()
    }
    pub fn inventory(&self, owner: u64) -> Inventory {
        Inventory { world: *self, owner }
    }
    pub fn selected_slot(&self, player: u64) -> u32 {
        unsafe { (host().selected_slot)(self.raw(), player) }
    }
    pub fn spawn_item(&self, pos: [f64; 3], stack: ItemStack) {
        unsafe { (host().spawn_item)(self.raw(), &pos, stack) }
    }
    /// Entity attached to the block at `pos` (furnace, chest), 0 if none.
    pub fn block_entity(&self, pos: [i32; 3]) -> u64 {
        unsafe { (host().block_entity)(self.raw(), &pos) }
    }
    pub fn open_menu(&self, player: u64, menu: &Menu, owner: u64) {
        unsafe { (host().open_menu)(self.raw(), player, menu.id(), owner) }
    }
}

#[derive(Clone, Copy)]
pub struct Inventory {
    world: World,
    owner: u64,
}

impl Inventory {
    pub fn size(&self) -> u32 {
        unsafe { (host().inv_size)(self.world.raw(), self.owner) }
    }
    pub fn get(&self, slot: u32) -> ItemStack {
        unsafe { (host().inv_get)(self.world.raw(), self.owner, slot) }
    }
    pub fn set(&self, slot: u32, stack: ItemStack) {
        unsafe { (host().inv_set)(self.world.raw(), self.owner, slot, stack) }
    }
    /// Returns what did not fit.
    pub fn insert(&self, stack: ItemStack) -> ItemStack {
        unsafe { (host().inv_insert)(self.world.raw(), self.owner, stack) }
    }
}

// ---------------------------------------------------------------- item queries

pub fn item_prop<T: Copy>(item: u32, key: &str) -> Option<T> {
    let mut len = 0usize;
    let p = unsafe { (host().item_prop)(item, Str::new(key), &mut len) };
    (!p.is_null() && len >= size_of::<T>()).then(|| unsafe { std::ptr::read_unaligned(p as *const T) })
}

/// (max stack, durability, placed block state or 0).
pub fn item_info(item: u32) -> (u32, u32, u32) {
    let (mut m, mut d) = (0, 0);
    let b = unsafe { (host().item_info)(item, &mut m, &mut d) };
    (m, d, b)
}

pub fn item_has_tag(item: u32, tag: &str) -> bool {
    unsafe { (host().item_has_tag)(item, Str::new(tag)) != 0 }
}

pub fn recipe_match(kind: &str, grid: &[ItemStack], width: u32, height: u32) -> ItemStack {
    unsafe { (host().recipe_match)(Str::new(kind), grid.as_ptr(), width, height) }
}

/// Output and time of a processing recipe (smelting) for `input`.
pub fn recipe_process(kind: &str, input: u32) -> Option<(ItemStack, u32)> {
    let mut t = 0;
    let s = unsafe { (host().recipe_process)(Str::new(kind), input, &mut t) };
    (!s.is_empty()).then_some((s, t))
}

// ---------------------------------------------------------------- registration

pub struct EntityTick {
    args: abi::EntityTickArgs,
}

impl EntityTick {
    pub fn world(&self) -> World {
        World::from_raw(self.args.world)
    }
    pub fn ids(&self) -> &[u64] {
        unsafe { std::slice::from_raw_parts(self.args.ids, self.args.count) }
    }
}

pub struct EntityEvent {
    args: abi::EntityEventArgs,
}

impl EntityEvent {
    pub fn world(&self) -> World {
        World::from_raw(self.args.world)
    }
    pub fn entity(&self) -> u64 {
        self.args.entity
    }
    pub fn other(&self) -> u64 {
        self.args.other
    }
    pub fn amount(&self) -> f32 {
        self.args.amount
    }
    pub fn set_amount(&mut self, a: f32) {
        self.args.amount = a;
    }
    pub fn cause(&self) -> u32 {
        self.args.cause
    }
}

fn tick_cb(f: impl Fn(&EntityTick) + Send + Sync + 'static) -> abi::Callback {
    callback(move |a: &mut abi::EntityTickArgs| f(unsafe { &*(a as *const abi::EntityTickArgs as *const EntityTick) }))
}

fn event_cb(f: impl Fn(&mut EntityEvent) + Send + Sync + 'static) -> abi::Callback {
    callback(move |a: &mut abi::EntityEventArgs| f(unsafe { &mut *(a as *mut abi::EntityEventArgs as *mut EntityEvent) }))
}

pub enum Hook {
    Tick,
    Hurt,
    Death,
    Interact,
}

/// A model part: pivot, animation role, boxes.
pub struct Part {
    pub pivot: [f32; 3],
    pub role: u32,
    pub boxes: Vec<abi::ModelCuboid>,
}

impl Part {
    pub fn new(pivot: [f32; 3], role: u32) -> Part {
        Part { pivot, role, boxes: Vec::new() }
    }
    /// Box relative to the pivot in model pixels, `uv` = top-left texel of its unwrap.
    pub fn cube(mut self, origin: [f32; 3], size: [f32; 3], uv: [f32; 2]) -> Part {
        self.boxes.push(abi::ModelCuboid { origin, size, uv, inflate: 0.0 });
        self
    }
}

/// Classic humanoid on a 64x32 skin; `arms_forward` for zombies.
pub fn biped(arms_forward: bool) -> Vec<Part> {
    let (l, r) = if arms_forward { (part_role::ARM_FORWARD_LEFT, part_role::ARM_FORWARD_RIGHT) } else { (part_role::ARM_LEFT, part_role::ARM_RIGHT) };
    vec![
        Part::new([0.0, 24.0, 0.0], part_role::HEAD).cube([-4.0, 0.0, -4.0], [8.0, 8.0, 8.0], [0.0, 0.0]),
        Part::new([0.0, 24.0, 0.0], part_role::STATIC).cube([-4.0, -12.0, -2.0], [8.0, 12.0, 4.0], [16.0, 16.0]),
        Part::new([-6.0, 22.0, 0.0], l).cube([-2.0, -10.0, -2.0], [4.0, 12.0, 4.0], [40.0, 16.0]),
        Part::new([6.0, 22.0, 0.0], r).cube([-2.0, -10.0, -2.0], [4.0, 12.0, 4.0], [40.0, 16.0]),
        Part::new([-2.0, 12.0, 0.0], part_role::LEG_LEFT).cube([-2.0, -12.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0]),
        Part::new([2.0, 12.0, 0.0], part_role::LEG_RIGHT).cube([-2.0, -12.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0]),
    ]
}

/// Four-legged animal: body (width, height, length), leg height and head size in model pixels.
/// Skin is 64x64: head at (0,0), legs at (0,16), body at (0,32).
pub fn quadruped(body: [f32; 3], leg: f32, head: [f32; 3]) -> Vec<Part> {
    let [bw, bh, bl] = body;
    let y = leg;
    let legs = [
        ([-bw / 2.0 + 2.0, y, -bl / 2.0 + 2.0], part_role::LEG_FRONT_LEFT),
        ([bw / 2.0 - 2.0, y, -bl / 2.0 + 2.0], part_role::LEG_FRONT_RIGHT),
        ([-bw / 2.0 + 2.0, y, bl / 2.0 - 2.0], part_role::LEG_RIGHT),
        ([bw / 2.0 - 2.0, y, bl / 2.0 - 2.0], part_role::LEG_LEFT),
    ];
    let mut parts = vec![
        Part::new([0.0, y, 0.0], part_role::STATIC).cube([-bw / 2.0, 0.0, -bl / 2.0], [bw, bh, bl], [0.0, 32.0]),
        Part::new([0.0, y + bh * 0.75, -bl / 2.0], part_role::HEAD).cube([-head[0] / 2.0, -head[1] / 2.0, -head[2]], head, [0.0, 0.0]),
    ];
    for (pivot, role) in legs {
        parts.push(Part::new(pivot, role).cube([-2.0, -leg, -2.0], [4.0, leg, 4.0], [0.0, 16.0]));
    }
    parts
}

pub struct EntityBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    handle: &'static Entity,
    width: f32,
    height: f32,
    eye: f32,
    health: f32,
    speed: f32,
    flags: u32,
    category: u32,
    texture: String,
    texture_size: [u32; 2],
    parts: Vec<Part>,
    on_tick: abi::Callback,
    on_hurt: abi::Callback,
    on_death: abi::Callback,
    on_interact: abi::Callback,
}

impl EntityBuilder<'_> {
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self.eye = height * 0.85;
        self
    }
    pub fn health(mut self, h: f32) -> Self {
        self.health = h;
        self
    }
    pub fn speed(mut self, s: f32) -> Self {
        self.speed = s;
        self
    }
    pub fn flags(mut self, f: u32) -> Self {
        self.flags |= f;
        self
    }
    pub fn hostile(mut self) -> Self {
        self.category = 1;
        self
    }
    pub fn passive(mut self) -> Self {
        self.category = 2;
        self
    }
    /// Skin texture, relative names are under `entity/`.
    pub fn texture(mut self, t: &str, w: u32, h: u32) -> Self {
        self.texture = if t.contains(':') { t.to_string() } else { format!("{}:entity/{t}", self.reg.ns) };
        self.texture_size = [w, h];
        self
    }
    pub fn model(mut self, parts: Vec<Part>) -> Self {
        self.parts = parts;
        self
    }
    /// Called once per tick with every entity of this kind.
    pub fn on_tick(mut self, f: impl Fn(&EntityTick) + Send + Sync + 'static) -> Self {
        self.on_tick = tick_cb(f);
        self
    }
    pub fn on_hurt(mut self, f: impl Fn(&mut EntityEvent) + Send + Sync + 'static) -> Self {
        self.on_hurt = event_cb(f);
        self
    }
    pub fn on_death(mut self, f: impl Fn(&mut EntityEvent) + Send + Sync + 'static) -> Self {
        self.on_death = event_cb(f);
        self
    }
    pub fn on_interact(mut self, f: impl Fn(&mut EntityEvent) + Send + Sync + 'static) -> Self {
        self.on_interact = event_cb(f);
        self
    }
}

impl Drop for EntityBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let parts: Vec<abi::ModelPart> =
            self.parts.iter().map(|p| abi::ModelPart { pivot: p.pivot, role: p.role, cuboids: Slice::new(&p.boxes) }).collect();
        let desc = abi::EntityDesc {
            name: Str::new(&self.name),
            cell: self.handle.cell(),
            width: self.width,
            height: self.height,
            eye: self.eye,
            max_health: self.health,
            speed: self.speed,
            flags: self.flags,
            category: self.category,
            texture: Str::new(&self.texture),
            texture_size: self.texture_size,
            parts: Slice::new(&parts),
            on_tick: self.on_tick,
            on_hurt: self.on_hurt,
            on_death: self.on_death,
            on_interact: self.on_interact,
        };
        unsafe { (host().reg_entity)(self.reg.raw, &desc) }
    }
}

pub struct MenuCtx {
    args: abi::MenuArgs,
}

impl MenuCtx {
    pub fn world(&self) -> World {
        World::from_raw(self.args.world)
    }
    pub fn player(&self) -> u64 {
        self.args.player
    }
    /// Block entity the menu belongs to, 0 for player menus.
    pub fn owner(&self) -> u64 {
        self.args.owner
    }
    /// Slot passed to `on_take`.
    pub fn slot(&self) -> u32 {
        self.args.slot
    }
    /// Adds slots: `TEMP` (menu grid), `PLAYER`, or an entity id.
    pub fn slots(&self, owner: u64, start: u32, count: u32, flags: u32) {
        unsafe { (host().menu_slots)(self.args.menu, owner, start, count, flags) }
    }
    pub fn get(&self, slot: u32) -> ItemStack {
        unsafe { (host().menu_get)(self.args.menu, slot) }
    }
    pub fn set(&self, slot: u32, stack: ItemStack) {
        unsafe { (host().menu_set)(self.args.menu, slot, stack) }
    }
    pub fn set_data(&self, index: u32, value: i32) {
        unsafe { (host().menu_set_data)(self.args.menu, index, value) }
    }
}

pub const TEMP: u64 = 0;
pub const PLAYER: u64 = u64::MAX;

fn menu_cb(f: impl Fn(&MenuCtx) + Send + Sync + 'static) -> abi::Callback {
    callback(move |a: &mut abi::MenuArgs| f(unsafe { &*(a as *const abi::MenuArgs as *const MenuCtx) }))
}

pub struct MenuBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    handle: &'static Menu,
    desc: abi::MenuDesc,
}

impl MenuBuilder<'_> {
    pub fn temp_slots(mut self, n: u32) -> Self {
        self.desc.temp_slots = n;
        self
    }
    pub fn data(mut self, n: u32) -> Self {
        self.desc.data_count = n;
        self
    }
    pub fn setup(mut self, f: impl Fn(&MenuCtx) + Send + Sync + 'static) -> Self {
        self.desc.setup = menu_cb(f);
        self
    }
    pub fn on_change(mut self, f: impl Fn(&MenuCtx) + Send + Sync + 'static) -> Self {
        self.desc.on_change = menu_cb(f);
        self
    }
    pub fn on_take(mut self, f: impl Fn(&MenuCtx) + Send + Sync + 'static) -> Self {
        self.desc.on_take = menu_cb(f);
        self
    }
    pub fn on_tick(mut self, f: impl Fn(&MenuCtx) + Send + Sync + 'static) -> Self {
        self.desc.on_tick = menu_cb(f);
        self
    }
    pub fn screen(mut self, f: impl Fn(&mut Ui) + Send + Sync + 'static) -> Self {
        self.desc.screen = ui_callback(f);
        self
    }
}

impl Drop for MenuBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.desc.name = Str::new(&self.name);
        self.desc.cell = self.handle.cell();
        unsafe { (host().reg_menu)(self.reg.raw, &self.desc) }
    }
}

impl Registry {
    pub fn entity<'a>(&'a mut self, name: &str, handle: &'static Entity) -> EntityBuilder<'a> {
        EntityBuilder {
            name: qualify(self.ns, name),
            reg: self,
            handle,
            width: 0.6,
            height: 1.8,
            eye: 1.6,
            health: 20.0,
            speed: 3.0,
            flags: entity_flags::FALL_DAMAGE | entity_flags::PUSHABLE,
            category: 0,
            texture: String::new(),
            texture_size: [64, 32],
            parts: Vec::new(),
            on_tick: abi::Callback::NONE,
            on_hurt: abi::Callback::NONE,
            on_death: abi::Callback::NONE,
            on_interact: abi::Callback::NONE,
        }
    }

    /// Plain-data component; `attach_to` lists entity kinds that get it on spawn ("core:player").
    pub fn component<T: Copy>(&mut self, name: &str, handle: &'static Component<T>, default: T, flags: u32, attach_to: &[&str]) {
        let name = qualify(self.ns, name);
        let bytes = unsafe { std::slice::from_raw_parts(&default as *const T as *const u8, size_of::<T>()) };
        let attach: Vec<Str> = attach_to.iter().map(|s| Str::new(s)).collect();
        let desc = abi::ComponentDesc {
            name: Str::new(&name),
            cell: &handle.id,
            size: size_of::<T>() as u32,
            flags,
            default: Slice::new(bytes),
            attach_to: Slice::new(&attach),
        };
        unsafe { (host().reg_component)(self.raw, &desc) }
    }

    /// Adds behaviour to an entity kind registered elsewhere (the engine's player, another mod's mob).
    pub fn hook_tick(&mut self, kind: &str, f: impl Fn(&EntityTick) + Send + Sync + 'static) {
        unsafe { (host().reg_entity_hook)(self.raw, Str::new(kind), 0, tick_cb(f)) }
    }

    pub fn hook(&mut self, kind: &str, hook: Hook, f: impl Fn(&mut EntityEvent) + Send + Sync + 'static) {
        let h = match hook {
            Hook::Tick => 0,
            Hook::Hurt => 1,
            Hook::Death => 2,
            Hook::Interact => 3,
        };
        unsafe { (host().reg_entity_hook)(self.raw, Str::new(kind), h, event_cb(f)) }
    }

    pub fn menu<'a>(&'a mut self, name: &str, handle: &'static Menu) -> MenuBuilder<'a> {
        MenuBuilder {
            name: qualify(self.ns, name),
            reg: self,
            handle,
            desc: abi::MenuDesc {
                name: Str::EMPTY,
                cell: std::ptr::null(),
                temp_slots: 0,
                data_count: 0,
                setup: abi::Callback::NONE,
                on_change: abi::Callback::NONE,
                on_take: abi::Callback::NONE,
                on_tick: abi::Callback::NONE,
                screen: abi::Callback::NONE,
            },
        }
    }

    /// The menu the inventory key opens.
    pub fn player_menu(&mut self, menu: &str) {
        let m = qualify(self.ns, menu);
        unsafe { (host().set_player_menu)(self.raw, Str::new(&m)) }
    }

    /// Gives `block` a block entity of kind `entity` with an inventory of `slots`.
    pub fn block_entity(&mut self, block: &str, entity: &str, slots: u32) {
        let (b, e) = (qualify(self.ns, block), qualify(self.ns, entity));
        unsafe { (host().set_block_entity)(self.raw, Str::new(&b), Str::new(&e), slots) }
    }

    pub fn hud(&mut self, f: impl Fn(&mut Ui) + Send + Sync + 'static) {
        unsafe { (host().reg_hud)(self.raw, ui_callback(f)) }
    }

    /// Shaped recipe: `pattern` rows use the characters in `keys`; ingredients are item names or `#tag`.
    pub fn shaped(&mut self, kind: &str, output: &str, count: u32, pattern: &[&str], keys: &[(char, &str)]) {
        let pattern: Vec<Str> = pattern.iter().map(|p| Str::new(p)).collect();
        let key_names: Vec<String> = keys.iter().map(|(_, n)| self.ingredient(n)).collect();
        let keys: Vec<abi::RecipeKey> = keys.iter().zip(&key_names).map(|((c, _), n)| abi::RecipeKey { ch: *c as u32, item: Str::new(n) }).collect();
        let output = qualify(self.ns, output);
        let kind = qualify(self.ns, kind);
        let desc = abi::RecipeDesc {
            kind: Str::new(&kind),
            pattern: Slice::new(&pattern),
            keys: Slice::new(&keys),
            inputs: Slice::EMPTY,
            output: Str::new(&output),
            count,
            time: 0,
        };
        unsafe { (host().reg_recipe)(self.raw, &desc) }
    }

    /// Shapeless recipe or a processing recipe (one input, with `time`).
    pub fn shapeless(&mut self, kind: &str, output: &str, count: u32, inputs: &[&str], time: u32) {
        let names: Vec<String> = inputs.iter().map(|n| self.ingredient(n)).collect();
        let inputs: Vec<Str> = names.iter().map(|n| Str::new(n)).collect();
        let output = qualify(self.ns, output);
        let kind = qualify(self.ns, kind);
        let desc = abi::RecipeDesc {
            kind: Str::new(&kind),
            pattern: Slice::EMPTY,
            keys: Slice::EMPTY,
            inputs: Slice::new(&inputs),
            output: Str::new(&output),
            count,
            time,
        };
        unsafe { (host().reg_recipe)(self.raw, &desc) }
    }

    fn ingredient(&self, n: &str) -> String {
        if n.starts_with('#') { n.to_string() } else { qualify(self.ns, n) }
    }
}
