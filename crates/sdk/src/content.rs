use std::sync::atomic::{AtomicU32, Ordering};

use crate::abi::{self, Layer, Model, Slice, Str, block_flags as bf};
use crate::{World, callback, host, qualify};

/// Handle to registered content, filled in when the engine freezes registries.
/// Declare as `static`: `pub static STONE: Block = Block::new();`
pub struct Handle {
    id: AtomicU32,
    count: AtomicU32,
}

impl Handle {
    pub const fn new() -> Self {
        Handle { id: AtomicU32::new(u32::MAX), count: AtomicU32::new(1) }
    }

    /// For blocks, the first state.
    #[inline]
    pub fn id(&self) -> u32 {
        self.id.load(Ordering::Relaxed)
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
}

impl Default for Handle {
    fn default() -> Self {
        Self::new()
    }
}

pub type Block = Handle;
pub type Item = Handle;
pub type Biome = Handle;
pub type Dimension = Handle;

pub struct Registry {
    pub(crate) raw: *mut abi::Registrar,
    pub(crate) ns: &'static str,
}

impl Registry {
    pub fn namespace(&self) -> &'static str {
        self.ns
    }

    /// Registers a block and, unless `.no_item()` is called, an item that places it.
    /// Texture defaults to `block/<name>` for all faces.
    pub fn block<'a>(&'a mut self, name: &str, handle: &'static Block) -> BlockBuilder<'a> {
        let tex = qualify(self.ns, &format!("block/{name}"));
        let name = qualify(self.ns, name);
        BlockBuilder {
            reg: self,
            name,
            handle,
            states: vec![StateDef::new(&tex)],
            tool: String::new(),
            harvest_level: 0,
            tags: Vec::new(),
            props: Vec::new(),
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
    pub fn item<'a>(&'a mut self, name: &str, handle: &'static Item) -> ItemBuilder<'a> {
        ItemBuilder {
            _p: std::marker::PhantomData,
            raw: self.raw,
            name: qualify(self.ns, name),
            texture: qualify(self.ns, &format!("item/{name}")),
            handle: Some(handle),
            max_stack: 64,
            durability: 0,
            block: String::new(),
            tool: String::new(),
            tool_level: 0,
            tool_speed: 1.0,
            attack: 1.0,
            tags: Vec::new(),
            props: Vec::new(),
            on_use: abi::Callback::NONE,
            ns: self.ns,
        }
    }

    pub fn biome(&mut self, name: &str, handle: &'static Biome, grass: u32, foliage: u32, water: u32, temperature: f32, rainfall: f32) {
        let name = qualify(self.ns, name);
        let desc = abi::BiomeDesc {
            name: Str::new(&name),
            cell: handle.cell(),
            grass,
            foliage,
            water,
            temperature,
            rainfall,
        };
        unsafe { (host().reg_biome)(self.raw, &desc) }
    }

    /// Creative inventory tab named `tab.<ns>.<name>` in the language files, shown with `icon`.
    /// Blocks and items join with `.tab(name)`; untabbed items land in a tab named after the mod.
    pub fn creative_tab(&mut self, name: &str, icon: &str) {
        let (name, icon) = (qualify(self.ns, name), qualify(self.ns, icon));
        unsafe { (host().reg_creative_tab)(self.raw, Str::new(&name), Str::new(&icon)) }
    }

    /// Ore veins of `block` in any mod's dimension, e.g.
    /// `r.ore("base:overworld", "tin_ore").y(0, 64).veins(6).size(8).replace("base:stone")`.
    pub fn ore<'a>(&'a mut self, dimension: &str, block: &str) -> OreBuilder<'a> {
        OreBuilder {
            raw: self.raw,
            ns: self.ns,
            dimension: qualify(self.ns, dimension),
            block: qualify(self.ns, block),
            replace: String::new(),
            desc: (i32::MIN, i32::MAX, 4, 8),
            _p: std::marker::PhantomData,
        }
    }

    pub fn dimension<'a>(&'a mut self, name: &str, handle: &'static Dimension) -> DimensionBuilder<'a> {
        DimensionBuilder {
            raw: self.raw,
            name: qualify(self.ns, name),
            desc: abi::DimensionDesc {
                name: Str::EMPTY,
                cell: handle.cell(),
                generator: abi::Callback::NONE,
                spawn: abi::Callback::NONE,
                min_cube_y: i32::MIN,
                max_cube_y: i32::MAX,
                sky_cube_y: i32::MAX,
                sky_light: 1,
                gravity: 32.0,
                day_length: 24000,
                fixed_time: 6000,
                sky_color: 0x78a7ff,
                fog_color: 0xc0d8ff,
                ambient: 0,
            },
            _p: std::marker::PhantomData,
        }
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
}

impl StateDef {
    fn new(tex: &str) -> Self {
        StateDef {
            flags: bf::SOLID | bf::OPAQUE,
            light: [0; 3],
            filter: [0; 3],
            opacity: 15,
            tint: 0,
            tint_faces: 0x3F,
            layer: Layer::Opaque,
            level: 0,
            model: Model::Cube,
            textures: std::array::from_fn(|_| tex.to_string()),
            boxes: Vec::new(),
            collision: Vec::new(),
            hardness: 1.0,
            friction: 0.6,
            drag: 1.0,
            support_side: 255,
            support: String::new(),
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
        /// Plants: cross model, no collision, replaceable by placement.
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
        pub fn plant(mut self) -> Self {
            self.each(|s, _| {
                s.flags = bf::REPLACEABLE;
                s.model = Model::Cross;
                s.layer = Layer::Cutout;
                s.opacity = 0;
                s.hardness = 0.0;
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
}

pub struct BlockBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    handle: &'static Block,
    states: Vec<StateDef>,
    tool: String,
    harvest_level: u32,
    tags: Vec<String>,
    props: Vec<(String, Vec<u8>)>,
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

    /// Tool class that mines it fast, and the level needed for drops.
    pub fn tool(mut self, class: &str, level: u32) -> Self {
        self.tool = class.to_string();
        self.harvest_level = level;
        self
    }

    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_string());
        self
    }

    /// Creative inventory tab of the block's item, see `Registry::creative_tab`.
    pub fn tab(mut self, tab: &str) -> Self {
        self.tags.push(format!("tab/{}", qualify(self.reg.ns, tab)));
        self
    }

    /// Arbitrary typed data other mods can read by key, e.g. `"base:flammable"`.
    pub fn prop<T: Copy>(mut self, key: &str, value: T) -> Self {
        self.props.push((key.to_string(), pod_bytes(&value)));
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

    /// Picks the placed state. Gets the default state index 0 and returns the index to use.
    pub fn placement(mut self, f: impl Fn(&Place) -> u32 + Send + Sync + 'static) -> Self {
        let handle = self.handle;
        self.callbacks.placement = Some(callback(move |a: &mut abi::PlaceArgs| {
            let i = f(Place::wrap(a));
            a.state = handle.state(i);
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
            .map(|s| {
                s.boxes
                    .iter()
                    .map(|(min, max, t)| abi::ModelBox {
                        min: *min,
                        max: *max,
                        textures: std::array::from_fn(|i| Str::new(&t[i])),
                    })
                    .collect()
            })
            .collect();
        let states: Vec<abi::StateDesc> = self
            .states
            .iter()
            .zip(&boxes)
            .map(|(s, b)| abi::StateDesc {
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
            })
            .collect();
        let tags: Vec<Str> = self.tags.iter().map(|t| Str::new(t)).collect();
        let props: Vec<abi::Prop> =
            self.props.iter().map(|(k, v)| abi::Prop { key: Str::new(k), value: Slice::new(v) }).collect();
        let cb = &self.callbacks;
        let desc = abi::BlockDesc {
            name: Str::new(&self.name),
            cell: self.handle.cell(),
            states: Slice::new(&states),
            tool: Str::new(&self.tool),
            harvest_level: self.harvest_level,
            tags: Slice::new(&tags),
            props: Slice::new(&props),
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
        };
        self.handle.count.store(self.states.len() as u32, Ordering::Relaxed);
        unsafe { (host().reg_block)(self.reg.raw, &desc) };

        if self.item {
            let mut item = ItemBuilder {
                _p: std::marker::PhantomData,
                raw: self.reg.raw,
                name: self.name.clone(),
                texture: std::mem::take(&mut self.item_texture),
                handle: self.item_handle,
                max_stack: 64,
                durability: 0,
                block: self.name.clone(),
                tool: String::new(),
                tool_level: 0,
                tool_speed: 1.0,
                attack: 1.0,
                tags: Vec::new(),
                props: Vec::new(),
                on_use: abi::Callback::NONE,
                ns: self.reg.ns,
            };
            item.tags = self.tags.clone();
            drop(item);
        }
    }
}

pub struct ItemBuilder<'a> {
    _p: std::marker::PhantomData<&'a mut Registry>,
    raw: *mut abi::Registrar,
    ns: &'static str,
    name: String,
    texture: String,
    handle: Option<&'static Item>,
    max_stack: u32,
    durability: u32,
    block: String,
    tool: String,
    tool_level: u32,
    tool_speed: f32,
    attack: f32,
    tags: Vec<String>,
    props: Vec<(String, Vec<u8>)>,
    on_use: abi::Callback,
}

impl ItemBuilder<'_> {
    pub fn texture(mut self, t: &str) -> Self {
        self.texture = if t.contains(':') { t.to_string() } else { format!("{}:item/{t}", self.ns) };
        self
    }
    pub fn stack(mut self, n: u32) -> Self {
        self.max_stack = n;
        self
    }
    pub fn durability(mut self, d: u32) -> Self {
        self.durability = d;
        self.max_stack = 1;
        self
    }
    pub fn tool(mut self, class: &str, level: u32, speed: f32) -> Self {
        self.tool = class.to_string();
        self.tool_level = level;
        self.tool_speed = speed;
        self
    }
    pub fn attack(mut self, damage: f32) -> Self {
        self.attack = damage;
        self
    }
    pub fn places(mut self, block: &str) -> Self {
        self.block = qualify(self.ns, block);
        self
    }
    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_string());
        self
    }
    /// Creative inventory tab, see `Registry::creative_tab`.
    pub fn tab(mut self, tab: &str) -> Self {
        self.tags.push(format!("tab/{}", qualify(self.ns, tab)));
        self
    }
    pub fn prop<T: Copy>(mut self, key: &str, value: T) -> Self {
        self.props.push((key.to_string(), pod_bytes(&value)));
        self
    }
    /// Shown on the wearer's model while in an equipment slot: `parts` from `worn_parts`, `texture`
    /// in the wearer's skin layout.
    pub fn worn(mut self, parts: u32, inflate: f32, texture: &str) -> Self {
        let mut bytes = pod_bytes(&abi::Worn { roles: parts, inflate });
        bytes.extend_from_slice(qualify(self.ns, texture).as_bytes());
        self.props.push((abi::WORN.to_string(), bytes));
        self
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
        let tags: Vec<Str> = self.tags.iter().map(|t| Str::new(t)).collect();
        let props: Vec<abi::Prop> =
            self.props.iter().map(|(k, v)| abi::Prop { key: Str::new(k), value: Slice::new(v) }).collect();
        let desc = abi::ItemDesc {
            name: Str::new(&self.name),
            cell: self.handle.map_or(std::ptr::null(), |h| h.cell()),
            max_stack: self.max_stack,
            durability: self.durability,
            texture: Str::new(&self.texture),
            block: Str::new(&self.block),
            tool: Str::new(&self.tool),
            tool_level: self.tool_level,
            tool_speed: self.tool_speed,
            attack: self.attack,
            tags: Slice::new(&tags),
            props: Slice::new(&props),
            on_use: self.on_use,
        };
        unsafe { (host().reg_item)(self.raw, &desc) }
    }
}

pub struct DimensionBuilder<'a> {
    raw: *mut abi::Registrar,
    name: String,
    desc: abi::DimensionDesc,
    _p: std::marker::PhantomData<&'a mut Registry>,
}

impl DimensionBuilder<'_> {
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
}

pub struct OreBuilder<'a> {
    raw: *mut abi::Registrar,
    ns: &'static str,
    dimension: String,
    block: String,
    replace: String,
    desc: (i32, i32, u32, u32),
    _p: std::marker::PhantomData<&'a mut Registry>,
}

impl OreBuilder<'_> {
    /// Heights the veins start at, inclusive.
    pub fn y(mut self, min: i32, max: i32) -> Self {
        (self.desc.0, self.desc.1) = (min, max);
        self
    }
    /// Veins started per cube.
    pub fn veins(mut self, n: u32) -> Self {
        self.desc.2 = n;
        self
    }
    /// Blocks per vein.
    pub fn size(mut self, n: u32) -> Self {
        self.desc.3 = n;
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
        let desc = abi::OreDesc {
            dimension: Str::new(&self.dimension),
            block: Str::new(&self.block),
            replace: Str::new(&self.replace),
            min_y: self.desc.0,
            max_y: self.desc.1,
            veins: self.desc.2,
            size: self.desc.3,
        };
        unsafe { (host().reg_ore)(self.raw, &desc) }
    }
}

impl Drop for DimensionBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.desc.name = Str::new(&self.name);
        unsafe { (host().reg_dimension)(self.raw, &self.desc) }
    }
}

fn pod_bytes<T: Copy>(v: &T) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(v as *const T as *const u8, size_of::<T>()).to_vec() }
}

/// Generation context for one cube.
pub struct Gen {
    args: abi::GenArgs,
}

impl Gen {
    fn wrap(a: &mut abi::GenArgs) -> &mut Gen {
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
