use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::abi::{self, Layer, Model, Slice, Str, Versioned, block_flags as bf};
use crate::component::{self as c, Component, Value};
use crate::{World, callback, host, qualify};

/// Registry kinds a handle can point at.
pub mod kind {
    pub trait Kind {
        const KIND: u32;
    }
    macro_rules! kinds {
        ($($n:ident = $v:expr),*) => {
            $(pub struct $n;
            impl Kind for $n {
                const KIND: u32 = $v;
            })*
        };
    }
    kinds!(Block = crate::abi::entry_kind::BLOCK, Item = crate::abi::entry_kind::ITEM, Entity = crate::abi::entry_kind::ENTITY, Biome = crate::abi::entry_kind::BIOME, Dimension = crate::abi::entry_kind::DIMENSION, Menu = crate::abi::entry_kind::MENU);
}

const UNRESOLVED: u32 = u32::MAX - 1;

/// Handle to registered content, valid once the engine froze the registries. Either bound when
/// registering (`static STONE: Block = Block::new();` + `r.block("stone").handle(&STONE)`) or
/// looked up by full name on first use (`static STONE: Block = Block::named("base:stone");`),
/// which also works for content of other mods.
pub struct Handle<K: kind::Kind> {
    id: AtomicU32,
    count: AtomicU32,
    name: &'static str,
    _k: PhantomData<fn() -> K>,
}

impl<K: kind::Kind> Handle<K> {
    pub const fn new() -> Self {
        Handle { id: AtomicU32::new(u32::MAX), count: AtomicU32::new(1), name: "", _k: PhantomData }
    }

    pub const fn named(name: &'static str) -> Self {
        Handle { id: AtomicU32::new(UNRESOLVED), count: AtomicU32::new(1), name, _k: PhantomData }
    }

    /// For blocks, the first state. `u32::MAX` if unknown.
    #[inline]
    pub fn id(&self) -> u32 {
        let id = self.id.load(Ordering::Relaxed);
        if id != UNRESOLVED { id } else { self.resolve() }
    }

    #[cold]
    fn resolve(&self) -> u32 {
        let mut count = 0;
        let id = unsafe { (host().find)(K::KIND, Str::new(self.name), &mut count) };
        if id != u32::MAX {
            self.count.store(count, Ordering::Relaxed);
            self.id.store(id, Ordering::Relaxed);
        } else if unsafe { !(host().components)().is_null() } {
            // Frozen and still unknown: a typo or a missing mod. Say so once.
            crate::error!("no {} named {}", ["block", "item", "entity", "biome", "dimension", "menu"].get(K::KIND as usize).unwrap_or(&"entry"), self.name);
            self.id.store(u32::MAX, Ordering::Relaxed);
        }
        id
    }

    /// The `i`-th state of a block.
    #[inline]
    pub fn state(&self, i: u32) -> u32 {
        self.id() + i
    }

    /// True if `state` belongs to this block.
    #[inline]
    pub fn is(&self, state: u32) -> bool {
        state.wrapping_sub(self.id()) < self.count.load(Ordering::Relaxed)
    }

    /// State index inside the block for a state id of this block.
    #[inline]
    pub fn index(&self, state: u32) -> u32 {
        state - self.id()
    }

    pub(crate) fn cell(&self) -> *const AtomicU32 {
        &self.id
    }

    pub(crate) fn set_count(&self, n: u32) {
        self.count.store(n, Ordering::Relaxed);
    }
}

impl<K: kind::Kind> Default for Handle<K> {
    fn default() -> Self {
        Self::new()
    }
}

fn cell<K: kind::Kind>(h: Option<&'static Handle<K>>) -> *const AtomicU32 {
    h.map_or(std::ptr::null(), |h| h.cell())
}

pub type Block = Handle<kind::Block>;
pub type Item = Handle<kind::Item>;
pub type Biome = Handle<kind::Biome>;
pub type Dimension = Handle<kind::Dimension>;

/// A component value waiting to be sent with a descriptor. Text is kept as a `String` until then.
#[derive(Clone)]
pub struct Pending {
    name: String,
    schema: String,
    bytes: Vec<u8>,
    text: Option<String>,
}

impl Pending {
    pub(crate) fn value(name: &str, schema: String, bytes: Vec<u8>) -> Pending {
        Pending { name: name.to_string(), schema, bytes, text: None }
    }
    fn text(name: &str, text: &str) -> Pending {
        Pending { name: name.to_string(), schema: "str".into(), bytes: Vec::new(), text: Some(text.to_string()) }
    }
    fn tag(name: &str) -> Pending {
        Pending { name: name.to_string(), schema: String::new(), bytes: Vec::new(), text: None }
    }
}

/// `CompValue`s pointing into `list`; for text the `Str` bytes live in `scratch`.
pub(crate) fn raw_values_pub(list: &[Pending], scratch: &mut Vec<Vec<u8>>) -> Vec<abi::CompValue> {
    raw_values(list, scratch)
}

fn raw_values(list: &[Pending], scratch: &mut Vec<Vec<u8>>) -> Vec<abi::CompValue> {
    scratch.clear();
    for p in list {
        let bytes = match &p.text {
            Some(t) => c::pod_bytes(&Str::new(t)),
            None => p.bytes.clone(),
        };
        scratch.push(bytes);
    }
    list.iter().zip(scratch.iter()).map(|(p, b)| abi::CompValue { name: Str::new(&p.name), schema: Str::new(&p.schema), value: Slice::new(b) }).collect()
}

/// What `Registry::set_on` applies to; names without a namespace are this mod's.
pub enum Target<'a> {
    /// The block (all states, or one with `name#3`) and item of that name.
    Name(&'a str),
    /// Blocks and items with a matching tag (`*` matches any text).
    Tagged(&'a str),
    Entity(&'a str),
    Biome(&'a str),
    Dimension(&'a str),
}

pub struct Registry {
    pub(crate) raw: *mut abi::Registrar,
    pub(crate) ns: &'static str,
}

impl Registry {
    pub fn namespace(&self) -> &'static str {
        self.ns
    }

    /// Registers a block and, unless `.no_item()` is called, an item that places it (and carries
    /// the block's components). Texture defaults to `block/<name>` for all faces.
    pub fn block<'a>(&'a mut self, name: &str) -> BlockBuilder<'a> {
        let tex = qualify(self.ns, &format!("block/{name}"));
        let name = qualify(self.ns, name);
        BlockBuilder {
            reg: self,
            name,
            handle: None,
            states: vec![StateDef::new(&tex)],
            components: Vec::new(),
            drop: String::new(),
            drop_count: 1,
            item: true,
            item_handle: None,
            item_texture: String::new(),
            callbacks: BlockCallbacks::default(),
            replace: false,
        }
    }

    /// Texture defaults to `item/<name>`.
    pub fn item<'a>(&'a mut self, name: &str) -> ItemBuilder<'a> {
        ItemBuilder {
            _p: PhantomData,
            raw: self.raw,
            name: qualify(self.ns, name),
            texture: qualify(self.ns, &format!("item/{name}")),
            handle: None,
            block: String::new(),
            components: Vec::new(),
            on_use: abi::Callback::NONE,
            ns: self.ns,
        }
    }

    pub fn biome<'a>(&'a mut self, name: &str) -> BiomeBuilder<'a> {
        BiomeBuilder { _p: PhantomData, raw: self.raw, name: qualify(self.ns, name), handle: None, desc: abi::BiomeDesc::DEFAULT, components: Vec::new() }
    }

    /// Creative inventory tab named `tab.<ns>.<name>` in the language files, shown with `icon`.
    /// Blocks and items join with `.tab(name)`; untabbed items land in a tab named after the mod.
    pub fn creative_tab(&mut self, name: &str, icon: &str) {
        let (name, icon) = (qualify(self.ns, name), qualify(self.ns, icon));
        unsafe { (host().reg_creative_tab)(self.raw, Str::new(&name), Str::new(&icon)) }
    }

    /// Ore veins of `block` in any mod's dimension, e.g.
    /// `r.ore("base:overworld", "tin_ore").y(0, 64).veins(6).size(8).replace("#stone")`.
    pub fn ore<'a>(&'a mut self, dimension: &str, block: &str) -> OreBuilder<'a> {
        OreBuilder {
            raw: self.raw,
            ns: self.ns,
            dimension: qualify(self.ns, dimension),
            block: qualify(self.ns, block),
            replace: String::new(),
            desc: abi::OreDesc::DEFAULT,
            _p: PhantomData,
        }
    }

    pub fn dimension<'a>(&'a mut self, name: &str) -> DimensionBuilder<'a> {
        DimensionBuilder { raw: self.raw, name: qualify(self.ns, name), handle: None, desc: abi::DimensionDesc::DEFAULT, components: Vec::new(), _p: PhantomData }
    }

    /// Sets a component on the block and item called `name`, whoever registered them:
    /// `r.set("base:coal", &FUEL, 1600)`. The last mod in load order wins; overrides are logged.
    pub fn set<T: Value>(&mut self, name: &str, comp: &Component<T>, v: T) {
        self.set_on(Target::Name(name), comp, v)
    }

    /// Sets a component on every block and item with a matching tag: `r.set_tagged("logs", &FUEL, 300)`,
    /// `r.set_tagged("cooked_meat/*", &HEAL, 2.0)`.
    pub fn set_tagged<T: Value>(&mut self, pattern: &str, comp: &Component<T>, v: T) {
        self.set_on(Target::Tagged(pattern), comp, v)
    }

    pub fn set_on<T: Value>(&mut self, target: Target, comp: &Component<T>, v: T) {
        let p = comp.raw_value(&v);
        self.send_set(target, &p, false);
    }

    /// Adds `tag` to blocks and items of any mod, e.g. `r.tag("ingots/tin", &["othermod:tin_ingot"])`
    /// or `r.tag("plant_soil", &["othermod:mud"])`.
    pub fn tag(&mut self, tag: &str, names: &[&str]) {
        for n in names {
            self.send_set(Target::Name(n), &Pending::tag(tag), false);
        }
    }

    pub fn tag_on(&mut self, target: Target, tag: &str) {
        self.send_set(target, &Pending::tag(tag), false);
    }

    /// Takes a component or tag away from entries of any mod.
    pub fn remove(&mut self, target: Target, component: &str) {
        self.send_set(target, &Pending::tag(component), true);
    }

    fn send_set(&mut self, target: Target, value: &Pending, remove: bool) {
        use abi::set_target as st;
        let (kind, name) = match target {
            Target::Name(n) => (st::NAME, qualify(self.ns, n)),
            Target::Tagged(t) => (st::TAGGED, t.to_string()),
            Target::Entity(n) => (st::ENTITY, qualify(self.ns, n)),
            Target::Biome(n) => (st::BIOME, qualify(self.ns, n)),
            Target::Dimension(n) => (st::DIMENSION, qualify(self.ns, n)),
        };
        let mut scratch = Vec::new();
        let values = raw_values(std::slice::from_ref(value), &mut scratch);
        let desc = abi::SetDesc { target_kind: kind, target: Str::new(&name), value: values[0], remove: remove as u32, ..abi::SetDesc::DEFAULT };
        unsafe { (host().reg_set)(self.raw, &desc) }
    }
}

#[derive(Clone)]
struct StateDef {
    flags: u32,
    light: [u8; 3],
    filter: [u8; 3],
    opacity: u8,
    tint: u8,
    tint_faces: u8,
    layer: Layer,
    level: u8,
    model: Model,
    textures: [String; 6],
    boxes: Vec<([f32; 3], [f32; 3], [String; 6])>,
    collision: Vec<[f32; 6]>,
    hardness: f32,
    friction: f32,
    drag: f32,
    support_side: u8,
    support: String,
    components: Vec<Pending>,
}

impl StateDef {
    fn new(tex: &str) -> Self {
        let d = abi::StateDesc::DEFAULT;
        StateDef {
            flags: d.flags,
            light: d.light,
            filter: d.filter,
            opacity: d.opacity,
            tint: d.tint,
            tint_faces: d.tint_faces,
            layer: d.layer,
            level: d.level,
            model: d.model,
            textures: std::array::from_fn(|_| tex.to_string()),
            boxes: Vec::new(),
            collision: Vec::new(),
            hardness: d.hardness,
            friction: d.friction,
            drag: d.drag,
            support_side: d.support_side,
            support: String::new(),
            components: Vec::new(),
        }
    }
}

#[derive(Default)]
struct BlockCallbacks {
    on_use: Option<abi::Callback>,
    on_placed: Option<abi::Callback>,
    on_broken: Option<abi::Callback>,
    on_random_tick: Option<abi::Callback>,
    on_neighbor: Option<abi::Callback>,
    on_scheduled: Option<abi::Callback>,
    placement: Option<abi::Callback>,
}

/// Per-state settings, used directly and through `BlockBuilder::states`.
pub struct State<'a> {
    def: &'a mut StateDef,
    ns: &'static str,
}

macro_rules! state_setters {
    () => {
        /// Seconds to mine by hand.
        pub fn hardness(mut self, h: f32) -> Self {
            self.each(|s, _| s.hardness = h);
            self
        }
        pub fn unbreakable(mut self) -> Self {
            self.each(|s, _| s.hardness = -1.0);
            self
        }
        /// White light. 15 is full strength; more is allowed and reaches further.
        pub fn light(mut self, level: u8) -> Self {
            self.each(|s, _| s.light = [level; 3]);
            self
        }
        pub fn light_rgb(mut self, r: u8, g: u8, b: u8) -> Self {
            self.each(|s, _| s.light = [r, g, b]);
            self
        }
        /// Per-channel absorption of passing light (stained glass).
        pub fn filter(mut self, r: u8, g: u8, b: u8) -> Self {
            self.each(|s, _| s.filter = [r, g, b]);
            self
        }
        /// Same texture on every face, relative names are under `block/`.
        pub fn texture(mut self, t: &str) -> Self {
            self.each(|s, ns| s.textures = std::array::from_fn(|_| block_tex(ns, t)));
            self
        }
        /// Top, sides, bottom.
        pub fn textures(mut self, top: &str, side: &str, bottom: &str) -> Self {
            self.each(|s, ns| {
                let (t, si, b) = (block_tex(ns, top), block_tex(ns, side), block_tex(ns, bottom));
                s.textures = [si.clone(), si.clone(), b, t, si.clone(), si];
            });
            self
        }
        /// Faces in -X, +X, -Y, +Y, -Z, +Z order.
        pub fn faces(mut self, f: [&str; 6]) -> Self {
            self.each(|s, ns| s.textures = std::array::from_fn(|i| block_tex(ns, f[i])));
            self
        }
        /// Not opaque, rendered alpha-tested, lets light through (glass, leaves).
        pub fn cutout(mut self) -> Self {
            self.each(|s, _| {
                s.flags &= !bf::OPAQUE;
                s.layer = Layer::Cutout;
                s.opacity = 0;
            });
            self
        }
        /// Hide faces between neighbours of the same block (glass panes of a window).
        pub fn cull_same(mut self) -> Self {
            self.each(|s, _| s.flags |= bf::CULL_SAME);
            self
        }
        /// Like cutout but keeps light partially (leaves).
        pub fn opacity(mut self, o: u8) -> Self {
            self.each(|s, _| s.opacity = o);
            self
        }
        pub fn translucent(mut self) -> Self {
            self.each(|s, _| {
                s.flags &= !bf::OPAQUE;
                s.flags |= bf::CULL_SAME;
                s.layer = Layer::Translucent;
                s.opacity = 0;
            });
            self
        }
        /// Needs a block tagged `tag` underneath (`#` optional): placed only there, and breaks
        /// off when it goes.
        pub fn needs_below(self, tag: &str) -> Self {
            self.needs(2, tag)
        }
        /// Needs the neighbour on `face` (-X, +X, -Y, +Y, -Z, +Z) to be tagged `tag`, or solid
        /// if `tag` is empty.
        pub fn needs(mut self, face: u32, tag: &str) -> Self {
            let tag = tag.trim_start_matches('#').to_string();
            self.each(|s, _| {
                s.support_side = face as u8;
                s.support = tag.clone();
            });
            self
        }
        /// A plant: tagged `plantable`, cross model, no collision, replaced by placing a block.
        pub fn plant(mut self) -> Self {
            self.each(|s, _| {
                s.flags = bf::REPLACEABLE;
                s.model = Model::Cross;
                s.layer = Layer::Cutout;
                s.opacity = 0;
                s.hardness = 0.0;
                s.components.push(Pending::tag("plantable"));
            });
            self
        }
        pub fn not_solid(mut self) -> Self {
            self.each(|s, _| s.flags &= !bf::SOLID);
            self
        }
        pub fn replaceable(mut self) -> Self {
            self.each(|s, _| s.flags |= bf::REPLACEABLE);
            self
        }
        pub fn climbable(mut self) -> Self {
            self.each(|s, _| s.flags |= bf::CLIMBABLE);
            self
        }
        pub fn tint_grass(mut self) -> Self {
            self.each(|s, _| s.tint = 1);
            self
        }
        /// Grass colour on the top face only (the sides carry their own colour).
        pub fn tint_grass_top(mut self) -> Self {
            self.each(|s, _| {
                s.tint = 1;
                s.tint_faces = 1 << 3;
            });
            self
        }
        pub fn tint_foliage(mut self) -> Self {
            self.each(|s, _| s.tint = 2);
            self
        }
        pub fn friction(mut self, f: f32) -> Self {
            self.each(|s, _| s.friction = f);
            self
        }
        pub fn drag(mut self, d: f32) -> Self {
            self.each(|s, _| s.drag = d);
            self
        }
        /// Custom model made of boxes in 1/16 units, all faces use `texture`. Collision follows the
        /// union bounds unless `collision` is set.
        pub fn model_box(mut self, min: [f32; 3], max: [f32; 3], texture: &str) -> Self {
            self.each(|s, ns| {
                if s.model != Model::Boxes {
                    s.boxes.clear();
                }
                s.model = Model::Boxes;
                s.flags &= !bf::OPAQUE;
                s.opacity = 0;
                s.layer = Layer::Cutout;
                let t = block_tex(ns, texture);
                s.boxes.push((min, max, std::array::from_fn(|_| t.clone())));
            });
            self
        }
        /// Box with per-face textures (-X, +X, -Y, +Y, -Z, +Z).
        pub fn model_box_faces(mut self, min: [f32; 3], max: [f32; 3], f: [&str; 6]) -> Self {
            self.each(|s, ns| {
                if s.model != Model::Boxes {
                    s.boxes.clear();
                }
                s.model = Model::Boxes;
                s.flags &= !bf::OPAQUE;
                s.opacity = 0;
                s.layer = Layer::Cutout;
                s.boxes.push((min, max, std::array::from_fn(|i| block_tex(ns, f[i]))));
            });
            self
        }
        pub fn collision(mut self, min: [f32; 3], max: [f32; 3]) -> Self {
            self.each(|s, _| s.collision.push([min[0], min[1], min[2], max[0], max[1], max[2]]));
            self
        }
        /// Liquid model with the given level (0 = source, 7 = thinnest).
        pub fn liquid(mut self, level: u8) -> Self {
            self.each(|s, _| {
                s.flags = bf::LIQUID | bf::REPLACEABLE | bf::NOT_TARGETABLE;
                s.model = Model::Liquid;
                s.layer = Layer::Translucent;
                s.level = level;
                s.opacity = 2;
                s.hardness = -1.0;
                s.drag = 0.5;
            });
            self
        }
        pub fn tint_water(mut self) -> Self {
            self.each(|s, _| s.tint = 3);
            self
        }
        /// Air-like: invisible, no collision, not targetable.
        pub fn air(mut self) -> Self {
            self.each(|s, _| {
                s.flags = bf::REPLACEABLE | bf::NOT_TARGETABLE;
                s.model = Model::None;
                s.opacity = 0;
                s.layer = Layer::Opaque;
            });
            self
        }
        pub fn flags(mut self, add: u32, remove: u32) -> Self {
            self.each(|s, _| s.flags = (s.flags | add) & !remove);
            self
        }
    };
}

fn block_tex(ns: &str, t: &str) -> String {
    if t.contains(':') { t.to_string() } else { format!("{ns}:block/{t}") }
}

impl<'a> State<'a> {
    fn each(&mut self, f: impl Fn(&mut StateDef, &str)) {
        f(self.def, self.ns)
    }
    state_setters!();

    /// A component on this state only (on top of the block's).
    pub fn set<T: Value>(self, comp: &Component<T>, v: T) -> Self {
        self.def.components.push(comp.raw_value(&v));
        self
    }
    pub fn tag(self, tag: &str) -> Self {
        self.def.components.push(Pending::tag(tag));
        self
    }
}

pub struct BlockBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    handle: Option<&'static Block>,
    states: Vec<StateDef>,
    components: Vec<Pending>,
    drop: String,
    drop_count: u32,
    item: bool,
    item_handle: Option<&'static Item>,
    item_texture: String,
    callbacks: BlockCallbacks,
    replace: bool,
}

impl<'a> BlockBuilder<'a> {
    fn each(&mut self, f: impl Fn(&mut StateDef, &str)) {
        let ns = self.reg.ns;
        for s in &mut self.states {
            f(s, ns);
        }
    }

    state_setters!();

    /// Fills `h` with the block's first state once the registries are frozen.
    pub fn handle(mut self, h: &'static Block) -> Self {
        self.handle = Some(h);
        self
    }

    /// Makes `n` states copied from the current settings, then customises each one.
    pub fn states(mut self, n: u32, f: impl Fn(u32, State) -> State) -> Self {
        let base = self.states[0].clone();
        self.states = (0..n).map(|_| base.clone()).collect();
        let ns = self.reg.ns;
        for (i, s) in self.states.iter_mut().enumerate() {
            f(i as u32, State { def: s, ns });
        }
        self
    }

    /// Tools of this class (`tools/<class>` items) mine it at their speed: tags `mineable/<class>`.
    pub fn mineable(mut self, class: &str) -> Self {
        self.components.push(Pending::tag(&format!("mineable/{class}")));
        self
    }

    /// Drops nothing unless mined with a fitting tool of at least this level (wood 0, stone 1,
    /// iron 2, diamond 3), like vanilla's "requires correct tool". Levels above 0 are modern
    /// vanilla's tags (`needs_stone_tool`, `needs_iron_tool`, `needs_diamond_tool`), so other mods
    /// can add them with `r.tag` too; higher tiers use `harvest_level`.
    pub fn needs_tool(mut self, level: u32) -> Self {
        self.components.push(Pending::tag(abi::comp::NEEDS_TOOL));
        match level {
            0 => {}
            1..=3 => self.components.push(Pending::tag(["needs_stone_tool", "needs_iron_tool", "needs_diamond_tool"][level as usize - 1])),
            _ => self.components.push(c::HARVEST_LEVEL.raw_value(&level)),
        }
        self
    }

    pub fn tag(mut self, tag: &str) -> Self {
        self.components.push(Pending::tag(tag));
        self
    }

    /// A component on every state of the block (and its item).
    pub fn set<T: Value>(mut self, comp: &Component<T>, v: T) -> Self {
        self.components.push(comp.raw_value(&v));
        self
    }

    pub fn set_text(mut self, comp: &Component<c::Text>, text: &str) -> Self {
        self.components.push(Pending::text(comp.name(), text));
        self
    }

    /// Creative inventory tab of the block's item, see `Registry::creative_tab`.
    pub fn tab(mut self, tab: &str) -> Self {
        let tag = format!("tab/{}", qualify(self.reg.ns, tab));
        self.components.push(Pending::tag(&tag));
        self
    }

    pub fn drops(mut self, item: &str, count: u32) -> Self {
        self.drop = qualify(self.reg.ns, item);
        self.drop_count = count;
        self
    }

    pub fn drops_nothing(mut self) -> Self {
        self.drop = "-".into();
        self
    }

    /// The block's item shows this flat texture (a block texture name, or `ns:path`) instead of
    /// the block model.
    pub fn item_texture(mut self, t: &str) -> Self {
        self.item_texture = if t.contains(':') { t.to_string() } else { format!("{}:block/{t}", self.reg.ns) };
        self
    }

    pub fn no_item(mut self) -> Self {
        self.item = false;
        self
    }

    /// Replaces the block with this name from a mod listed in `deps` (its item stays).
    pub fn replace(mut self) -> Self {
        self.replace = true;
        self.item = false;
        self
    }

    /// Registers the block item into this handle (otherwise it is only reachable by name).
    pub fn item_handle(mut self, h: &'static Item) -> Self {
        self.item_handle = Some(h);
        self
    }

    pub fn random_ticks(mut self, f: impl Fn(&mut BlockEvent) + Send + Sync + 'static) -> Self {
        self.each(|s, _| s.flags |= bf::RANDOM_TICKS);
        self.callbacks.on_random_tick = Some(callback(move |a: &mut abi::BlockEventArgs| f(BlockEvent::wrap(a))));
        self
    }

    pub fn on_use(mut self, f: impl Fn(&mut Use) -> bool + Send + Sync + 'static) -> Self {
        self.callbacks.on_use = Some(callback(move |a: &mut abi::UseArgs| {
            let handled = f(Use::wrap(a));
            a.handled = handled as u32;
        }));
        self
    }

    pub fn on_placed(mut self, f: impl Fn(&mut BlockEvent) + Send + Sync + 'static) -> Self {
        self.callbacks.on_placed = Some(callback(move |a: &mut abi::BlockEventArgs| f(BlockEvent::wrap(a))));
        self
    }

    pub fn on_broken(mut self, f: impl Fn(&mut BlockEvent) + Send + Sync + 'static) -> Self {
        self.callbacks.on_broken = Some(callback(move |a: &mut abi::BlockEventArgs| f(BlockEvent::wrap(a))));
        self
    }

    pub fn on_neighbor(mut self, f: impl Fn(&mut BlockEvent) + Send + Sync + 'static) -> Self {
        self.callbacks.on_neighbor = Some(callback(move |a: &mut abi::BlockEventArgs| f(BlockEvent::wrap(a))));
        self
    }

    pub fn on_scheduled(mut self, f: impl Fn(&mut BlockEvent) + Send + Sync + 'static) -> Self {
        self.callbacks.on_scheduled = Some(callback(move |a: &mut abi::BlockEventArgs| f(BlockEvent::wrap(a))));
        self
    }

    /// Picks the placed state from the default state index 0: returns the index to use. The
    /// state ids come from the block's first state the engine passes in.
    pub fn placement(mut self, f: impl Fn(&Place) -> u32 + Send + Sync + 'static) -> Self {
        self.callbacks.placement = Some(callback(move |a: &mut abi::PlaceArgs| {
            let first = a.state;
            let i = f(Place::wrap(a));
            a.state = first + i;
        }));
        self
    }
}

impl Drop for BlockBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let boxes: Vec<Vec<abi::ModelBox>> = self
            .states
            .iter()
            .map(|s| s.boxes.iter().map(|(min, max, t)| abi::ModelBox { min: *min, max: *max, textures: std::array::from_fn(|i| Str::new(&t[i])), ..abi::ModelBox::DEFAULT }).collect())
            .collect();
        let mut scratch: Vec<Vec<Vec<u8>>> = vec![Vec::new(); self.states.len()];
        let state_values: Vec<Vec<abi::CompValue>> = self.states.iter().zip(scratch.iter_mut()).map(|(s, sc)| raw_values(&s.components, sc)).collect();
        let states: Vec<abi::StateDesc> = self
            .states
            .iter()
            .zip(&boxes)
            .zip(&state_values)
            .map(|((s, b), v)| abi::StateDesc {
                flags: s.flags,
                light: s.light,
                filter: s.filter,
                opacity: s.opacity,
                tint: s.tint,
                tint_faces: s.tint_faces,
                layer: s.layer,
                level: s.level,
                model: s.model,
                textures: std::array::from_fn(|i| Str::new(&s.textures[i])),
                boxes: Slice::new(b),
                collision: Slice::new(&s.collision),
                hardness: s.hardness,
                friction: s.friction,
                drag: s.drag,
                support_side: s.support_side,
                support: Str::new(&s.support),
                components: Slice::new(v),
                ..abi::StateDesc::DEFAULT
            })
            .collect();
        let mut scratch = Vec::new();
        let values = raw_values(&self.components, &mut scratch);
        let cb = &self.callbacks;
        let desc = abi::BlockDesc {
            name: Str::new(&self.name),
            cell: cell(self.handle),
            states: Slice::new(&states),
            components: Slice::new(&values),
            drop: Str::new(&self.drop),
            drop_count: self.drop_count,
            on_use: cb.on_use.unwrap_or(abi::Callback::NONE),
            on_placed: cb.on_placed.unwrap_or(abi::Callback::NONE),
            on_broken: cb.on_broken.unwrap_or(abi::Callback::NONE),
            on_random_tick: cb.on_random_tick.unwrap_or(abi::Callback::NONE),
            on_neighbor: cb.on_neighbor.unwrap_or(abi::Callback::NONE),
            on_scheduled: cb.on_scheduled.unwrap_or(abi::Callback::NONE),
            placement: cb.placement.unwrap_or(abi::Callback::NONE),
            replace: self.replace as u32,
            ..abi::BlockDesc::DEFAULT
        };
        if let Some(h) = self.handle {
            h.set_count(self.states.len() as u32);
        }
        unsafe { (host().reg_block)(self.reg.raw, &desc) };

        if self.item {
            // The item gets the block's components from the engine.
            let item = ItemBuilder {
                _p: PhantomData,
                raw: self.reg.raw,
                name: self.name.clone(),
                texture: std::mem::take(&mut self.item_texture),
                handle: self.item_handle,
                block: self.name.clone(),
                components: Vec::new(),
                on_use: abi::Callback::NONE,
                ns: self.reg.ns,
            };
            drop(item);
        }
    }
}

pub struct ItemBuilder<'a> {
    _p: PhantomData<&'a mut Registry>,
    raw: *mut abi::Registrar,
    ns: &'static str,
    name: String,
    texture: String,
    handle: Option<&'static Item>,
    block: String,
    components: Vec<Pending>,
    on_use: abi::Callback,
}

impl ItemBuilder<'_> {
    pub fn handle(mut self, h: &'static Item) -> Self {
        self.handle = Some(h);
        self
    }
    pub fn texture(mut self, t: &str) -> Self {
        self.texture = if t.contains(':') { t.to_string() } else { format!("{}:item/{t}", self.ns) };
        self
    }
    /// Stack size (`max_stack`).
    pub fn stack(self, n: u32) -> Self {
        self.set(&c::MAX_STACK, n)
    }
    /// Uses before it breaks (`durability`); damageable items do not stack.
    pub fn durability(self, d: u32) -> Self {
        self.set(&c::DURABILITY, d).set(&c::MAX_STACK, 1)
    }
    /// A tool of `class` (tag `tools/<class>`) mining blocks `mineable/<class>` at `speed`, good
    /// for blocks needing up to `level`.
    pub fn tool(self, class: &str, level: u32, speed: f32) -> Self {
        let tag = format!("tools/{class}");
        self.tag(&tag).set(&c::TOOL_LEVEL, level).set(&c::MINING_SPEED, speed)
    }
    pub fn attack(self, damage: f32) -> Self {
        self.set(&c::ATTACK_DAMAGE, damage)
    }
    pub fn places(mut self, block: &str) -> Self {
        self.block = qualify(self.ns, block);
        self
    }
    pub fn tag(mut self, tag: &str) -> Self {
        self.components.push(Pending::tag(tag));
        self
    }
    pub fn set<T: Value>(mut self, comp: &Component<T>, v: T) -> Self {
        self.components.push(comp.raw_value(&v));
        self
    }
    pub fn set_text(mut self, comp: &Component<c::Text>, text: &str) -> Self {
        self.components.push(Pending::text(comp.name(), text));
        self
    }
    /// Creative inventory tab, see `Registry::creative_tab`.
    pub fn tab(self, tab: &str) -> Self {
        let tag = format!("tab/{}", qualify(self.ns, tab));
        self.tag(&tag)
    }
    /// Shown on the wearer's model while in an equipment slot: `parts` from `worn_parts`, `texture`
    /// in the wearer's skin layout.
    pub fn worn(self, parts: u32, inflate: f32, texture: &str) -> Self {
        let tex = qualify(self.ns, texture);
        self.set(&c::WORN, abi::Worn { roles: parts, inflate }).set_text(&c::WORN_TEXTURE, &tex)
    }

    pub fn on_use(mut self, f: impl Fn(&mut ItemUse) -> bool + Send + Sync + 'static) -> Self {
        self.on_use = callback(move |a: &mut abi::ItemUseArgs| {
            let handled = f(ItemUse::wrap(a));
            a.handled = handled as u32;
        });
        self
    }
}

impl Drop for ItemBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let mut scratch = Vec::new();
        let values = raw_values(&self.components, &mut scratch);
        let desc = abi::ItemDesc {
            name: Str::new(&self.name),
            cell: cell(self.handle),
            texture: Str::new(&self.texture),
            block: Str::new(&self.block),
            components: Slice::new(&values),
            on_use: self.on_use,
            ..abi::ItemDesc::DEFAULT
        };
        unsafe { (host().reg_item)(self.raw, &desc) }
    }
}

pub struct BiomeBuilder<'a> {
    _p: PhantomData<&'a mut Registry>,
    raw: *mut abi::Registrar,
    name: String,
    handle: Option<&'static Biome>,
    desc: abi::BiomeDesc,
    components: Vec<Pending>,
}

impl BiomeBuilder<'_> {
    pub fn handle(mut self, h: &'static Biome) -> Self {
        self.handle = Some(h);
        self
    }
    /// Grass, foliage and water colours (`0xRRGGBB`).
    pub fn colors(mut self, grass: u32, foliage: u32, water: u32) -> Self {
        (self.desc.grass, self.desc.foliage, self.desc.water) = (grass, foliage, water);
        self
    }
    pub fn climate(mut self, temperature: f32, rainfall: f32) -> Self {
        (self.desc.temperature, self.desc.rainfall) = (temperature, rainfall);
        self
    }
    pub fn tag(mut self, tag: &str) -> Self {
        self.components.push(Pending::tag(tag));
        self
    }
    pub fn set<T: Value>(mut self, comp: &Component<T>, v: T) -> Self {
        self.components.push(comp.raw_value(&v));
        self
    }
}

impl Drop for BiomeBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let mut scratch = Vec::new();
        let values = raw_values(&self.components, &mut scratch);
        let desc = abi::BiomeDesc { name: Str::new(&self.name), cell: cell(self.handle), components: Slice::new(&values), ..self.desc };
        unsafe { (host().reg_biome)(self.raw, &desc) }
    }
}

pub struct DimensionBuilder<'a> {
    raw: *mut abi::Registrar,
    name: String,
    handle: Option<&'static Dimension>,
    desc: abi::DimensionDesc,
    components: Vec<Pending>,
    _p: PhantomData<&'a mut Registry>,
}

impl DimensionBuilder<'_> {
    pub fn handle(mut self, h: &'static Dimension) -> Self {
        self.handle = Some(h);
        self
    }
    /// Must be a pure function of its input: it runs in parallel and must give the same cube for the
    /// same seed and position on every machine.
    pub fn generator(mut self, f: impl Fn(&mut Gen) + Send + Sync + 'static) -> Self {
        self.desc.generator = callback(move |a: &mut abi::GenArgs| f(Gen::wrap(a)));
        self
    }
    pub fn spawn(mut self, f: impl Fn(u64) -> [i32; 3] + Send + Sync + 'static) -> Self {
        self.desc.spawn = callback(move |a: &mut abi::SpawnArgs| a.pos = f(a.seed));
        self
    }
    /// Inclusive limits in cubes (16 blocks).
    pub fn height(mut self, min_cube: i32, max_cube: i32) -> Self {
        self.desc.min_cube_y = min_cube;
        self.desc.max_cube_y = max_cube;
        self
    }
    /// Everything at or above this cube y is empty, so sky light there needs no generation.
    pub fn sky_above(mut self, cube_y: i32) -> Self {
        self.desc.sky_cube_y = cube_y;
        self
    }
    pub fn no_sky_light(mut self) -> Self {
        self.desc.sky_light = 0;
        self
    }
    /// Blocks per second squared.
    pub fn gravity(mut self, g: f32) -> Self {
        self.desc.gravity = g;
        self
    }
    pub fn day_length(mut self, ticks: u32) -> Self {
        self.desc.day_length = ticks;
        self
    }
    pub fn fixed_time(mut self, time: u32) -> Self {
        self.desc.day_length = 0;
        self.desc.fixed_time = time;
        self
    }
    pub fn sky(mut self, sky: u32, fog: u32) -> Self {
        self.desc.sky_color = sky;
        self.desc.fog_color = fog;
        self
    }
    pub fn ambient(mut self, level: u32) -> Self {
        self.desc.ambient = level;
        self
    }
    pub fn tag(mut self, tag: &str) -> Self {
        self.components.push(Pending::tag(tag));
        self
    }
    pub fn set<T: Value>(mut self, comp: &Component<T>, v: T) -> Self {
        self.components.push(comp.raw_value(&v));
        self
    }
}

impl Drop for DimensionBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let mut scratch = Vec::new();
        let values = raw_values(&self.components, &mut scratch);
        let desc = abi::DimensionDesc { name: Str::new(&self.name), cell: cell(self.handle), components: Slice::new(&values), ..self.desc };
        unsafe { (host().reg_dimension)(self.raw, &desc) }
    }
}

pub struct OreBuilder<'a> {
    raw: *mut abi::Registrar,
    ns: &'static str,
    dimension: String,
    block: String,
    replace: String,
    desc: abi::OreDesc,
    _p: PhantomData<&'a mut Registry>,
}

impl OreBuilder<'_> {
    /// Heights the veins start at, inclusive.
    pub fn y(mut self, min: i32, max: i32) -> Self {
        (self.desc.min_y, self.desc.max_y) = (min, max);
        self
    }
    /// Veins started per cube.
    pub fn veins(mut self, n: u32) -> Self {
        self.desc.veins = n;
        self
    }
    /// Blocks per vein.
    pub fn size(mut self, n: u32) -> Self {
        self.desc.blocks = n;
        self
    }
    /// Blocks or `#tags` the ore may replace, space separated.
    pub fn replace(mut self, blocks: &str) -> Self {
        let names: Vec<String> = blocks.split_whitespace().map(|b| if b.starts_with('#') { b.to_string() } else { qualify(self.ns, b) }).collect();
        self.replace = names.join(" ");
        self
    }
}

impl Drop for OreBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let desc = abi::OreDesc { dimension: Str::new(&self.dimension), block: Str::new(&self.block), replace: Str::new(&self.replace), ..self.desc };
        unsafe { (host().reg_ore)(self.raw, &desc) }
    }
}

/// Generation context for one cube.
pub struct Gen {
    args: abi::GenArgs,
}

impl Gen {
    pub(crate) fn wrap(a: &mut abi::GenArgs) -> &mut Gen {
        unsafe { &mut *(a as *mut abi::GenArgs as *mut Gen) }
    }
    pub fn seed(&self) -> u64 {
        self.args.seed
    }
    /// Cube coordinates (block coords = cube * 16).
    pub fn cube(&self) -> [i32; 3] {
        self.args.cube
    }
    pub fn origin(&self) -> [i32; 3] {
        let c = self.args.cube;
        [c[0] * 16, c[1] * 16, c[2] * 16]
    }
    pub fn blocks(&mut self) -> &mut [u32; 4096] {
        unsafe { &mut *(self.args.blocks as *mut [u32; 4096]) }
    }
    pub fn biomes(&mut self) -> &mut [u8; 256] {
        unsafe { &mut *(self.args.biomes as *mut [u8; 256]) }
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, state: u32) {
        self.blocks()[(y * 16 + z) * 16 + x] = state;
    }
    #[inline]
    pub fn get(&mut self, x: usize, y: usize, z: usize) -> u32 {
        self.blocks()[(y * 16 + z) * 16 + x]
    }
}

pub struct BlockEvent {
    args: abi::BlockEventArgs,
}

impl BlockEvent {
    fn wrap(a: &mut abi::BlockEventArgs) -> &mut BlockEvent {
        unsafe { &mut *(a as *mut abi::BlockEventArgs as *mut BlockEvent) }
    }
    pub fn world(&self) -> World {
        World::from_raw(self.args.world)
    }
    pub fn pos(&self) -> [i32; 3] {
        self.args.pos
    }
    pub fn state(&self) -> u32 {
        self.args.state
    }
    /// The neighbour that changed, for `on_neighbor`.
    pub fn other(&self) -> [i32; 3] {
        self.args.other
    }
    pub fn entity(&self) -> u64 {
        self.args.entity
    }
}

pub struct Use {
    args: abi::UseArgs,
}

impl Use {
    fn wrap(a: &mut abi::UseArgs) -> &mut Use {
        unsafe { &mut *(a as *mut abi::UseArgs as *mut Use) }
    }
    pub fn world(&self) -> World {
        World::from_raw(self.args.world)
    }
    pub fn pos(&self) -> [i32; 3] {
        self.args.pos
    }
    pub fn state(&self) -> u32 {
        self.args.state
    }
    pub fn face(&self) -> u32 {
        self.args.face
    }
    pub fn player(&self) -> u64 {
        self.args.player
    }
}

pub struct Place {
    args: abi::PlaceArgs,
}

impl Place {
    fn wrap(a: &mut abi::PlaceArgs) -> &Place {
        unsafe { &*(a as *mut abi::PlaceArgs as *const Place) }
    }
    pub fn pos(&self) -> [i32; 3] {
        self.args.pos
    }
    /// Face of the block that was clicked: 0..6 in -X, +X, -Y, +Y, -Z, +Z order.
    pub fn face(&self) -> u32 {
        self.args.face
    }
    pub fn look(&self) -> [f32; 3] {
        self.args.look
    }
    /// Horizontal facing the player looks towards: 0 = -Z (north), 1 = +X, 2 = +Z, 3 = -X.
    pub fn horizontal(&self) -> u32 {
        let [x, _, z] = self.args.look;
        if x.abs() > z.abs() {
            if x > 0.0 { 1 } else { 3 }
        } else if z > 0.0 {
            2
        } else {
            0
        }
    }
}

pub struct ItemUse {
    args: abi::ItemUseArgs,
}

impl ItemUse {
    fn wrap(a: &mut abi::ItemUseArgs) -> &mut ItemUse {
        unsafe { &mut *(a as *mut abi::ItemUseArgs as *mut ItemUse) }
    }
    pub fn world(&self) -> World {
        World::from_raw(self.args.world)
    }
    pub fn player(&self) -> u64 {
        self.args.player
    }
    pub fn target(&self) -> Option<([i32; 3], u32)> {
        (self.args.has_target != 0).then_some((self.args.pos, self.args.face))
    }
    pub fn stack(&self) -> crate::ItemStack {
        self.args.stack
    }
    /// Replaces the held stack (use count 0 to consume it entirely).
    pub fn set_stack(&mut self, s: crate::ItemStack) {
        self.args.stack = s;
    }
}

impl<K: kind::Kind> Handle<K> {
    /// Full name it was declared with (`Handle::named`), empty for bound handles.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// A liquid: its block (16 states) and flow rules, optionally a filled bucket item.
///
/// ```ignore
/// r.liquid("oil").handle(&OIL).texture("oil_still").delay(15).drop_off(2).bucket("oil_bucket", "base:bucket");
/// r.liquid_reaction("#hot", "#water").source("obsidian").flowing("cobblestone").falls_into("stone");
/// ```
pub struct LiquidBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    handle: Option<&'static Block>,
    texture: String,
    tint: bool,
    light: [u8; 3],
    opacity: u8,
    drag: f32,
    components: Vec<Pending>,
    desc: abi::LiquidDesc,
    bucket: Option<(String, String)>,
}

impl LiquidBuilder<'_> {
    pub fn handle(mut self, h: &'static Block) -> Self {
        self.handle = Some(h);
        self
    }
    /// Still texture, relative names are under `block/`.
    pub fn texture(mut self, t: &str) -> Self {
        self.texture = t.to_string();
        self
    }
    /// Coloured by the biome's water colour.
    pub fn tint_water(mut self) -> Self {
        self.tint = true;
        self
    }
    pub fn light(mut self, level: u8) -> Self {
        self.light = [level; 3];
        self
    }
    /// How much light it absorbs per block.
    pub fn opacity(mut self, o: u8) -> Self {
        self.opacity = o;
        self
    }
    /// Movement speed multiplier inside.
    pub fn drag(mut self, d: f32) -> Self {
        self.drag = d;
        self
    }
    /// Ticks between flow steps (water 5, lava 30).
    pub fn delay(mut self, ticks: u32) -> Self {
        self.desc.delay = ticks;
        self
    }
    /// Level lost per block spread (water 1: 7 blocks, lava 2: 3 blocks).
    pub fn drop_off(mut self, n: u32) -> Self {
        self.desc.drop_off = n;
        self
    }
    /// How far it looks for a way down (water 4, lava 2).
    pub fn slope_find(mut self, n: u32) -> Self {
        self.desc.slope_find = n;
        self
    }
    /// Sources needed side by side to make a new one (water 2), 0 = never.
    pub fn infinite(mut self, sources: u32) -> Self {
        self.desc.infinite = sources;
        self
    }
    /// Rises instead of falling.
    pub fn rises(mut self) -> Self {
        self.desc.density = -self.desc.density.abs().max(1);
        self
    }
    /// Roughly Kelvin; 1000 and more burns what touches it.
    pub fn temperature(mut self, t: u32) -> Self {
        self.desc.temperature = t;
        self
    }
    /// A tag on every state (`water`, `hot` for reactions).
    pub fn tag(mut self, tag: &str) -> Self {
        self.components.push(Pending::tag(tag));
        self
    }
    pub fn set<T: Value>(mut self, comp: &Component<T>, v: T) -> Self {
        self.components.push(comp.raw_value(&v));
        self
    }
    /// Also registers `item`: `empty` (a container item, any mod's) filled with this liquid. The
    /// engine fills and pours it; no code needed.
    pub fn bucket(mut self, item: &str, empty: &str) -> Self {
        self.bucket = Some((item.to_string(), qualify(self.reg.ns, empty)));
        self
    }
}

impl Drop for LiquidBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let short = self.name.rsplit(':').next().unwrap_or(&self.name).to_string();
        let tex = if self.texture.is_empty() { format!("{short}_still") } else { self.texture.clone() };
        let (tint, light, opacity, drag) = (self.tint, self.light, self.opacity, self.drag);
        let mut b = self.reg.block(&short).texture(&tex).states(16, |i, s| s.liquid(if i >= 8 { 0 } else { i as u8 })).light_rgb(light[0], light[1], light[2]).opacity(opacity).drag(drag).no_item().drops_nothing();
        if tint {
            b = b.tint_water();
        }
        if let Some(h) = self.handle {
            b = b.handle(h);
        }
        b.components = std::mem::take(&mut self.components);
        drop(b);
        let desc = abi::LiquidDesc { name: Str::new(&self.name), ..self.desc };
        unsafe { (host().reg_liquid)(self.reg.raw, &desc) };
        if let Some((item, empty)) = self.bucket.take() {
            self.reg.item(&item).stack(1).set_text(&c::LIQUID, &self.name).set_text(&c::REMAINDER, &empty).tab("tools");
        }
    }
}

pub struct ReactionBuilder<'a> {
    reg: &'a mut Registry,
    names: [String; 5],
}

impl ReactionBuilder<'_> {
    /// What a source of the liquid becomes.
    pub fn source(mut self, block: &str) -> Self {
        self.names[2] = qualify(self.reg.ns, block);
        self
    }
    /// What flowing liquid becomes.
    pub fn flowing(mut self, block: &str) -> Self {
        self.names[3] = qualify(self.reg.ns, block);
        self
    }
    /// What the touched liquid becomes when this one flows down into it.
    pub fn falls_into(mut self, block: &str) -> Self {
        self.names[4] = qualify(self.reg.ns, block);
        self
    }
}

impl Drop for ReactionBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let n = &self.names;
        let desc = abi::ReactionDesc { liquid: Str::new(&n[0]), touching: Str::new(&n[1]), source: Str::new(&n[2]), flowing: Str::new(&n[3]), falls_into: Str::new(&n[4]), ..abi::ReactionDesc::DEFAULT };
        unsafe { (host().reg_liquid_reaction)(self.reg.raw, &desc) }
    }
}

fn block_or_tag(ns: &str, s: &str) -> String {
    if s.starts_with('#') { s.to_string() } else { qualify(ns, s) }
}

impl Registry {
    /// A liquid the engine flows; see `LiquidBuilder`. Defaults are vanilla water's numbers.
    pub fn liquid<'a>(&'a mut self, name: &str) -> LiquidBuilder<'a> {
        LiquidBuilder {
            name: qualify(self.ns, name),
            reg: self,
            handle: None,
            texture: String::new(),
            tint: false,
            light: [0; 3],
            opacity: 2,
            drag: 0.5,
            components: Vec::new(),
            desc: abi::LiquidDesc::DEFAULT,
            bucket: None,
        }
    }

    /// When `liquid` touches `touching` (names or `#tags` of liquid blocks), see `ReactionBuilder`.
    pub fn liquid_reaction<'a>(&'a mut self, liquid: &str, touching: &str) -> ReactionBuilder<'a> {
        let names = [block_or_tag(self.ns, liquid), block_or_tag(self.ns, touching), String::new(), String::new(), String::new()];
        ReactionBuilder { reg: self, names }
    }
}

/// Liquid id of a block state (`None` for other blocks).
pub fn liquid_of(state: u32) -> Option<u32> {
    let l = unsafe { (host().liquid_of)(state) };
    (l != u32::MAX).then_some(l)
}
