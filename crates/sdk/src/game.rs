use crate::abi::{self, ItemStack, Slice, Str};
use crate::component::{Component, ENERGY, Value};
use crate::content::{Block, Biome, Entity, EntityCategory, EquipmentSlot, GizmoGroup, HudElement, Menu, Pending, RecipeKind, Screen, TintKind};
use crate::ui::{Ui, ui_callback};
use crate::{Registry, World, callback, host, qualify};

pub use abi::{Energy, EntityState, component_flags, damage, entity_flags, part_role, slot_flags};

/// Eye height as a share of an entity's height, the engine's default too.
pub const EYE_SHARE: f32 = 0.85;

/// The engine's player kind, for hooks and components on players.
pub static PLAYER: Entity = Entity::named("core:player");


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
    /// the same entity with its own behavior.
    pub fn set_look(&self, id: u64, kind: Option<&str>) {
        unsafe { (host().set_entity_look)(self.raw(), id, Str::new(kind.unwrap_or(""))) }
    }
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
    /// `id` hits `target` the way players do: other mods can forbid it (`question::ATTACK`) or
    /// change it (`chain::ATTACK_DAMAGE`); the damage is the kind's `.attack_damage(..)`, and a
    /// mob hits at most once a second. True if it landed.
    pub fn attack(&self, id: u64, target: u64) -> bool {
        unsafe { (host().entity_attack)(self.raw(), id, target) != 0 }
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
        let p = unsafe { (host().component)(self.raw(), id, c.id()) };
        (!p.is_null()).then(|| unsafe { &*(p as *const T) })
    }
    /// The component to write; changes are saved and synced like any other.
    pub fn component_mut<T: Value>(&self, id: u64, c: &Component<T>) -> Option<&mut T> {
        let p = unsafe { (host().component)(self.raw(), id, c.id()) };
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
    pub fn insert_energy(&self, id: u64, amount: i64, simulate: bool) -> i64 {
        unsafe { (host().energy_insert)(self.raw(), id, amount, simulate as u32) }
    }

    pub fn extract_energy(&self, id: u64, amount: i64, simulate: bool) -> i64 {
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
    pub fn gizmo_on(&self, group: &GizmoGroup) -> bool {
        unsafe { (host().gizmo_on)(self.raw(), Str::new(group.name())) != 0 }
    }
    /// A line seen by players showing `group`, color `0xRRGGBBAA`, for `ticks`.
    pub fn draw_line(&self, group: &GizmoGroup, a: [f64; 3], b: [f64; 3], color: u32, ticks: u32) {
        self.gizmo(group, abi::gizmo_kind::LINE, a, b, color, ticks, "", &[]);
    }
    pub fn draw_box(&self, group: &GizmoGroup, min: [f64; 3], max: [f64; 3], color: u32, ticks: u32) {
        self.gizmo(group, abi::gizmo_kind::BOX, min, max, color, ticks, "", &[]);
    }
    /// The translation `key` with `args` at `pos`, in each viewer's language.
    pub fn draw_text(&self, group: &GizmoGroup, pos: [f64; 3], key: &str, args: &[crate::Arg], color: u32, ticks: u32) {
        self.gizmo(group, abi::gizmo_kind::LABEL, pos, pos, color, ticks, key, args);
    }
    #[allow(clippy::too_many_arguments)]
    fn gizmo(&self, group: &GizmoGroup, kind: u32, a: [f64; 3], b: [f64; 3], color: u32, ticks: u32, key: &str, args: &[crate::Arg]) {
        let raw = crate::ui::raw_args(args);
        unsafe { (host().gizmo)(self.raw(), Str::new(group.name()), kind, &a, &b, color, ticks, Str::new(key), abi::Slice::new(&raw)) }
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
    /// A machine's slot group (`MachineBuilder::group`) in this inventory; a name it doesn't
    /// have is logged as a mistake and gives no slots.
    pub fn group(&self, name: &str) -> InventoryGroup {
        self.find_group(name).unwrap_or_else(|| {
            crate::error!("the inventory of entity {} has no group {name}", self.owner);
            InventoryGroup { inv: Inventory { world: self.world, owner: self.owner }, start: 0, len: 0 }
        })
    }

    /// The group if this inventory has it: a player's equipment, which mobs don't have.
    pub fn find_group(&self, name: &str) -> Option<InventoryGroup> {
        let mut out = [0u32; 2];
        let found = unsafe { (host().inventory_group)(self.world.raw(), self.owner, Str::new(name), &mut out) } != 0;
        found.then(|| InventoryGroup { inv: Inventory { world: self.world, owner: self.owner }, start: out[0], len: out[1] })
    }
}

/// Slots of one group in an inventory, numbered from 0.
pub struct InventoryGroup {
    inv: Inventory,
    start: u32,
    len: u32,
}

impl InventoryGroup {
    pub fn len(&self) -> u32 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn get(&self, i: u32) -> ItemStack {
        if i < self.len { self.inv.get(self.start + i) } else { ItemStack::EMPTY }
    }
    pub fn set(&self, i: u32, stack: ItemStack) {
        if i < self.len {
            self.inv.set(self.start + i, stack)
        }
    }
}

// ---------------------------------------------------------------- recipes

pub struct PlayerInventoryBuilder<'a> {
    raw: *mut abi::Registrar,
    groups: GroupList,
    _p: std::marker::PhantomData<&'a mut Registry>,
}

impl PlayerInventoryBuilder<'_> {
    /// A group of the player's inventory: its size from `slots` (`Slots::row(9).hotbar()`).
    pub fn group(mut self, name: &str, slots: Slots) -> Self {
        self.groups.push("", name, slots, abi::slot_owner::PLAYER);
        self
    }
}

impl Drop for PlayerInventoryBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let descs = self.groups.descs();
        unsafe { (host().reg_player_inventory)(self.raw, Slice::new(&descs)) }
    }
}

pub fn recipe_match(kind: &RecipeKind, grid: &[ItemStack], width: u32, height: u32) -> ItemStack {
    unsafe { (host().recipe_match)(Str::new(kind.name()), grid.as_ptr(), width, height) }
}

/// Output and time of a processing recipe (smelting) for `input`.
pub fn recipe_process(kind: &RecipeKind, input: u32) -> Option<(ItemStack, u32)> {
    let mut t = 0;
    let s = unsafe { (host().recipe_process)(Str::new(kind.name()), input, &mut t) };
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
    pub fn rotation(mut self, x: f32, y: f32, z: f32) -> Part {
        self.rest = [x, y, z];
        self
    }
    /// Box relative to the pivot in model pixels, `uv` = top-left texel of its unwrap.
    pub fn cube(mut self, origin: [f32; 3], size: [f32; 3], uv: [f32; 2]) -> Part {
        self.boxes.push(abi::ModelCuboid { origin, extent: size, uv, ..abi::ModelCuboid::DEFAULT });
        self
    }
    /// The last box's unwrap mirrored left to right.
    pub fn mirrored(mut self) -> Part {
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
/// A model built in code: its parts and the size of the skin its UVs are laid out on (the
/// picture itself may be bigger, a pack's high resolution skin keeps lining up).
pub struct Model {
    pub parts: Vec<Part>,
    pub uv: [u32; 2],
}

impl Model {
    /// The UV size, if the layout isn't on a 64 x 32 skin.
    pub fn uv(mut self, width: u32, height: u32) -> Self {
        self.uv = [width, height];
        self
    }
}

impl From<Vec<Part>> for Model {
    fn from(parts: Vec<Part>) -> Model {
        Model { parts, uv: [64, 32] }
    }
}

pub fn biped(arms_forward: bool) -> Model {
    let (l, r) = if arms_forward { (part_role::ARM_FORWARD_LEFT, part_role::ARM_FORWARD_RIGHT) } else { (part_role::ARM_LEFT, part_role::ARM_RIGHT) };
    Model::from(vec![
        Part::new([0.0, 24.0, 0.0], part_role::HEAD).cube([-4.0, 0.0, -4.0], [8.0, 8.0, 8.0], [0.0, 0.0]),
        Part::new([0.0, 24.0, 0.0], part_role::STATIC).cube([-4.0, -12.0, -2.0], [8.0, 12.0, 4.0], [16.0, 16.0]),
        Part::new([-6.0, 22.0, 0.0], l).cube([-2.0, -10.0, -2.0], [4.0, 12.0, 4.0], [40.0, 16.0]),
        Part::new([6.0, 22.0, 0.0], r).cube([-2.0, -10.0, -2.0], [4.0, 12.0, 4.0], [40.0, 16.0]),
        Part::new([-2.0, 12.0, 0.0], part_role::LEG_LEFT).cube([-2.0, -12.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0]),
        Part::new([2.0, 12.0, 0.0], part_role::LEG_RIGHT).cube([-2.0, -12.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0]),
    ])
}

/// Vanilla's spider on a 64x32 skin: head, thorax, abdomen and eight splayed legs.
pub fn spider() -> Model {
    use std::f32::consts::FRAC_PI_4 as Q;
    let mut parts = vec![
        Part::new([0.0, 9.0, -3.0], part_role::HEAD).cube([-4.0, -4.0, -8.0], [8.0, 8.0, 8.0], [32.0, 4.0]),
        Part::new([0.0, 9.0, 0.0], part_role::STATIC).cube([-3.0, -3.0, -3.0], [6.0, 6.0, 6.0], [0.0, 0.0]),
        Part::new([0.0, 9.0, 9.0], part_role::STATIC).cube([-5.0, -4.0, -6.0], [10.0, 8.0, 12.0], [0.0, 12.0]),
    ];
    // (z, sideways splay, fan) back to front; right legs mirror the left ones.
    for (i, (z, splay, fan)) in [(2.0, 1.0, 2.0), (1.0, 0.74, 1.0), (0.0, 0.74, -1.0), (-1.0, 1.0, -2.0)].into_iter().enumerate() {
        let (a, b) = if i % 2 == 0 { (part_role::LEG_SWEEP_A, part_role::LEG_SWEEP_B) } else { (part_role::LEG_SWEEP_B, part_role::LEG_SWEEP_A) };
        parts.push(Part::new([-4.0, 9.0, z], a).cube([-15.0, -1.0, -1.0], [16.0, 2.0, 2.0], [18.0, 0.0]).rotation(0.0, fan * Q / 2.0, splay * Q));
        parts.push(Part::new([4.0, 9.0, z], b).cube([-1.0, -1.0, -1.0], [16.0, 2.0, 2.0], [18.0, 0.0]).rotation(0.0, -fan * Q / 2.0, -splay * Q));
    }
    Model::from(parts)
}

/// Four-legged animal: body (width, height, length), leg height and head size in model pixels.
/// Skin is 64x64: head at (0,0), legs at (0,16), body at (0,32).
pub fn quadruped(body: [f32; 3], leg: f32, head: [f32; 3]) -> Model {
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
    Model::from(parts).uv(64, 64)
}

pub struct EntityBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    handle: Option<&'static Entity>,
    components: Vec<Pending>,
    width: f32,
    height: f32,
    eye: f32,
    speed: f32,
    flags: u32,
    texture: String,
    texture_size: [u32; 2],
    parts: Vec<Part>,
    on_tick: abi::Callback,
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
    /// Every entity of this kind carries `component` (declared with `r.entity_component`, by any
    /// mod), starting at its declared value. Not `.set`: that is one value of the kind itself,
    /// `.attach` gives each entity its own, which changes.
    pub fn attach<T: Value>(mut self, component: &'static Component<T>) -> Self {
        self.attached.push((component.name(), Vec::new()));
        self
    }
    /// `attach` with another starting value.
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
    pub fn unpushable(mut self) -> Self {
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
    pub fn crushing(mut self) -> Self {
        self.flags |= entity_flags::CRUSHES;
        self
    }
    /// Riders up to `blocks` above the body (jumping, flying around inside it) keep moving with
    /// it and their own motion stays relative to it, so a jump on a fast ship lands on the ship.
    pub fn carry_height(mut self, blocks: f32) -> Self {
        self.carry_height = blocks;
        self
    }
    /// Replaces the entity kind with this name from a mod listed in `deps`.
    pub fn replacing(mut self) -> Self {
        self.replace = true;
        self
    }
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self.eye = height * EYE_SHARE;
        self
    }
    /// Damage of its melee hits (`World::attack`), like an item's.
    pub fn attack_damage(self, damage: f32) -> Self {
        crate::WithComponents::set(self, &crate::component::ATTACK_DAMAGE, damage)
    }
    pub fn health(mut self, h: f32) -> Self {
        self.components.retain(|p| p.name != abi::attribute::MAX_HEALTH);
        self.components.push(Pending::value(abi::attribute::MAX_HEALTH, "f64".into(), crate::component::pod_bytes(&(h as f64))));
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
    pub fn category(mut self, category: &EntityCategory) -> Self {
        self.category_name = category.name().to_string();
        self
    }
    pub fn hostile(mut self) -> Self {
        self.category_name = "core:hostile".into();
        self
    }
    pub fn passive(mut self) -> Self {
        self.category_name = "core:passive".into();
        self
    }
    /// Skin texture under `entity/`; without this, the kind's name (`entity/zombie`).
    pub fn texture(mut self, t: &str) -> Self {
        self.texture = crate::asset("entity", t);
        self
    }
    /// A model built in code (`biped(..)`, `quadruped(..)`, `spider()`, or parts of your own).
    pub fn model(mut self, model: impl Into<Model>) -> Self {
        let m = model.into();
        (self.parts, self.texture_size) = (m.parts, m.uv);
        self
    }
    /// Sound event `event` (`hurt`, `death`, `ambient`, `step`): one of the files
    /// `sounds/entity/<kind>/<event>*.ogg|wav` at random.
    pub fn sound(self, event: &str) -> Self {
        let file = format!("entity/{}/{event}*", self.kind_path());
        self.sound_files(event, &[&file])
    }
    /// Sound event `event` from these files of `sounds/entity/<kind>/`, one at random.
    pub fn sound_variants(self, event: &str, files: &[&str]) -> Self {
        let path = self.kind_path();
        let files: Vec<String> = files.iter().map(|f| format!("entity/{path}/{f}")).collect();
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        self.sound_files(event, &files)
    }
    fn kind_path(&self) -> String {
        self.name.split_once(':').map_or(self.name.as_str(), |x| x.1).to_string()
    }
    fn sound_files(mut self, event: &str, files: &[&str]) -> Self {
        let sound = crate::Sound::generated(format!("{}.{event}", self.name));
        self.reg.sound(sound).files(files).category(&crate::sound_categories::CREATURES).pitch(0.8, 1.2);
        self.components.retain(|p| p.name != abi::comp::SOUND);
        self.components.push(Pending::text(abi::comp::SOUND, &self.name));
        self
    }
    /// Sounds from `<ns>:entity.<group>` (`.hurt`, `.death`, `.ambient`, `.step` events); a name
    /// with a namespace is used as it is.
    pub fn sound_group(mut self, group: &str) -> Self {
        let prefix = if group.contains(':') { group.to_string() } else { format!("{}:entity.{group}", self.reg.ns) };
        self.components.push(Pending::text(abi::comp::SOUND, &prefix));
        self
    }
    /// Model from `models/<name>.geo.json` (Bedrock geometry, as Blockbench exports it) instead of
    /// the parts in code. A file named like the kind is used even without this.
    pub fn model_file(mut self, name: &str) -> Self {
        let name = crate::asset("", name);
        self.components.push(Pending::text(abi::comp::MODEL, &name));
        self
    }
    /// Keyframe animations from `animations/<name>.animation.json`: `idle` and `walk` play by
    /// themselves, others with `World::play_animation`. A file named like the kind is used even
    /// without this.
    pub fn animations(mut self, name: &str) -> Self {
        let name = crate::asset("", name);
        self.components.push(Pending::text(abi::comp::ANIMATIONS, &name));
        self
    }
    /// A moving structure built of blocks (boxes from `from` to `to`, inclusive, in its own
    /// coordinates; block (0, 0, 0) is centered on the entity's position). Blocks are placed and
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
        let name = crate::asset("", name);
        self.components.push(Pending::text(abi::comp::SHADER, &name));
        self
    }
    /// Called once per tick with every entity of this kind.
    pub fn on_tick(mut self, f: impl Fn(&EntityTick) + Send + Sync + 'static) -> Self {
        self.on_tick = tick_cb(f);
        self
    }
    /// Behavior from goals (`ai::Goals`), run every tick; replaces `on_tick`.
    pub fn goals(self, goals: crate::ai::Goals) -> Self {
        self.on_tick(move |t| goals.tick(t))
    }
    /// One of these was hurt: `ENTITY_HURT` for this kind, after the damage, with the other
    /// events at the end of the tick. Changing the damage is `r.modify(&chain::DAMAGE, ..)`,
    /// preventing it `r.intercept(&question::HURT, ..)`.
    pub fn on_hurt(self, f: impl Fn(&World, &crate::Hurt) + Send + Sync + 'static) -> Self {
        let Some(kind) = self.handle else { return self };
        self.reg.on(&crate::ENTITY_HURT, move |b| {
            let w = b.world();
            for h in b.iter().filter(|h| w.entity(h.entity).is_some_and(|s| s.kind == kind.id())) {
                f(&w, h);
            }
        });
        self
    }
    /// One of these died: `ENTITY_DIED` for this kind.
    pub fn on_death(self, f: impl Fn(&World, &crate::Death) + Send + Sync + 'static) -> Self {
        let Some(kind) = self.handle else { return self };
        self.reg.on(&crate::ENTITY_DIED, move |b| {
            let w = b.world();
            for d in b.iter().filter(|d| d.kind == kind.id()) {
                f(&w, d);
            }
        });
        self
    }
    /// A player right clicked one, and `question::INTERACT_ENTITY` let it; `e.handled()` swings
    /// the player's arm.
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
            speed: self.speed,
            flags: self.flags,
            texture: Str::new(&self.texture),
            texture_size: self.texture_size,
            parts: Slice::new(&parts),
            on_tick: self.on_tick,
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
    /// Slot group `name` of this menu.
    pub fn group(&self, name: &str) -> MenuGroup<'_> {
        let mut out = [0u32; 3];
        unsafe { (host().menu_group)(self.args.menu, Str::new(name), &mut out) };
        MenuGroup { m: self, start: out[0], len: out[1], cols: out[2] }
    }
}

/// Slots of one group of an open menu, numbered from 0, whoever's inventory they show.
pub struct MenuGroup<'a> {
    m: &'a MenuCtx,
    start: u32,
    len: u32,
    cols: u32,
}

impl MenuGroup<'_> {
    pub fn len(&self) -> u32 {
        self.len
    }
    /// Its width as laid out (a 3 x 3 grid has 3).
    pub fn cols(&self) -> u32 {
        self.cols
    }
    pub fn rows(&self) -> u32 {
        self.len.div_ceil(self.cols.max(1))
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn get(&self, i: u32) -> ItemStack {
        if i < self.len { unsafe { (host().menu_get)(self.m.args.menu, self.start + i) } } else { ItemStack::EMPTY }
    }
    pub fn set(&self, i: u32, stack: ItemStack) {
        if i < self.len {
            unsafe { (host().menu_set)(self.m.args.menu, self.start + i, stack) }
        }
    }
}

/// The groups `player_inventory()` adds, for `Ui::group` and `MenuCtx::group`.
pub const INVENTORY_GROUP: &str = "core:inventory";
pub const HOTBAR_GROUP: &str = "core:hotbar";
/// Every equipment slot of a player's inventory (`w.inventory(player).group(EQUIPMENT_GROUP)`).
pub const EQUIPMENT_GROUP: &str = "core:equipment";

/// A group of slots: a grid, row or column of one inventory sharing its rules. Shown with
/// `ui.group(name)`, read and written with `MenuCtx::group(name)`.
///
/// ```ignore
/// r.machine(&ALLOY_SMELTER, &MENU)
///     .group("input", Slots::row(2))
///     .group("fuel", Slots::single().accepts("fuel").background("flame_slot"))
///     .group("output", Slots::single().output().large());
/// ```
#[derive(Clone)]
pub struct Slots {
    owner: Option<u32>,
    start: u32,
    cols: u32,
    rows: u32,
    flags: u32,
    accepts: String,
    limit: u32,
    size: f32,
    background: String,
    tooltip: String,
    priority: i32,
}

impl Slots {
    pub fn grid(cols: u32, rows: u32) -> Slots {
        Slots { owner: None, start: u32::MAX, cols, rows, flags: 0, accepts: String::new(), limit: 0, size: 0.0, background: String::new(), tooltip: String::new(), priority: 0 }
    }
    pub fn row(n: u32) -> Slots {
        Slots::grid(n, 1)
    }
    pub fn column(n: u32) -> Slots {
        Slots::grid(1, n)
    }
    pub fn single() -> Slots {
        Slots::grid(1, 1)
    }
    /// Rows of nine, like a chest.
    pub fn chest(rows: u32) -> Slots {
        Slots::grid(9, rows)
    }
    /// The menu's own slots, given back to the player when it closes (crafting grids). The
    /// default for `MenuBuilder::group`.
    pub fn menu(mut self) -> Self {
        self.owner = Some(abi::slot_owner::MENU);
        self
    }
    /// The inventory of the entity the menu opened on. The default for `MachineBuilder::group`.
    pub fn machine(mut self) -> Self {
        self.owner = Some(abi::slot_owner::BLOCK);
        self
    }
    /// The player's hotbar, in `Registry::player_inventory`.
    pub fn hotbar(mut self) -> Self {
        self.flags |= slot_flags::HOTBAR;
        self
    }
    /// From this index of the owner's inventory instead of right after the group before.
    pub fn at(mut self, start: u32) -> Self {
        self.start = start;
        self
    }
    /// Items only go in (a machine uses them up).
    pub fn input(mut self) -> Self {
        self.flags |= abi::slot_flags::INPUT;
        self
    }
    /// Items only come out (results).
    pub fn output(mut self) -> Self {
        self.flags |= abi::slot_flags::OUTPUT;
        self
    }
    /// Only items with this tag or component (its full name, like `fuel` or `armor_head`).
    pub fn accepts(mut self, tag: &str) -> Self {
        self.accepts = tag.to_string();
        self
    }
    /// At most this many items per slot.
    pub fn limit(mut self, n: u32) -> Self {
        self.limit = n;
        self
    }
    /// Side of a slot in GUI pixels (18 by default).
    pub fn size(mut self, px: f32) -> Self {
        self.size = px;
        self
    }
    /// The big slot vanilla uses for results.
    pub fn large(self) -> Self {
        self.size(26.0)
    }
    /// A texture from `gui/` shown while a slot is empty (the armor silhouettes).
    pub fn background(mut self, texture: &str) -> Self {
        self.background = crate::asset("gui", texture);
        self
    }
    /// A translation shown over an empty slot.
    pub fn tooltip(mut self, key: &str) -> Self {
        self.tooltip = key.to_string();
        self
    }
    /// Shift-clicks from the player's inventory fill groups with a higher priority first.
    pub fn priority(mut self, p: i32) -> Self {
        self.priority = p;
        self
    }
}

/// Owned strings of the groups while their descriptors are sent.
struct GroupList {
    names: Vec<String>,
    groups: Vec<Slots>,
    strings: Vec<[String; 3]>,
}

impl GroupList {
    fn new() -> GroupList {
        GroupList { names: Vec::new(), groups: Vec::new(), strings: Vec::new() }
    }
    fn push(&mut self, ns: &str, name: &str, s: Slots, default_owner: u32) {
        let s = Slots { owner: Some(s.owner.unwrap_or(default_owner)), ..s };
        let q = |t: &str| if t.is_empty() || t.contains(':') { t.to_string() } else { qualify(ns, t) };
        self.strings.push([s.accepts.clone(), q(&s.background), s.tooltip.clone()]);
        self.names.push(name.to_string());
        self.groups.push(s);
    }
    fn player_inventory(&mut self) {
        self.names.push(INVENTORY_GROUP.into());
        self.groups.push(Slots { owner: Some(abi::slot_owner::PLAYER_MAIN), ..Slots::single() });
        self.strings.push(Default::default());
        self.names.push(HOTBAR_GROUP.into());
        self.groups.push(Slots { owner: Some(abi::slot_owner::PLAYER_HOTBAR), ..Slots::single() });
        self.strings.push(Default::default());
    }
    fn descs(&self) -> Vec<abi::SlotGroupDesc> {
        self.groups
            .iter()
            .zip(&self.names)
            .zip(&self.strings)
            .map(|((g, name), [accepts, background, tooltip])| abi::SlotGroupDesc {
                name: Str::new(name),
                owner: g.owner.unwrap_or(0),
                start: g.start,
                cols: g.cols,
                rows: g.rows,
                flags: g.flags,
                accepts: Str::new(accepts),
                limit: g.limit,
                size: g.size,
                background: Str::new(background),
                tooltip: Str::new(tooltip),
                priority: g.priority,
            })
            .collect()
    }
}

fn menu_cb(f: impl Fn(&MenuCtx) + Send + Sync + 'static) -> abi::Callback {
    callback(move |a: &mut abi::MenuArgs| f(unsafe { &*(a as *const abi::MenuArgs as *const MenuCtx) }))
}

pub struct MenuBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    handle: Option<&'static Menu>,
    desc: abi::MenuDesc,
    groups: GroupList,
    entity: String,
    recipes: Vec<String>,
}

impl MenuBuilder<'_> {
    /// A group of slots, by default the menu's own (`Slots::menu`). Groups come in this order.
    pub fn group(mut self, name: &str, slots: Slots) -> Self {
        self.groups.push(self.reg.ns, name, slots, abi::slot_owner::MENU);
        self
    }
    /// The player's inventory and hotbar as two groups (`INVENTORY_GROUP`, `HOTBAR_GROUP`), shown
    /// with `ui.player_inventory(..)`.
    pub fn player_inventory(mut self) -> Self {
        self.groups.player_inventory();
        self
    }
    /// The player's equipment slots, here among the groups, each named like its slot
    /// (`ui.equipment()` draws them).
    pub fn equipment(mut self) -> Self {
        self.groups.push("", EQUIPMENT_GROUP, Slots { owner: Some(abi::slot_owner::PLAYER_EQUIPMENT), ..Slots::single() }, abi::slot_owner::PLAYER_EQUIPMENT);
        self
    }
    /// Recipes of this kind are made here: clicking an `arrow` bar of the screen shows them.
    pub fn recipes(mut self, kind: &RecipeKind) -> Self {
        self.recipes.push(kind.name().to_string());
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
        let groups = self.groups.descs();
        self.desc.groups = Slice::new(&groups);
        self.desc.entity = Str::new(&self.entity);
        let recipes = self.recipes.join(" ");
        self.desc.recipes = Str::new(&recipes);
        unsafe { (host().reg_menu)(self.reg.raw, &self.desc) }
    }
}

impl Registry {
    pub fn entity<'a>(&'a mut self, kind: &'static Entity) -> EntityBuilder<'a> {
        let d = abi::EntityDesc::DEFAULT;
        EntityBuilder {
            name: kind.name().to_string(),
            reg: self,
            handle: Some(kind),
            components: Vec::new(),
            width: d.width,
            height: d.height,
            eye: d.eye,
            speed: d.speed,
            flags: d.flags,
            texture: crate::asset("entity", kind.name()),
            texture_size: d.texture_size,
            parts: Vec::new(),
            on_tick: abi::Callback::NONE,
            on_interact: abi::Callback::NONE,
            replace: false,
            carry_height: 0.0,
            category_name: String::new(),
            attached: Vec::new(),
        }
    }

    /// Declares per-entity storage for `comp`: the value an entity starts with and the
    /// `component_flags` (saved, synced to whom). Kinds carry it with `.attach(&COMP)` (or
    /// `r.attach(&KIND, &COMP)` for another mod's kind).
    pub fn entity_component<T: Value>(&mut self, comp: &Component<T>, default: T, flags: u32) {
        let bytes = crate::component::pod_bytes(&default);
        let schema = T::schema();
        let desc = abi::ComponentDesc {
            name: Str::new(comp.name()),
            schema: Str::new(&schema),
            value_size: size_of::<T>() as u32,
            flags,
            default: Slice::new(&bytes),
            ..abi::ComponentDesc::DEFAULT
        };
        unsafe { (host().reg_component)(self.raw, &desc) }
    }

    /// Logic that is nobody's own (players' hunger, a mod's weather): once per tick in every
    /// region of every dimension, where `w.players()` are the players in that region. Another
    /// mod's entities are reached through events (`r.on(&ENTITY_HURT, ..)`) and questions.
    pub fn world_tick(&mut self, f: impl Fn(&World) + Send + Sync + 'static) {
        let cb = crate::callback(move |a: &mut abi::WorldTickArgs| f(&World::from_raw(a.world)));
        unsafe { (host().reg_world_tick)(self.raw, cb) }
    }

    pub fn menu<'a>(&'a mut self, menu: &'static Menu) -> MenuBuilder<'a> {
        MenuBuilder { name: menu.name().to_string(), reg: self, handle: Some(menu), desc: abi::MenuDesc::DEFAULT, groups: GroupList::new(), entity: String::new(), recipes: Vec::new() }
    }

    /// The player inventory as named groups, one of them `.hotbar()`:
    /// `r.player_inventory().group("hotbar", Slots::row(9).hotbar()).group("main", Slots::grid(9, 3))`.
    /// The engine lays them out, hotbar first, the equipment slots (`equipment_slot`) after
    /// everything; `w.inventory(player).group(name)` reads a group, `core:equipment` all of the
    /// equipment. The mod loaded last wins.
    pub fn player_inventory(&mut self) -> PlayerInventoryBuilder<'_> {
        PlayerInventoryBuilder { raw: self.raw, groups: GroupList::new(), _p: std::marker::PhantomData }
    }

    /// An equipment slot of players (head, chest...): what is worn there covers `parts`
    /// (`worn_parts`) of the model, `background` (from `gui/`) shows while it is empty. Items go
    /// there with `.wearable(&SLOT, ..)`; menus show the slots with `.equipment()`.
    pub fn equipment_slot(&mut self, slot: &EquipmentSlot, parts: u32, background: &str) {
        let bg = crate::asset("gui", background);
        unsafe { (host().reg_equipment_slot)(self.raw, Str::new(slot.name()), parts, Str::new(&bg)) }
    }

    /// The menu the inventory key opens.
    pub fn player_menu(&mut self, menu: &Menu) {
        unsafe { (host().set_player_menu)(self.raw, Str::new(menu.name())) }
    }

    /// Gives `block` a block entity of kind `entity` with an inventory of `slots`.
    pub fn block_entity(&mut self, block: &Block, entity: &Entity, slots: u32) {
        unsafe { (host().set_block_entity)(self.raw, Str::new(block.name()), Str::new(entity.name()), slots) }
    }

    /// Every entity of `kind` (any mod's, `&PLAYER`) carries `component`, starting at its
    /// declared value.
    pub fn attach<T: Value>(&mut self, kind: &Entity, component: &Component<T>) {
        unsafe { (host().reg_attach_value)(self.raw, Str::new(kind.name()), Str::new(component.name()), Slice::EMPTY) }
    }

    /// `attach` with another starting value (a battery's capacity).
    pub fn attach_with<T: Value>(&mut self, kind: &Entity, component: &Component<T>, value: T) {
        let bytes = crate::component::pod_bytes(&value);
        unsafe { (host().reg_attach_value)(self.raw, Str::new(kind.name()), Str::new(component.name()), Slice::new(&bytes)) }
    }

    /// Chat command `/name`; `usage` declares its arguments (`<name:type>`, `[name:type]`,
    /// literal words, `|` alternatives), which the engine parses, checks and completes.
    pub fn command(&mut self, name: &str, usage: &str, f: impl Fn(&mut crate::Command) + Send + Sync + 'static) {
        let cb = crate::command::command_cb(f);
        unsafe { (host().reg_command)(self.raw, Str::new(name), Str::new(usage), cb) }
    }

    /// Like `command`, but runs on the client that types it (`cmd.client()`), never on the
    /// server: for what only the player's own game has (views, local settings, debug output).
    pub fn client_command(&mut self, name: &str, usage: &str, f: impl Fn(&mut crate::Command) + Send + Sync + 'static) {
        let cb = crate::command::command_cb(f);
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
    pub fn gizmo_group(&mut self, group: &GizmoGroup) {
        self.words("core:gizmos", &[group.name()]);
    }

    /// A new branch of an existing command, handled here: `r.command_branch("gizmo", "clear",
    /// |cmd| ..)` makes `/gizmo clear` this mod's. Earlier alternatives win when both fit.
    pub fn command_branch(&mut self, command: &str, usage: &str, f: impl Fn(&mut crate::Command) + Send + Sync + 'static) {
        let cb = crate::command::command_cb(f);
        unsafe { (host().reg_command_branch)(self.raw, Str::new(command), Str::new(usage), cb) }
    }

    /// Adds to place `area` of `screen` without replacing the screen: a button in the main menu is
    /// `r.screen_part(&screens::TITLE, "buttons", 0, |ui| ..)`. Lower `order` comes first. The
    /// engine's places: `TITLE` `buttons` and `side`, `PAUSE` `buttons`, `SETTINGS` `buttons`.
    pub fn screen_part(&mut self, screen: &Screen, area: &str, order: i32, f: impl Fn(&mut Ui) + Send + Sync + 'static) {
        let cb = crate::ui::ui_callback(f);
        unsafe { (host().reg_screen_part)(self.raw, Str::new(screen.name()), Str::new(area), order, cb) }
    }

    /// A screen opened with `Client::open_screen` (or `--screen <name>`); registering an engine
    /// screen's handle (`screens::PAUSE`) replaces that one. How it behaves is declared on it,
    /// the same properties the engine's screens have: `r.screen(&screens::PAUSE, build).pauses()`.
    pub fn screen<'a>(&'a mut self, screen: &Screen, f: impl Fn(&mut Ui) + Send + Sync + 'static) -> ScreenBuilder<'a> {
        ScreenBuilder { raw: self.raw, name: screen.name().to_string(), desc: abi::ScreenDesc { build: ui_callback(f), ..abi::ScreenDesc::DEFAULT }, _p: std::marker::PhantomData }
    }

    /// HUD element at `anchor` (`hud_anchor::*`); `order` 0 sits nearest the anchor's edge.
    pub fn hud_element(&mut self, element: &HudElement, anchor: &str, order: i32, f: impl Fn(&mut Ui) + Send + Sync + 'static) {
        unsafe { (host().reg_hud_element)(self.raw, Str::new(element.name()), Str::new(anchor), order, ui_callback(f)) }
    }

    /// Shaped recipe: `pattern` rows use the characters in `keys`; ingredients are item names or `#tag`.
    /// The recipe viewer shows `item` (a machine or workstation) next to recipes of type `kind`.
    /// A tint kind: blocks with `.tint(&KIND)` take its color from the biome they stand in
    /// (`biome_tint`), `default` (0xRRGGBB) elsewhere and on items.
    pub fn tint_kind(&mut self, kind: &TintKind, default: u32) {
        unsafe { (host().reg_tint_kind)(self.raw, Str::new(kind.name()), default) }
    }

    /// The color of a tint kind in a biome; either may be another mod's.
    pub fn biome_tint(&mut self, biome: &Biome, kind: &TintKind, color: u32) {
        unsafe { (host().reg_biome_tint)(self.raw, Str::new(biome.name()), Str::new(kind.name()), color) }
    }

    /// An entity category: at most `cap` of it spawn naturally near a player; with `despawn` > 0
    /// they vanish when no player is within that many blocks.
    pub fn entity_category(&mut self, category: &EntityCategory, cap: u32, despawn: f32) {
        unsafe { (host().reg_entity_category)(self.raw, Str::new(category.name()), cap, despawn) }
    }

    /// Registers a recipe kind this mod owns, and how the recipe viewer shows it.
    pub fn recipe_kind<'a>(&'a mut self, kind: &RecipeKind) -> RecipeKindBuilder<'a> {
        RecipeKindBuilder { raw: self.raw, name: kind.name().to_string(), input_grid: [0; 2], output_grid: [0; 2], info: String::new(), info_values: Vec::new(), display: abi::Callback::NONE, _p: std::marker::PhantomData }
    }

    /// Shaped recipe: `pattern` rows use the characters in `keys`; ingredients are item names or
    /// `#tag`. More results and data with the builder it returns.
    pub fn shaped<'a>(&'a mut self, kind: &RecipeKind, output: &str, count: u32, pattern: &[&str], keys: &[(char, &str)]) -> RecipeBuilder<'a> {
        let keys = keys.iter().map(|(c, n)| (*c, self.ingredient(n))).collect();
        let output = self.ingredient(output);
        RecipeBuilder { kind: kind.name().to_string(), pattern: pattern.iter().map(|p| p.to_string()).collect(), keys, inputs: Vec::new(), counts: Vec::new(), output, count, time: 0, outputs: Vec::new(), values: Vec::new(), flags: 0, reg: self }
    }

    /// Shapeless recipe or a processing recipe (inputs used up over `time` ticks). Inputs are
    /// items, `#tags` or `fluid:<liquid>`.
    pub fn shapeless<'a>(&'a mut self, kind: &RecipeKind, output: &str, count: u32, inputs: &[&str], time: u32) -> RecipeBuilder<'a> {
        let inputs = inputs.iter().map(|n| self.ingredient(n)).collect();
        let output = self.ingredient(output);
        RecipeBuilder { kind: kind.name().to_string(), pattern: Vec::new(), keys: Vec::new(), inputs, counts: Vec::new(), output, count, time, outputs: Vec::new(), values: Vec::new(), flags: 0, reg: self }
    }

    fn ingredient(&self, n: &str) -> String {
        match n.strip_prefix("fluid:") {
            Some(f) => format!("fluid:{}", qualify(self.ns, f)),
            None if n.starts_with('#') => n.to_string(),
            None => qualify(self.ns, n),
        }
    }
}

/// A recipe being registered; sent when dropped.
pub struct RecipeBuilder<'a> {
    reg: &'a mut Registry,
    kind: String,
    pattern: Vec<String>,
    keys: Vec<(char, String)>,
    inputs: Vec<String>,
    counts: Vec<u32>,
    output: String,
    count: u32,
    time: u32,
    outputs: Vec<(String, u32, f32)>,
    values: Vec<Pending>,
    flags: u32,
}

impl RecipeBuilder<'_> {
    /// How many of each input it takes, in the order of the inputs (millibuckets for fluids).
    pub fn input_amounts(mut self, counts: &[u32]) -> Self {
        self.counts = counts.to_vec();
        self
    }
    /// Another result (an item or `fluid:<liquid>`).
    pub fn output(self, item: &str, count: u32) -> Self {
        self.chance_output(item, count, 1.0)
    }
    /// A result that comes out with chance `chance` (0..1) each time.
    pub fn chance_output(mut self, item: &str, count: u32, chance: f32) -> Self {
        let item = self.reg.ingredient(item);
        self.outputs.push((item, count, chance));
        self
    }
    /// Data the recipe carries for the machine making it (energy per tick); the recipe
    /// viewer can show it in the kind's info line.
    pub fn value<T: Value>(mut self, comp: &Component<T>, v: T) -> Self {
        self.values.push(comp.raw_value(&v));
        self
    }
    /// Works, but the recipe viewer doesn't list it.
    pub fn hidden(mut self) -> Self {
        self.flags |= abi::recipe_flags::HIDDEN;
        self
    }
}

impl Drop for RecipeBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let pattern: Vec<Str> = self.pattern.iter().map(|p| Str::new(p)).collect();
        let keys: Vec<abi::RecipeKey> = self.keys.iter().map(|(c, n)| abi::RecipeKey { ch: *c as u32, item: Str::new(n) }).collect();
        let inputs: Vec<Str> = self.inputs.iter().map(|n| Str::new(n)).collect();
        let outputs: Vec<abi::RecipeOutput> = self.outputs.iter().map(|(n, c, ch)| abi::RecipeOutput { item: Str::new(n), count: *c, chance: *ch }).collect();
        let mut scratch = Vec::new();
        let values = crate::content::raw_values_pub(&self.values, &mut scratch);
        let desc = abi::RecipeDesc {
            kind: Str::new(&self.kind),
            pattern: Slice::new(&pattern),
            keys: Slice::new(&keys),
            inputs: Slice::new(&inputs),
            input_counts: Slice::new(&self.counts),
            output: Str::new(&self.output),
            count: self.count,
            time: self.time,
            outputs: Slice::new(&outputs),
            values: Slice::new(&values),
            flags: self.flags,
        };
        unsafe { (host().reg_recipe)(self.reg.raw, &desc) }
    }
}

/// A recipe kind being registered; sent when dropped.
pub struct RecipeKindBuilder<'a> {
    raw: *mut abi::Registrar,
    name: String,
    input_grid: [u32; 2],
    output_grid: [u32; 2],
    info: String,
    info_values: Vec<String>,
    display: abi::Callback,
    _p: std::marker::PhantomData<&'a mut Registry>,
}

impl RecipeKindBuilder<'_> {
    /// The recipe viewer shows inputs as a grid this big, whatever a recipe uses (a crafting
    /// table's 3 x 3).
    pub fn inputs(mut self, cols: u32, rows: u32) -> Self {
        self.input_grid = [cols, rows];
        self
    }
    pub fn outputs(mut self, cols: u32, rows: u32) -> Self {
        self.output_grid = [cols, rows];
        self
    }
    /// A line under each recipe: translation `key`, its `{0}`.. filled with `values`: `time`
    /// (seconds) or components the recipe carries (`mymod:energy_per_tick`).
    pub fn info_line(mut self, key: &str, values: &[&str]) -> Self {
        self.info = key.to_string();
        self.info_values = values.iter().map(|v| v.to_string()).collect();
        self
    }
    /// Builds each recipe yourself instead of the usual inputs, arrow, outputs:
    /// `ui.recipe_inputs()`, `ui.recipe_outputs()` and `ui.recipe_input(i)` place its slots.
    pub fn layout(mut self, f: impl Fn(&mut Ui, &Recipe) + Send + Sync + 'static) -> Self {
        self.display = callback(move |a: &mut abi::RecipeUiArgs| {
            let mut ui = Ui::from_raw(a.ui, a.client);
            let r = Recipe { kind: unsafe { a.kind.as_str() }.to_string(), index: a.recipe };
            f(&mut ui, &r);
        });
        self
    }
}

impl Drop for RecipeKindBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let values: Vec<Str> = self.info_values.iter().map(|v| Str::new(v)).collect();
        let desc = abi::RecipeKindDesc {
            name: Str::new(&self.name),
            input_grid: self.input_grid,
            output_grid: self.output_grid,
            info: Str::new(&self.info),
            info_values: Slice::new(&values),
            display: self.display,
        };
        unsafe { (host().reg_recipe_kind)(self.raw, &desc) }
    }
}

/// One recipe of a kind, found by `recipe_find` (or shown by a kind's `display`).
pub struct Recipe {
    kind: String,
    index: u32,
}

/// The first recipe of `kind` whose inputs, with their counts, are all among `stacks`.
pub fn recipe_find(kind: &RecipeKind, stacks: &[ItemStack]) -> Option<Recipe> {
    let n = unsafe { (host().recipe_find)(Str::new(kind.name()), stacks.as_ptr(), stacks.len() as u32) };
    (n > 0).then(|| Recipe { kind: kind.name().to_string(), index: n - 1 })
}

impl Recipe {
    fn info(&self) -> abi::RecipeInfo {
        let mut out = abi::RecipeInfo::default();
        unsafe { (host().recipe_info)(Str::new(&self.kind), self.index, &mut out) };
        out
    }
    /// Its time in ticks.
    pub fn time(&self) -> u32 {
        self.info().time
    }
    /// Its results with their chances; fluids are not stacks and show as empty ones.
    pub fn outputs(&self) -> Vec<(ItemStack, f32)> {
        (0..self.info().outputs)
            .map(|i| {
                let (mut s, mut chance) = (ItemStack::EMPTY, 1.0);
                unsafe { (host().recipe_output)(Str::new(&self.kind), self.index, i, &mut s, &mut chance) };
                (s, chance)
            })
            .collect()
    }
    /// A value given with `RecipeBuilder::value`.
    pub fn value<T: Value>(&self, comp: &Component<T>) -> Option<T> {
        let mut buf = vec![0u8; std::mem::size_of::<T>()];
        let n = unsafe { (host().recipe_value)(Str::new(&self.kind), self.index, Str::new(comp.name()), buf.as_mut_ptr(), buf.len() as u32) };
        (n as usize == buf.len()).then(|| unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const T) })
    }
    /// Takes its inputs out of `stacks` (the ones `recipe_find` was given); false, and nothing
    /// taken, if they are not all there.
    pub fn consume(&self, stacks: &mut [ItemStack]) -> bool {
        unsafe { (host().recipe_consume)(Str::new(&self.kind), self.index, stacks.as_mut_ptr(), stacks.len() as u32) != 0 }
    }
}

/// A natural spawning rule (see `abi::SpawnDesc`):
/// `r.spawn("pig").biome("#animals").ground("#animal_spawn").light(9, 15).group(2, 3).chance(0.05)`.
pub struct SpawnBuilder<'a> {
    raw: *mut abi::Registrar,
    names: [String; 4],
    desc: abi::SpawnDesc,
    _p: std::marker::PhantomData<&'a mut Registry>,
}

impl Registry {
    pub fn spawn<'a>(&'a mut self, entity: &Entity) -> SpawnBuilder<'a> {
        let entity = entity.name().to_string();
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

type BlockLook = Box<dyn for<'r> FnOnce(crate::BlockBuilder<'r>) -> crate::BlockBuilder<'r>>;

/// A machine in one go: the block (its look and mining from `block`), an entity of the same
/// name holding its inventory and state (ticking with `on_tick`), components on that entity,
/// and the menu showing its slot groups above the player's inventory, opened by using the block.
///
/// ```ignore
/// r.machine(&FURNACE, &FURNACE_MENU)
///     .block(|b| b.textures(..).hardness(3.5))
///     .group("input", Slots::single())
///     .group("fuel", Slots::single().accepts("fuel"))
///     .group("output", Slots::single().output().large())
///     .attach(&SMELTING)
///     .screen(|ui| ..);
/// r.system(&SMELTING, tick_furnaces);   // the work: every furnace's state at once
/// ```
pub struct MachineBuilder<'a> {
    reg: &'a mut Registry,
    block_handle: &'static Block,
    menu: &'static Menu,
    block: Option<BlockLook>,
    groups: Vec<(String, Slots)>,
    recipes: Vec<String>,
    with: Vec<Box<dyn FnOnce(&mut Registry, &Entity)>>,
    screen: Option<Box<dyn Fn(&mut Ui) + Send + Sync>>,
}

impl Registry {
    /// The machine's entity kind is named like its block: `Entity::named("furnace")`.
    pub fn machine<'a>(&'a mut self, block: &'static Block, menu: &'static Menu) -> MachineBuilder<'a> {
        MachineBuilder { block_handle: block, reg: self, menu, block: None, groups: Vec::new(), recipes: Vec::new(), with: Vec::new(), screen: None }
    }
}

impl MachineBuilder<'_> {
    pub fn block(mut self, f: impl for<'r> FnOnce(crate::BlockBuilder<'r>) -> crate::BlockBuilder<'r> + 'static) -> Self {
        self.block = Some(Box::new(f));
        self
    }
    /// Recipes of this kind are made here: clicking an `arrow` bar of its screen shows them, and
    /// the recipe viewer shows the machine next to them.
    pub fn recipes(mut self, kind: &RecipeKind) -> Self {
        self.recipes.push(kind.name().to_string());
        self
    }
    /// A group of the machine's slots (its inventory holds them all, in this order). The
    /// machine's tick finds them with `w.inventory(id).group(name)`.
    pub fn group(mut self, name: &str, slots: Slots) -> Self {
        self.groups.push((name.to_string(), slots));
        self
    }
    /// Every machine of this kind carries `component` (declared with `r.entity_component`),
    /// starting at its declared value.
    pub fn attach<T: Value>(mut self, component: &'static Component<T>) -> Self {
        self.with.push(Box::new(move |r, kind| r.attach(kind, component)));
        self
    }
    /// `attach` with the value this machine starts with.
    pub fn attach_with<T: Value>(mut self, component: &'static Component<T>, value: T) -> Self {
        self.with.push(Box::new(move |r, kind| r.attach_with(kind, component, value)));
        self
    }
    /// The menu screen: the machine's groups by name (`ui.group`), the player's inventory with
    /// `ui.player_inventory(..)`.
    pub fn screen(mut self, f: impl Fn(&mut Ui) + Send + Sync + 'static) -> Self {
        self.screen = Some(Box::new(f));
        self
    }
}

impl Drop for MachineBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let (block, menu, reg) = (self.block_handle, self.menu, &mut *self.reg);
        let full = block.name().to_string();
        let kind = Entity::generated(full.clone());
        let look = self.block.take().unwrap_or_else(|| Box::new(|b| b));
        look(reg.block(block)).opens(menu);
        drop(reg.entity(kind).flags(entity_flags::NO_GRAVITY | entity_flags::INVULNERABLE));
        for f in self.with.drain(..) {
            f(reg, kind);
        }
        // The inventory holds every group of the machine's own, wherever they start.
        let mut size = 0;
        let mut next = 0;
        for (_, s) in &self.groups {
            if s.owner.is_none_or(|o| o == abi::slot_owner::BLOCK) {
                let start = if s.start == u32::MAX { next } else { s.start };
                next = start + s.cols * s.rows;
                size = size.max(next);
            }
        }
        reg.block_entity(block, kind, size);
        let mut m = reg.menu(menu);
        for (g, s) in self.groups.drain(..) {
            let ns = m.reg.ns;
            m.groups.push(ns, &g, s, abi::slot_owner::BLOCK);
        }
        m.entity = full.clone();
        m.recipes = std::mem::take(&mut self.recipes);
        let m = m.player_inventory();
        match self.screen.take() {
            Some(s) => drop(m.screen(s)),
            None => drop(m),
        }
    }
}

/// The engine's entity categories (`.hostile()` and `.passive()` pick the first two).
pub mod entity_categories {
    use crate::EntityCategory;
    pub static HOSTILE: EntityCategory = EntityCategory::named("core:hostile");
    pub static PASSIVE: EntityCategory = EntityCategory::named("core:passive");
    pub static MISC: EntityCategory = EntityCategory::named("core:misc");
}

pub struct ScreenBuilder<'a> {
    raw: *mut abi::Registrar,
    name: String,
    desc: abi::ScreenDesc,
    _p: std::marker::PhantomData<&'a mut Registry>,
}

impl ScreenBuilder<'_> {
    /// Single player stops while it is open (the pause and settings screens).
    pub fn pauses(mut self) -> Self {
        self.desc.flags |= abi::screen_flags::PAUSES;
        self
    }
    /// Escape doesn't close it (the death screen).
    pub fn keep_on_escape(mut self) -> Self {
        self.desc.flags |= abi::screen_flags::KEEP_ON_ESCAPE;
        self
    }
    /// It closes when the player respawns.
    pub fn closes_on_respawn(mut self) -> Self {
        self.desc.flags |= abi::screen_flags::CLOSES_ON_RESPAWN;
        self
    }
    /// Like the chat screen: the HUD leaves the messages to it and Enter sends the text of its
    /// input of class `chat`.
    pub fn shows_chat(mut self) -> Self {
        self.desc.flags |= abi::screen_flags::SHOWS_CHAT;
        self
    }
    /// Keys pressed while it is on top, by their settings name (`Enter`, `KeyE`), before
    /// Escape and the other global keys.
    pub fn on_key(mut self, f: impl Fn(&crate::ClientView, &str) + Send + Sync + 'static) -> Self {
        self.desc.on_key = crate::callback(move |a: &mut abi::ScreenKeyArgs| f(&crate::ClientView::from_raw(a.client), unsafe { a.key.as_str() }));
        self
    }
}

impl Drop for ScreenBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let desc = abi::ScreenDesc { name: Str::new(&self.name), ..self.desc };
        unsafe { (host().reg_screen)(self.raw, &desc) }
    }
}
