use crate::abi::{self, ItemStack, Slice, Str, Versioned};
use crate::component::{Component, ENERGY, Value};
use crate::content::{Handle, Pending, kind};
use crate::ui::{Ui, ui_callback};
use crate::{Registry, World, callback, host, qualify};

pub use abi::{Energy, EntityState, component_flags, damage, entity_flags, part_role, slot_flags};

pub type Entity = Handle<kind::Entity>;
pub type Menu = Handle<kind::Menu>;

// ---------------------------------------------------------------- world helpers

impl World {
    /// Entity `id` plays keyframe animation `name` for everyone who sees it (looping until
    /// another starts); an empty name goes back to idle and walk.
    pub fn play_animation(&self, id: u64, name: &str, looping: bool) {
        unsafe { (host().play_animation)(self.raw(), id, Str::new(name), looping as u32) }
    }

    pub fn light(&self, pos: [i32; 3]) -> u32 {
        unsafe { (host().light)(self.raw(), &pos) }
    }
    /// Biome id at `pos` (`Biome::id`, `Tag::on_biome`); underground biomes differ from the
    /// surface's.
    pub fn biome(&self, pos: [i32; 3]) -> u32 {
        unsafe { (host().biome_at)(self.raw(), &pos) }
    }
    /// Sky light level at `pos` (0..15).
    pub fn sky_light(&self, pos: [i32; 3]) -> u32 {
        abi::light::sky(self.light(pos))
    }
    /// Strongest block light channel at `pos`.
    pub fn block_light(&self, pos: [i32; 3]) -> u32 {
        abi::light::block(self.light(pos))
    }
    pub fn daylight(&self) -> f32 {
        unsafe { (host().daylight)(self.raw()) }
    }
    pub fn spawn(&self, kind: &Entity, pos: [f64; 3]) -> u64 {
        self.spawn_kind(kind.id(), pos)
    }
    /// By entity kind id (`find_entity`), 0 if there is no such kind.
    pub fn spawn_kind(&self, kind: u32, pos: [f64; 3]) -> u64 {
        unsafe { (host().entity_spawn)(self.raw(), kind, &pos) }
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
    /// The entity's own box (width, height, eye height); `None` goes back to its kind's.
    /// Crouching still shrinks it.
    pub fn set_size(&self, id: u64, size: Option<[f32; 3]>) {
        let [w, h, eye] = size.unwrap_or([0.0; 3]);
        unsafe { (host().set_entity_size)(self.raw(), id, w, h, eye) }
    }
    /// Draws the entity as another kind (model, texture, animations); `None` as itself. It stays
    /// the same entity with its own behaviour.
    pub fn set_look(&self, id: u64, kind: Option<&str>) {
        unsafe { (host().set_entity_look)(self.raw(), id, Str::new(kind.unwrap_or(""))) }
    }
    /// Steer an AI entity this tick: direction on the ground plane, speed factor, jump.
    /// `id` walks to `target` by itself, around obstacles, at `speed` times its walking speed
    /// (0 stops it). Call it again as the target moves. The path is found later in the tick;
    /// `navigating` turns false when the walk ends (arrived, or stuck for two seconds).
    pub fn navigate(&self, id: u64, target: [f64; 3], speed: f32) -> bool {
        unsafe { (host().navigate)(self.raw(), id, &target, speed) != 0 }
    }
    /// Lowest corner of the nearest structure `name` (full name) within `radius` blocks of
    /// `pos`, generated yet or not.
    pub fn structure_near(&self, name: &str, pos: [i32; 3], radius: i32) -> Option<[i32; 3]> {
        let mut out = [0; 3];
        (unsafe { (host().structure_near)(self.raw(), Str::new(name), &pos, radius, &mut out) } != 0).then_some(out)
    }
    /// The block at `pos` falls as an entity (sand, gravel) and is placed again where it lands.
    pub fn fall(&self, pos: [i32; 3]) -> u64 {
        unsafe { (host().fall_block)(self.raw(), &pos) }
    }
    /// Whether `id` is still walking where `navigate` sent it.
    pub fn navigating(&self, id: u64) -> bool {
        unsafe { (host().navigating)(self.raw(), id) != 0 }
    }
    pub fn walk(&self, id: u64, dir: [f32; 2], speed: f32, jump: bool) {
        unsafe { (host().entity_walk)(self.raw(), id, &dir, speed, jump as u32) }
    }
    /// The block `id` looks at within `max` blocks (liquids count if `liquids`): position, face
    /// hit (-X, +X, -Y, +Y, -Z, +Z) and state.
    pub fn look_at(&self, id: u64, max: f64, liquids: bool) -> Option<([i32; 3], u32, u32)> {
        let (mut pos, mut face) = ([0; 3], 0);
        let s = unsafe { (host().entity_look_at)(self.raw(), id, max, liquids as u32, &mut pos, &mut face) };
        (s != 0).then_some((pos, face, s - 1))
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
    /// An entity's component, read only (`component_mut` to change it).
    pub fn component<T: Value>(&self, id: u64, c: &Component<T>) -> Option<&T> {
        let p = unsafe { (host().component)(self.raw(), id, c.id(), 0) };
        (!p.is_null()).then(|| unsafe { &*(p as *const T) })
    }
    /// Marks the component changed (saved, synced to its owner) and lends it out for writing.
    pub fn component_mut<T: Value>(&self, id: u64, c: &Component<T>) -> Option<&mut T> {
        let p = unsafe { (host().component)(self.raw(), id, c.id(), 1) };
        (!p.is_null()).then(|| unsafe { &mut *(p as *mut T) })
    }
    /// A player in creative mode: no damage, hunger or item costs.
    pub fn creative(&self, id: u64) -> bool {
        unsafe { (host().is_creative)(self.raw(), id) != 0 }
    }
    /// The entity's `energy` state, if it takes part in the energy contract.
    pub fn energy(&self, id: u64) -> Option<&Energy> {
        self.component(id, &ENERGY)
    }
    pub fn energy_mut(&self, id: u64) -> Option<&mut Energy> {
        self.component_mut(id, &ENERGY)
    }

    /// Accepts up to `amount`, respecting capacity and input rate. Returns what was accepted.
    pub fn energy_insert(&self, id: u64, amount: i64, simulate: bool) -> i64 {
        unsafe { (host().energy_insert)(self.raw(), id, amount, simulate as u32) }
    }

    pub fn energy_extract(&self, id: u64, amount: i64, simulate: bool) -> i64 {
        unsafe { (host().energy_extract)(self.raw(), id, amount, simulate as u32) }
    }

    pub fn players(&self) -> Vec<u64> {
        self.each_player().collect()
    }
    /// `players` without collecting them (mob AI asks every tick).
    pub fn each_player(&self) -> impl Iterator<Item = u64> + '_ {
        let n = unsafe { (host().player_count)(self.raw()) };
        (0..n).map(move |i| unsafe { (host().player_at)(self.raw(), i) })
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
        self.open_menu_kind(player, menu.id(), owner)
    }
    /// By menu id (`find_menu`); `owner` 0 for menus without one.
    pub fn open_menu_kind(&self, player: u64, menu: u32, owner: u64) {
        unsafe { (host().open_menu)(self.raw(), player, menu, owner) }
    }

    /// Whether a player here shows the gizmo group `group` (`/gizmo mymod:pipes`; the mod's
    /// namespace is added to bare names). Check before computing what to draw.
    pub fn gizmo_on(&self, group: &str) -> bool {
        let g = qualify(crate::mod_id(), group);
        unsafe { (host().gizmo_on)(self.raw(), Str::new(&g)) != 0 }
    }
    /// A line seen by players showing `group`, colour `0xRRGGBBAA`, for `ticks`.
    pub fn gizmo_line(&self, group: &str, a: [f64; 3], b: [f64; 3], color: u32, ticks: u32) {
        self.gizmo(group, abi::gizmo_kind::LINE, a, b, color, ticks, "");
    }
    pub fn gizmo_box(&self, group: &str, min: [f64; 3], max: [f64; 3], color: u32, ticks: u32) {
        self.gizmo(group, abi::gizmo_kind::BOX, min, max, color, ticks, "");
    }
    pub fn gizmo_text(&self, group: &str, pos: [f64; 3], text: &str, color: u32, ticks: u32) {
        self.gizmo(group, abi::gizmo_kind::LABEL, pos, pos, color, ticks, text);
    }
    fn gizmo(&self, group: &str, kind: u32, a: [f64; 3], b: [f64; 3], color: u32, ticks: u32, text: &str) {
        let g = qualify(crate::mod_id(), group);
        unsafe { (host().gizmo)(self.raw(), Str::new(&g), kind, &a, &b, color, ticks, Str::new(text)) }
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

// ---------------------------------------------------------------- recipes

/// Slots of every player's inventory (`Registry::player_inventory`).
#[derive(Clone, Debug)]
pub struct PlayerLayout {
    pub slots: u32,
    pub hotbar: u32,
    pub worn: std::ops::Range<u32>,
}

pub fn player_layout() -> PlayerLayout {
    let mut o = [0u32; 4];
    unsafe { (host().player_layout)(&mut o) };
    PlayerLayout { slots: o[0], hotbar: o[1], worn: o[2]..o[2] + o[3] }
}

/// A kind of recipe (crafting, smelting). The mod owning it registers it
/// (`Registry::recipe_kind`); recipes, stations and loot naming an unregistered kind fail at
/// load.
pub struct RecipeKind(pub &'static str);

impl RecipeKind {
    pub const fn named(name: &'static str) -> RecipeKind {
        RecipeKind(name)
    }
}

pub fn recipe_match(kind: &RecipeKind, grid: &[ItemStack], width: u32, height: u32) -> ItemStack {
    unsafe { (host().recipe_match)(Str::new(kind.0), grid.as_ptr(), width, height) }
}

/// Output and time of a processing recipe (smelting) for `input`.
pub fn recipe_process(kind: &RecipeKind, input: u32) -> Option<(ItemStack, u32)> {
    let mut t = 0;
    let s = unsafe { (host().recipe_process)(Str::new(kind.0), input, &mut t) };
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

/// Events on entities of another mod's kind; ticks go through `Registry::hook_tick`.
pub enum Hook {
    Hurt,
    Death,
    Interact,
}

/// A model part (a bone): pivot, animation role, boxes, free-form triangles, and the part it
/// hangs from.
pub struct Part {
    pub pivot: [f32; 3],
    pub role: u32,
    pub boxes: Vec<abi::ModelCuboid>,
    pub rest: [f32; 3],
    pub name: String,
    pub parent: i32,
    pub mesh: Vec<abi::MeshVertex>,
}

impl Part {
    pub fn new(pivot: [f32; 3], role: u32) -> Part {
        Part { pivot, role, boxes: Vec::new(), rest: [0.0; 3], name: String::new(), parent: -1, mesh: Vec::new() }
    }
    /// Resting rotation in radians about x, y, z (applied in that order).
    pub fn rotate(mut self, x: f32, y: f32, z: f32) -> Part {
        self.rest = [x, y, z];
        self
    }
    /// Box relative to the pivot in model pixels, `uv` = top-left texel of its unwrap.
    pub fn cube(mut self, origin: [f32; 3], size: [f32; 3], uv: [f32; 2]) -> Part {
        self.boxes.push(abi::ModelCuboid { origin, extent: size, uv, ..abi::ModelCuboid::DEFAULT });
        self
    }
    /// The last box's unwrap mirrored left to right.
    pub fn mirror(mut self) -> Part {
        if let Some(b) = self.boxes.last_mut() {
            b.mirror = 1;
        }
        self
    }
    /// Bone name keyframe animations address.
    pub fn named(mut self, name: &str) -> Part {
        self.name = name.to_string();
        self
    }
    /// Hangs from the part at `index` in the same model: moves and turns with it.
    pub fn child_of(mut self, index: usize) -> Part {
        self.parent = index as i32;
        self
    }
    /// Free-form triangles (three corners each), in model pixels like the pivot.
    pub fn mesh(mut self, triangles: Vec<abi::MeshVertex>) -> Part {
        self.mesh.extend(triangles);
        self
    }
}

/// A sphere of triangles around `center` (model pixels), its texture stretched over the texel
/// rectangle `uv` (x, y, width, height) like a globe's map.
pub fn sphere(center: [f32; 3], radius: f32, rings: u32, segments: u32, uv: [f32; 4]) -> Vec<abi::MeshVertex> {
    use std::f32::consts::{PI, TAU};
    let (rings, segments) = (rings.max(2), segments.max(3));
    let at = |i: u32, j: u32| {
        let (theta, phi) = (PI * i as f32 / rings as f32, TAU * j as f32 / segments as f32);
        let pos = [center[0] + radius * theta.sin() * phi.cos(), center[1] + radius * theta.cos(), center[2] + radius * theta.sin() * phi.sin()];
        abi::MeshVertex { pos, uv: [uv[0] + uv[2] * j as f32 / segments as f32, uv[1] + uv[3] * i as f32 / rings as f32] }
    };
    let mut out = Vec::new();
    for i in 0..rings {
        for j in 0..segments {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            // Wound so the outside faces the camera (back faces are culled).
            out.extend([a, d, c, a, c, b]);
        }
    }
    out
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

/// Vanilla's spider on a 64x32 skin: head, thorax, abdomen and eight splayed legs.
pub fn spider() -> Vec<Part> {
    use std::f32::consts::FRAC_PI_4 as Q;
    let mut parts = vec![
        Part::new([0.0, 9.0, -3.0], part_role::HEAD).cube([-4.0, -4.0, -8.0], [8.0, 8.0, 8.0], [32.0, 4.0]),
        Part::new([0.0, 9.0, 0.0], part_role::STATIC).cube([-3.0, -3.0, -3.0], [6.0, 6.0, 6.0], [0.0, 0.0]),
        Part::new([0.0, 9.0, 9.0], part_role::STATIC).cube([-5.0, -4.0, -6.0], [10.0, 8.0, 12.0], [0.0, 12.0]),
    ];
    // (z, sideways splay, fan) back to front; right legs mirror the left ones.
    for (i, (z, splay, fan)) in [(2.0, 1.0, 2.0), (1.0, 0.74, 1.0), (0.0, 0.74, -1.0), (-1.0, 1.0, -2.0)].into_iter().enumerate() {
        let (a, b) = if i % 2 == 0 { (part_role::LEG_SWEEP_A, part_role::LEG_SWEEP_B) } else { (part_role::LEG_SWEEP_B, part_role::LEG_SWEEP_A) };
        parts.push(Part::new([-4.0, 9.0, z], a).cube([-15.0, -1.0, -1.0], [16.0, 2.0, 2.0], [18.0, 0.0]).rotate(0.0, fan * Q / 2.0, splay * Q));
        parts.push(Part::new([4.0, 9.0, z], b).cube([-1.0, -1.0, -1.0], [16.0, 2.0, 2.0], [18.0, 0.0]).rotate(0.0, -fan * Q / 2.0, -splay * Q));
    }
    parts
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
    handle: Option<&'static Entity>,
    components: Vec<Pending>,
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
    replace: bool,
    carry_height: f32,
    category_name: String,
    /// Components (full name, starting value) every entity of this kind gets.
    attached: Vec<(&'static str, Vec<u8>)>,
}

/// Components of the kind (not per entity; see `Registry::entity_component` for that).
impl crate::WithComponents for EntityBuilder<'_> {
    fn pending(&mut self) -> &mut Vec<Pending> {
        &mut self.components
    }
}

impl EntityBuilder<'_> {
    /// Every entity of this kind gets `component` (declared by any mod), starting at `value`.
    pub fn attach_with<T: Value>(mut self, component: &'static Component<T>, value: T) -> Self {
        self.attached.push((component.name(), crate::component::pod_bytes(&value)));
        self
    }
    /// A solid body: others stand on it and ride along, bump into it and get pushed by it
    /// (platforms, boats, lifts). Not pushed around itself.
    pub fn solid(mut self) -> Self {
        self.flags = (self.flags | entity_flags::SOLID) & !entity_flags::PUSHABLE;
        self
    }
    /// Takes no damage (no hurt flash, health stays); `/kill` and the void still remove it.
    pub fn invulnerable(mut self) -> Self {
        self.flags |= entity_flags::INVULNERABLE;
        self
    }
    /// Neither pushes nor is pushed by other entities (ghosts, projectiles).
    pub fn no_push(mut self) -> Self {
        self.flags &= !entity_flags::PUSHABLE;
        self
    }
    /// Moved exactly by the velocity mod logic gives it (`set_entity`), through blocks, no
    /// gravity: lifts, airships, contraptions.
    pub fn kinematic(mut self) -> Self {
        self.flags |= entity_flags::KINEMATIC | entity_flags::NO_GRAVITY;
        self
    }
    /// A solid body that hurts what it squeezes into blocks instead of only pushing it.
    pub fn crushes(mut self) -> Self {
        self.flags |= entity_flags::CRUSHES;
        self
    }
    /// Riders up to `blocks` above the body (jumping, flying around inside it) keep moving with
    /// it and their own motion stays relative to it, so a jump on a fast ship lands on the ship.
    pub fn carry_height(mut self, blocks: f32) -> Self {
        self.carry_height = blocks;
        self
    }
    pub fn handle(mut self, h: &'static Entity) -> Self {
        self.handle = Some(h);
        self
    }
    /// Replaces the entity kind with this name from a mod listed in `deps`.
    pub fn replace(mut self) -> Self {
        self.replace = true;
        self
    }
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
    /// A category registered with `Registry::entity_category` (any mod's), for spawn caps and
    /// despawning.
    pub fn category(mut self, name: &str) -> Self {
        self.category_name = qualify(self.reg.ns, name);
        self
    }
    pub fn hostile(mut self) -> Self {
        self.category = abi::entity_category::HOSTILE;
        self
    }
    pub fn passive(mut self) -> Self {
        self.category = abi::entity_category::PASSIVE;
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
    /// Sounds from `<ns>:entity.<group>` (`.hurt`, `.death`, `.ambient`, `.step` events); a name
    /// with a namespace is used as it is.
    pub fn sounds(mut self, group: &str) -> Self {
        let prefix = if group.contains(':') { group.to_string() } else { format!("{}:entity.{group}", self.reg.ns) };
        self.components.push(Pending::text(abi::comp::SOUND, &prefix));
        self
    }
    /// Model from `models/<name>.geo.json` (Bedrock geometry, as Blockbench exports it) instead of
    /// the parts in code. A file named like the kind is used even without this.
    pub fn model_file(mut self, name: &str) -> Self {
        let name = crate::qualify(self.reg.ns, name);
        self.components.push(Pending::text(abi::comp::MODEL, &name));
        self
    }
    /// Keyframe animations from `animations/<name>.animation.json`: `idle` and `walk` play by
    /// themselves, others with `World::play_animation`. A file named like the kind is used even
    /// without this.
    pub fn animations(mut self, name: &str) -> Self {
        let name = crate::qualify(self.reg.ns, name);
        self.components.push(Pending::text(abi::comp::ANIMATIONS, &name));
        self
    }
    /// A moving structure built of blocks (boxes from `from` to `to`, inclusive, in its own
    /// coordinates; block (0, 0, 0) is centred on the entity's position). Blocks are placed and
    /// broken on it like in the world, and it carries what stands on it; usually `.solid()`.
    pub fn blocks(mut self, boxes: &[(&str, [i32; 3], [i32; 3])]) -> Self {
        let text: Vec<String> = boxes
            .iter()
            .map(|(b, f, t)| format!("{} {} {} {} {} {} {}", crate::qualify(self.reg.ns, b), f[0], f[1], f[2], t[0], t[1], t[2]))
            .collect();
        self.components.push(Pending::text(abi::comp::BLOCKS, &text.join(";")));
        self
    }
    /// Shades the model with `shaders/entity/<name>.wgsl` (namespace optional).
    pub fn shader(mut self, name: &str) -> Self {
        let name = crate::qualify(self.reg.ns, name);
        self.components.push(Pending::text(abi::comp::SHADER, &name));
        self
    }
    /// Called once per tick with every entity of this kind.
    pub fn on_tick(mut self, f: impl Fn(&EntityTick) + Send + Sync + 'static) -> Self {
        self.on_tick = tick_cb(f);
        self
    }
    /// Behaviour from goals (`ai::Goals`), run every tick; replaces `on_tick`.
    pub fn goals(self, goals: crate::ai::Goals) -> Self {
        self.on_tick(move |t| goals.tick(t))
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
        let parts: Vec<abi::ModelPart> = self
            .parts
            .iter()
            .map(|p| abi::ModelPart { pivot: p.pivot, role: p.role, cuboids: Slice::new(&p.boxes), rest: p.rest, name: Str::new(&p.name), parent: p.parent, mesh: Slice::new(&p.mesh), ..abi::ModelPart::DEFAULT })
            .collect();
        let mut scratch = Vec::new();
        let values = crate::content::raw_values_pub(&self.components, &mut scratch);
        let desc = abi::EntityDesc {
            name: Str::new(&self.name),
            cell: self.handle.map_or(std::ptr::null(), |h| h.cell()),
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
            replace: self.replace as u32,
            components: Slice::new(&values),
            carry_height: self.carry_height,
            category_name: Str::new(&self.category_name),
            ..abi::EntityDesc::DEFAULT
        };
        unsafe { (host().reg_entity)(self.reg.raw, &desc) }
        for (comp, value) in &self.attached {
            unsafe { (host().reg_attach_value)(self.reg.raw, Str::new(&self.name), Str::new(comp), Slice::new(value)) }
        }
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
    /// Slots taking only items with tag or component `accepts` (any mod's, full name).
    pub fn slots_for(&self, owner: u64, start: u32, count: u32, flags: u32, accepts: &str) {
        unsafe { (host().menu_slots_for)(self.args.menu, owner, start, count, flags, Str::new(accepts)) }
    }
    /// The player's main inventory, then the hotbar (the layout `ui.player_inventory` shows).
    pub fn player_slots(&self) {
        let l = player_layout();
        let main: Vec<u32> = (l.hotbar..l.slots).filter(|i| !l.worn.contains(i)).collect();
        for i in main {
            self.slots(PLAYER, i, 1, 0);
        }
        self.slots(PLAYER, 0, l.hotbar, 0);
    }
    pub fn get(&self, slot: u32) -> ItemStack {
        unsafe { (host().menu_get)(self.args.menu, slot) }
    }
    pub fn set(&self, slot: u32, stack: ItemStack) {
        unsafe { (host().menu_set)(self.args.menu, slot, stack) }
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
    handle: Option<&'static Menu>,
    desc: abi::MenuDesc,
}

impl MenuBuilder<'_> {
    pub fn handle(mut self, h: &'static Menu) -> Self {
        self.handle = Some(h);
        self
    }
    pub fn temp_slots(mut self, n: u32) -> Self {
        self.desc.temp_slots = n;
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
        self.desc.cell = self.handle.map_or(std::ptr::null(), |h| h.cell());
        unsafe { (host().reg_menu)(self.reg.raw, &self.desc) }
    }
}

impl Registry {
    pub fn entity<'a>(&'a mut self, name: &str) -> EntityBuilder<'a> {
        let d = abi::EntityDesc::DEFAULT;
        EntityBuilder {
            name: qualify(self.ns, name),
            reg: self,
            handle: None,
            components: Vec::new(),
            width: d.width,
            height: d.height,
            eye: d.eye,
            health: d.max_health,
            speed: d.speed,
            flags: d.flags,
            category: 0,
            texture: String::new(),
            texture_size: d.texture_size,
            parts: Vec::new(),
            on_tick: abi::Callback::NONE,
            on_hurt: abi::Callback::NONE,
            on_death: abi::Callback::NONE,
            on_interact: abi::Callback::NONE,
            replace: false,
            carry_height: 0.0,
            category_name: String::new(),
            attached: Vec::new(),
        }
    }

    /// Per-entity storage for `comp`, created with `default` on every spawned entity of the kinds
    /// in `attach_to` ("core:player"); flags from `component_flags`.
    pub fn entity_component<T: Value>(&mut self, comp: &Component<T>, default: T, flags: u32, attach_to: &[&str]) {
        let bytes = crate::component::pod_bytes(&default);
        let schema = T::schema();
        let attach: Vec<Str> = attach_to.iter().map(|s| Str::new(s)).collect();
        let desc = abi::ComponentDesc {
            name: Str::new(comp.name()),
            schema: Str::new(&schema),
            value_size: size_of::<T>() as u32,
            flags,
            default: Slice::new(&bytes),
            attach_to: Slice::new(&attach),
            ..abi::ComponentDesc::DEFAULT
        };
        unsafe { (host().reg_component)(self.raw, &desc) }
    }

    /// Adds behaviour to an entity kind registered elsewhere (the engine's player, another mod's mob).
    pub fn hook_tick(&mut self, kind: &str, f: impl Fn(&EntityTick) + Send + Sync + 'static) {
        unsafe { (host().reg_entity_tick_hook)(self.raw, Str::new(kind), tick_cb(f)) }
    }

    pub fn hook(&mut self, kind: &str, hook: Hook, f: impl Fn(&mut EntityEvent) + Send + Sync + 'static) {
        let h = match hook {
            Hook::Hurt => abi::entity_hook::HURT,
            Hook::Death => abi::entity_hook::DEATH,
            Hook::Interact => abi::entity_hook::INTERACT,
        };
        unsafe { (host().reg_entity_hook)(self.raw, Str::new(kind), h, event_cb(f)) }
    }

    pub fn menu<'a>(&'a mut self, name: &str) -> MenuBuilder<'a> {
        MenuBuilder { name: qualify(self.ns, name), reg: self, handle: None, desc: abi::MenuDesc::DEFAULT }
    }

    /// The player inventory: `slots` in all, the first `hotbar` of them the hotbar, `worn`
    /// equipment (never filled by pickups, shown on the model). The mod loaded last wins.
    pub fn player_inventory(&mut self, slots: u32, hotbar: u32, worn: std::ops::Range<u32>) {
        unsafe { (host().reg_player_inventory)(self.raw, slots, hotbar, worn.start, worn.len() as u32) }
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

    /// Attaches a component (by full name, e.g. "core:energy") to an entity kind.
    pub fn attach(&mut self, kind: &str, component: &str) {
        let k = qualify(self.ns, kind);
        unsafe { (host().reg_attach)(self.raw, Str::new(&k), Str::new(component)) }
    }

    /// `attach` with the value this kind starts with (a battery's capacity).
    pub fn attach_with<T: Value>(&mut self, kind: &str, component: &Component<T>, value: T) {
        let k = qualify(self.ns, kind);
        let bytes = crate::component::pod_bytes(&value);
        unsafe { (host().reg_attach_value)(self.raw, Str::new(&k), Str::new(component.name()), Slice::new(&bytes)) }
    }

    /// Chat command `/name`; `usage` declares its arguments (`<name:type>`, `[name:type]`,
    /// literal words, `|` alternatives), which the engine parses, checks and completes.
    pub fn command(&mut self, name: &str, usage: &str, f: impl Fn(&mut crate::Command) + Send + Sync + 'static) {
        let cb = crate::command::command_cb(usage, f);
        unsafe { (host().reg_command)(self.raw, Str::new(name), Str::new(usage), cb) }
    }

    /// Like `command`, but runs on the client that types it (`cmd.client()`), never on the
    /// server: for what only the player's own game has (views, local settings, debug output).
    pub fn client_command(&mut self, name: &str, usage: &str, f: impl Fn(&mut crate::Command) + Send + Sync + 'static) {
        let cb = crate::command::command_cb(usage, f);
        unsafe { (host().reg_client_command)(self.raw, Str::new(name), Str::new(usage), cb) }
    }

    /// Adds words to a word set: command parameters `word(#set)` complete from it. The
    /// engine's are `core:rules` and `core:gizmos` (or see `gizmo_group`); a mod's command can
    /// declare its own set that other mods add to.
    pub fn words(&mut self, set: &str, words: &[&str]) {
        let words = words.join(",");
        unsafe { (host().reg_words)(self.raw, Str::new(set), Str::new(&words)) }
    }

    /// A gizmo group of this mod, offered by `/gizmo`'s completion.
    pub fn gizmo_group(&mut self, name: &str) {
        let name = qualify(self.ns, name);
        self.words("core:gizmos", &[&name]);
    }

    /// A new branch of an existing command, handled here: `r.command_branch("gizmo", "clear",
    /// |cmd| ..)` makes `/gizmo clear` this mod's. Earlier alternatives win when both fit.
    pub fn command_branch(&mut self, command: &str, usage: &str, f: impl Fn(&mut crate::Command) + Send + Sync + 'static) {
        let cb = crate::command::command_cb(usage, f);
        unsafe { (host().reg_command_branch)(self.raw, Str::new(command), Str::new(usage), cb) }
    }

    /// A screen opened by name (`Client::open_screen`, `--screen`); naming it like an engine
    /// screen (`core:pause`) replaces that one.
    pub fn screen(&mut self, name: &str, f: impl Fn(&mut Ui) + Send + Sync + 'static) {
        let name = qualify(self.ns, name);
        unsafe { (host().reg_screen)(self.raw, Str::new(&name), ui_callback(f)) }
    }

    /// HUD element at `anchor` (`hud_anchor::*`); `order` 0 sits nearest the anchor's edge.
    pub fn hud_element(&mut self, name: &str, anchor: &str, order: i32, f: impl Fn(&mut Ui) + Send + Sync + 'static) {
        let name = qualify(self.ns, name);
        unsafe { (host().reg_hud_element)(self.raw, Str::new(&name), Str::new(anchor), order, ui_callback(f)) }
    }

    /// Shaped recipe: `pattern` rows use the characters in `keys`; ingredients are item names or `#tag`.
    /// The recipe viewer shows `item` (a machine or workstation) next to recipes of type `kind`.
    /// A tint kind: blocks with `.tint(name)` take its colour from the biome they stand in
    /// (`biome_tint`), `default` (0xRRGGBB) elsewhere and on items.
    pub fn tint_kind(&mut self, name: &str, default: u32) {
        let name = qualify(self.ns, name);
        unsafe { (host().reg_tint_kind)(self.raw, Str::new(&name), default) }
    }

    /// The colour of a tint kind in a biome; either may be another mod's.
    pub fn biome_tint(&mut self, biome: &str, kind: &str, color: u32) {
        let (biome, kind) = (qualify(self.ns, biome), qualify(self.ns, kind));
        unsafe { (host().reg_biome_tint)(self.raw, Str::new(&biome), Str::new(&kind), color) }
    }

    /// An entity category: at most `cap` of it spawn naturally near a player; with `despawn` > 0
    /// they vanish when no player is within that many blocks.
    pub fn entity_category(&mut self, name: &str, cap: u32, despawn: f32) {
        let name = qualify(self.ns, name);
        unsafe { (host().reg_entity_category)(self.raw, Str::new(&name), cap, despawn) }
    }

    /// Registers a recipe kind this mod owns.
    pub fn recipe_kind(&mut self, kind: &RecipeKind) {
        unsafe { (host().reg_recipe_kind)(self.raw, Str::new(kind.0)) }
    }

    pub fn recipe_station(&mut self, kind: &RecipeKind, item: &str) {
        let item = qualify(self.ns, item);
        unsafe { (host().reg_recipe_station)(self.raw, Str::new(kind.0), Str::new(&item)) }
    }

    pub fn shaped(&mut self, kind: &RecipeKind, output: &str, count: u32, pattern: &[&str], keys: &[(char, &str)]) {
        let pattern: Vec<Str> = pattern.iter().map(|p| Str::new(p)).collect();
        let key_names: Vec<String> = keys.iter().map(|(_, n)| self.ingredient(n)).collect();
        let keys: Vec<abi::RecipeKey> = keys.iter().zip(&key_names).map(|((c, _), n)| abi::RecipeKey { ch: *c as u32, item: Str::new(n) }).collect();
        let output = self.ingredient(output);
        let desc = abi::RecipeDesc { kind: Str::new(kind.0), pattern: Slice::new(&pattern), keys: Slice::new(&keys), output: Str::new(&output), count, ..abi::RecipeDesc::DEFAULT };
        unsafe { (host().reg_recipe)(self.raw, &desc) }
    }

    /// Shapeless recipe or a processing recipe (one input, with `time`).
    pub fn shapeless(&mut self, kind: &RecipeKind, output: &str, count: u32, inputs: &[&str], time: u32) {
        let names: Vec<String> = inputs.iter().map(|n| self.ingredient(n)).collect();
        let inputs: Vec<Str> = names.iter().map(|n| Str::new(n)).collect();
        let output = self.ingredient(output);
        let desc = abi::RecipeDesc { kind: Str::new(kind.0), inputs: Slice::new(&inputs), output: Str::new(&output), count, time, ..abi::RecipeDesc::DEFAULT };
        unsafe { (host().reg_recipe)(self.raw, &desc) }
    }

    fn ingredient(&self, n: &str) -> String {
        if n.starts_with('#') { n.to_string() } else { qualify(self.ns, n) }
    }
}

/// A natural spawning rule (see `abi::SpawnDesc`):
/// `r.spawn("pig").biome("animals").ground("animal_spawn").light(9, 15).group(2, 3).chance(0.05)`.
pub struct SpawnBuilder<'a> {
    raw: *mut abi::Registrar,
    names: [String; 4],
    desc: abi::SpawnDesc,
    _p: std::marker::PhantomData<&'a mut Registry>,
}

impl Registry {
    pub fn spawn<'a>(&'a mut self, entity: &str) -> SpawnBuilder<'a> {
        let entity = qualify(self.ns, entity);
        SpawnBuilder { raw: self.raw, names: [entity, String::new(), String::new(), String::new()], desc: abi::SpawnDesc::DEFAULT, _p: std::marker::PhantomData }
    }
}

impl SpawnBuilder<'_> {
    /// Only in biomes with this tag.
    pub fn biome(mut self, tag: &str) -> Self {
        self.names[1] = tag.to_string();
        self
    }
    /// Only on blocks with this tag.
    pub fn ground(mut self, tag: &str) -> Self {
        self.names[2] = tag.to_string();
        self
    }
    /// Full dimension name.
    pub fn dimension(mut self, name: &str) -> Self {
        self.names[3] = name.to_string();
        self
    }
    /// Light at the spot, inclusive (0..15).
    pub fn light(mut self, min: u32, max: u32) -> Self {
        self.desc.light = [min, max];
        self
    }
    pub fn y(mut self, min: i32, max: i32) -> Self {
        self.desc.y = [min, max];
        self
    }
    pub fn group(mut self, min: u32, max: u32) -> Self {
        self.desc.group = [min, max];
        self
    }
    pub fn weight(mut self, w: u32) -> Self {
        self.desc.weight = w;
        self
    }
    pub fn chance(mut self, c: f32) -> Self {
        self.desc.chance = c;
        self
    }
}

impl Drop for SpawnBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let [entity, biome, ground, dimension] = &self.names;
        let desc = abi::SpawnDesc { entity: Str::new(entity), biome: Str::new(biome), ground: Str::new(ground), dimension: Str::new(dimension), ..self.desc };
        unsafe { (host().reg_spawn)(self.raw, &desc) }
    }
}

/// Using a block opens `menu` on its block entity.
pub fn open_menu_on_use(menu: &'static Menu) -> impl Fn(&mut crate::Use) -> bool + Send + Sync + 'static {
    move |u| {
        let w = u.world();
        let owner = w.block_entity(u.pos());
        if owner == 0 {
            return false;
        }
        w.open_menu(u.player(), menu, owner);
        true
    }
}

type BlockLook = Box<dyn for<'r> FnOnce(crate::BlockBuilder<'r>) -> crate::BlockBuilder<'r>>;

/// A machine in one go: the block (its look and mining from `block`), an entity of the same
/// name holding its inventory and state (ticking with `on_tick`), components on that entity,
/// and the menu showing its `slots` above the player's inventory, opened by using the block.
///
/// ```ignore
/// r.machine("furnace", &FURNACE_MENU)
///     .block(|b| b.handle(&FURNACE).textures(..).hardness(3.5))
///     .slots(&[0, 0, slot_flags::OUTPUT])
///     .component(&SMELTING, Smelting::default(), component_flags::SAVE | component_flags::SYNC_MENU)
///     .on_tick(tick_furnaces)
///     .screen(|ui| ..);
/// ```
pub struct MachineBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    menu: &'static Menu,
    block: Option<BlockLook>,
    entity: Option<&'static Entity>,
    tick: abi::Callback,
    slots: Vec<u32>,
    accepts: Vec<(u32, String)>,
    with: Vec<Box<dyn FnOnce(&mut Registry, &str)>>,
    screen: Option<Box<dyn Fn(&mut Ui, u32) + Send + Sync>>,
}

impl Registry {
    pub fn machine<'a>(&'a mut self, name: &str, menu: &'static Menu) -> MachineBuilder<'a> {
        MachineBuilder { name: name.to_string(), reg: self, menu, block: None, entity: None, tick: abi::Callback::NONE, slots: Vec::new(), accepts: Vec::new(), with: Vec::new(), screen: None }
    }
}

impl MachineBuilder<'_> {
    pub fn block(mut self, f: impl for<'r> FnOnce(crate::BlockBuilder<'r>) -> crate::BlockBuilder<'r> + 'static) -> Self {
        self.block = Some(Box::new(f));
        self
    }
    pub fn entity_handle(mut self, h: &'static Entity) -> Self {
        self.entity = Some(h);
        self
    }
    /// Its inventory: one entry per slot, with `slot_flags` (`OUTPUT` takes nothing in).
    pub fn slots(mut self, flags: &[u32]) -> Self {
        self.slots = flags.to_vec();
        self
    }
    /// Slot `slot` takes only items with this tag or component (full name, like `fuel`).
    pub fn accepts(mut self, slot: u32, tag: &str) -> Self {
        self.accepts.push((slot, tag.to_string()));
        self
    }
    /// A component every machine of this kind carries (`component_flags`).
    pub fn component<T: Value>(mut self, comp: &'static Component<T>, default: T, flags: u32) -> Self {
        self.with.push(Box::new(move |r, name| r.entity_component(comp, default, flags, &[name])));
        self
    }
    /// A component declared elsewhere (by full name, like `core:energy`).
    pub fn attach(mut self, component: &str) -> Self {
        let component = component.to_string();
        self.with.push(Box::new(move |r, name| r.attach(name, &component)));
        self
    }
    /// `attach` with the value this machine starts with.
    pub fn attach_with<T: Value>(mut self, component: &'static Component<T>, value: T) -> Self {
        self.with.push(Box::new(move |r, name| r.attach_with(name, component, value)));
        self
    }
    pub fn on_tick(mut self, f: impl Fn(&EntityTick) + Send + Sync + 'static) -> Self {
        self.tick = tick_cb(f);
        self
    }
    /// The menu screen: slots 0.. are the machine's (as given to `slots`), then the player's
    /// inventory and hotbar from the slot `f` gets (`Ui::player_inventory`).
    pub fn screen(mut self, f: impl Fn(&mut Ui, u32) + Send + Sync + 'static) -> Self {
        self.screen = Some(Box::new(f));
        self
    }
}

impl Drop for MachineBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let (name, menu, reg) = (self.name.clone(), self.menu, &mut *self.reg);
        let look = self.block.take().unwrap_or_else(|| Box::new(|b| b));
        look(reg.block(&name)).on_use(open_menu_on_use(menu));
        let mut e = reg.entity(&name).flags(entity_flags::NO_GRAVITY | entity_flags::INVULNERABLE);
        if let Some(h) = self.entity {
            e = e.handle(h);
        }
        e.on_tick = self.tick;
        drop(e);
        let full = qualify(reg.ns, &name);
        for f in self.with.drain(..) {
            f(reg, &full);
        }
        reg.block_entity(&name, &name, self.slots.len() as u32);
        let slots = std::mem::take(&mut self.slots);
        let accepts = std::mem::take(&mut self.accepts);
        let player = slots.len() as u32;
        let m = reg.menu(&name).handle(menu).setup(move |m| {
            for (i, &f) in slots.iter().enumerate() {
                match accepts.iter().find(|a| a.0 == i as u32) {
                    Some((_, tag)) => m.slots_for(m.owner(), i as u32, 1, f, tag),
                    None => m.slots(m.owner(), i as u32, 1, f),
                }
            }
            m.player_slots();
        });
        match self.screen.take() {
            Some(s) => drop(m.screen(move |ui| s(ui, player))),
            None => drop(m),
        }
    }
}
