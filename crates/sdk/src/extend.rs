//! Hooking into what happens without replacing anything:
//!
//! ```ignore
//! // Notifications after the fact, batched once per tick:
//! pub static EATEN: Event<Eaten> = Event::named("eaten");
//! r.on(&EATEN, |b| for e in b.iter() { b.world().damage(..) });
//! w.emit(&EATEN, Eaten { .. });
//! // May it happen? Any deny wins:
//! r.intercept(&question::BREAK_BLOCK, |q| if q.pos()[1] < 0 { Verdict::Deny } else { Verdict::Abstain });
//! // Computed values in stages, explainable:
//! r.modify(&chain::DAMAGE, stage::MULTIPLY, |m| m.value *= 0.5);
//! // Loot tables any mod can change:
//! r.loot(&OAK_LEAVES).pool(Pool::new().when(Cond::Chance(0.05)).item("sapling"));
//! r.loot_modify(&ZOMBIE).add_pool(Pool::new().item("mymod:brain"));
//! ```

use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::abi::{self, ItemStack, Slice, Str};
use crate::component::Value;
use crate::{Attribute, Chain, LootSource, Question, Registry, World, callback, host, qualify};

pub use abi::{BlockChange, Death, Hurt, ItemUsed, Pickup, PlayerEvent, attr_op, stage};

/// The engine's questions (`Registry::intercept`, `World::ask`).
pub mod question {
    use crate::Question;
    use crate::abi::question as q;
    /// `entity` the player (0 for `break_block` without one), `pos`, `state`, `item` the tool.
    pub static BREAK_BLOCK: Question = Question::named(q::BREAK_BLOCK);
    /// `entity`, `pos`, `state` to be placed, `item` used.
    pub static PLACE_BLOCK: Question = Question::named(q::PLACE_BLOCK);
    /// Right click on a block: `entity`, `pos`, `state`.
    pub static USE_BLOCK: Question = Question::named(q::USE_BLOCK);
    /// `entity` about to change dimension to `dim`.
    pub static ENTER_DIMENSION: Question = Question::named(q::ENTER_DIMENSION);
    /// `entity` (a player) picking up `item` (count in `amount`), `other` the item entity.
    pub static PICKUP_ITEM: Question = Question::named(q::PICKUP_ITEM);
    /// `entity` attacking `other` with `item`.
    pub static ATTACK: Question = Question::named(q::ATTACK);
    /// `entity` about to be hurt by `other` for `amount` of `cause`: deny to prevent it.
    pub static HURT: Question = Question::named(q::HURT);
    /// A player (`other`) right clicking `entity` holding `item`.
    pub static INTERACT_ENTITY: Question = Question::named(q::INTERACT_ENTITY);
    /// A player (`entity`) about to use the held `item`, on block `pos` when `state` isn't 0.
    pub static USE_ITEM: Question = Question::named(q::USE_ITEM);
}

/// The engine's value chains (`Registry::modify`, `World::compute`).
pub mod chain {
    use crate::Chain;
    /// Damage an entity takes (`entity` hurt by `other`, `cause`), after `on_hurt` hooks.
    pub static DAMAGE: Chain = Chain::named(crate::abi::chain::DAMAGE);
    /// Melee damage of `entity` hitting `other` holding `item`, from the item's `attack_damage`.
    pub static ATTACK_DAMAGE: Chain = Chain::named(crate::abi::chain::ATTACK_DAMAGE);
}

/// Attributes the engine reads.
pub mod attribute {
    use crate::Attribute;
    pub static MAX_HEALTH: Attribute = Attribute::named(crate::abi::attribute::MAX_HEALTH);
    /// Multiplier on walking speed (1 = the kind's speed).
    pub static MOVEMENT_SPEED: Attribute = Attribute::named(crate::abi::attribute::MOVEMENT_SPEED);
    /// Multiplier on how far mobs notice it (1; 0 in creative).
    pub static VISIBILITY: Attribute = Attribute::named(crate::abi::attribute::VISIBILITY);
}

const UNRESOLVED: u32 = u32::MAX - 1;

/// A typed event, named like any other thing a mod owns: `Event::named("eaten")` is this mod's,
/// `Event::named("base:eaten")` another's.
pub struct Event<T: Value> {
    name: &'static str,
    full: std::sync::OnceLock<String>,
    id: AtomicU32,
    _p: PhantomData<fn() -> T>,
}

impl<T: Value> Event<T> {
    pub const fn named(name: &'static str) -> Self {
        Event { name, full: std::sync::OnceLock::new(), id: AtomicU32::new(UNRESOLVED), _p: PhantomData }
    }
    pub fn name(&self) -> &str {
        self.full.get_or_init(|| crate::full_name(self.name))
    }
    /// `u32::MAX` when nobody listens.
    pub fn id(&self) -> u32 {
        let id = self.id.load(Ordering::Relaxed);
        if id != UNRESOLVED {
            return id;
        }
        if unsafe { (host().components)() }.is_null() {
            return u32::MAX;
        }
        let id = unsafe { (host().find_event)(Str::new(self.name()), Str::new(&T::schema())) };
        self.id.store(id, Ordering::Relaxed);
        id
    }
}

unsafe impl Value for ItemStack {
    fn schema() -> String {
        "{item:u32,count:u32,data:u32}".into()
    }
}
unsafe impl Value for abi::BlockChange {
    fn schema() -> String {
        "{player:u64,pos:[i32;3],state:u32,item:u32}".into()
    }
}
unsafe impl Value for abi::Death {
    fn schema() -> String {
        "{entity:u64,killer:u64,kind:u32,cause:u32,pos:[f64;3]}".into()
    }
}
unsafe impl Value for abi::Hurt {
    fn schema() -> String {
        "{entity:u64,source:u64,amount:f32,cause:u32}".into()
    }
}
unsafe impl Value for abi::PlayerEvent {
    fn schema() -> String {
        "{player:u64}".into()
    }
}
unsafe impl Value for abi::Pickup {
    fn schema() -> String {
        "{player:u64,stack:{item:u32,count:u32,data:u32}}".into()
    }
}
unsafe impl Value for abi::ItemUsed {
    fn schema() -> String {
        "{player:u64,stack:{item:u32,count:u32,data:u32},pos:[i32;3],has_target:u32}".into()
    }
}

/// The engine's events.
pub static BLOCK_BROKEN: Event<abi::BlockChange> = Event::named(abi::event::BLOCK_BROKEN);
pub static BLOCK_PLACED: Event<abi::BlockChange> = Event::named(abi::event::BLOCK_PLACED);
pub static ENTITY_DIED: Event<abi::Death> = Event::named(abi::event::ENTITY_DIED);
pub static PLAYER_JOINED: Event<abi::PlayerEvent> = Event::named(abi::event::PLAYER_JOINED);
pub static ITEM_PICKED_UP: Event<abi::Pickup> = Event::named(abi::event::ITEM_PICKED_UP);
pub static ENTITY_HURT: Event<abi::Hurt> = Event::named(abi::event::ENTITY_HURT);
/// A player used the held item and something happened (`stack` as it was before): eating,
/// emptying a bucket. Another mod reacts to food here, by the item's components.
pub static ITEM_USED: Event<abi::ItemUsed> = Event::named(abi::event::ITEM_USED);

/// Every value of one event queued during a tick, in the order they happened.
pub struct EventBatch<T> {
    world: World,
    values: Vec<T>,
}

impl<T: Copy> EventBatch<T> {
    pub fn world(&self) -> World {
        self.world
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.values.iter()
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Abstain,
    Allow,
    Deny,
}

/// A question being asked (`Registry::intercept`).
pub struct Ask {
    world: World,
    question: &'static str,
    query: *mut abi::Query,
}

impl Ask {
    pub fn world(&self) -> World {
        self.world
    }
    pub fn question(&self) -> &str {
        self.question
    }
    pub fn query(&self) -> &abi::Query {
        unsafe { &*self.query }
    }
    pub fn entity(&self) -> u64 {
        self.query().entity
    }
    pub fn pos(&self) -> [i32; 3] {
        self.query().pos
    }
    pub fn state(&self) -> u32 {
        self.query().state
    }
    pub fn item(&self) -> u32 {
        self.query().item
    }
}

/// One step of a value chain (`Registry::modify`): read the context, change `value`.
pub struct Modify {
    world: World,
    query: abi::Query,
    pub base: f64,
    pub value: f64,
}

impl Modify {
    pub fn world(&self) -> World {
        self.world
    }
    pub fn query(&self) -> &abi::Query {
        &self.query
    }
}

/// What a loot roll knows.
#[derive(Clone, Copy, Default)]
pub struct LootCtx {
    pub tool: ItemStack,
    pub killer: u64,
    pub by_player: bool,
    pub on_fire: bool,
}

/// A loot condition.
#[derive(Clone)]
pub enum Cond {
    Chance(f32),
    /// The tool carries the tag (`tools/shears`).
    ToolTag(String),
    NotToolTag(String),
    ToolLevel(u32),
    KilledByPlayer,
    OnFire,
}

impl Cond {
    pub fn tool_tag(t: &str) -> Cond {
        Cond::ToolTag(t.to_string())
    }
    fn raw(&self) -> (u32, f32, &str) {
        use abi::loot_cond as k;
        match self {
            Cond::Chance(p) => (k::CHANCE, *p, ""),
            Cond::ToolTag(t) => (k::TOOL_TAG, 1.0, t),
            Cond::NotToolTag(t) => (k::NOT_TOOL_TAG, 1.0, t),
            Cond::ToolLevel(l) => (k::TOOL_LEVEL, *l as f32, ""),
            Cond::KilledByPlayer => (k::KILLED_BY_PLAYER, 1.0, ""),
            Cond::OnFire => (k::ON_FIRE, 1.0, ""),
        }
    }
}

/// One choice of a pool.
#[derive(Clone)]
pub struct Entry {
    item: String,
    weight: u32,
    min: u32,
    max: u32,
    when: Vec<Cond>,
    smelt: String,
}

impl Entry {
    /// An item name (this mod's namespace by default) or `#tag`.
    pub fn item(name: &str) -> Entry {
        Entry { item: name.to_string(), weight: 1, min: 1, max: 1, when: Vec::new(), smelt: String::new() }
    }
    /// A choice that gives nothing (weight against the others).
    pub fn nothing() -> Entry {
        Entry::item("")
    }
    pub fn count(mut self, min: u32, max: u32) -> Entry {
        (self.min, self.max) = (min, max.max(min));
        self
    }
    pub fn weight(mut self, w: u32) -> Entry {
        self.weight = w;
        self
    }
    pub fn when(mut self, c: Cond) -> Entry {
        self.when.push(c);
        self
    }
    /// A burning victim drops it cooked by this recipe type (`base:smelting`).
    pub fn smelted(mut self, recipe_kind: &crate::RecipeKind) -> Entry {
        self.smelt = recipe_kind.name().to_string();
        self
    }
}

/// Rolls (1 by default) of one weighted choice each.
#[derive(Clone, Default)]
pub struct Pool {
    rolls: (u32, u32),
    when: Vec<Cond>,
    entries: Vec<Entry>,
}

impl Pool {
    pub fn new() -> Pool {
        Pool { rolls: (1, 1), when: Vec::new(), entries: Vec::new() }
    }
    pub fn rolls(mut self, min: u32, max: u32) -> Pool {
        self.rolls = (min, max.max(min));
        self
    }
    pub fn when(mut self, c: Cond) -> Pool {
        self.when.push(c);
        self
    }
    pub fn entry(mut self, e: Entry) -> Pool {
        self.entries.push(e);
        self
    }
    /// One of `name`.
    pub fn item(self, name: &str) -> Pool {
        self.entry(Entry::item(name))
    }
    /// `min..=max` of `name`.
    pub fn items(self, name: &str, min: u32, max: u32) -> Pool {
        self.entry(Entry::item(name).count(min, max))
    }
}

/// The ABI view of pools, kept alive while a descriptor points into it.
struct RawPools {
    conds: Vec<Vec<abi::LootCond>>,
    entries: Vec<Vec<abi::LootEntry>>,
    pools: Vec<abi::LootPool>,
    _names: Vec<String>,
}

fn raw_pools(ns: &str, pools: &[Pool]) -> RawPools {
    let mut names: Vec<String> = Vec::new();
    for p in pools {
        for e in &p.entries {
            names.push(if e.item.is_empty() || e.item.starts_with('#') { e.item.clone() } else { qualify(ns, &e.item) });
        }
    }
    let mut out = RawPools { conds: Vec::new(), entries: Vec::new(), pools: Vec::new(), _names: Vec::new() };
    let mut cond_index = Vec::new();
    for p in pools {
        let pc: Vec<abi::LootCond> = p.when.iter().map(|c| {
            let (kind, value, text) = c.raw();
            abi::LootCond { kind, value, text: Str::new(text), ..abi::LootCond::DEFAULT }
        }).collect();
        cond_index.push(out.conds.len());
        out.conds.push(pc);
        for e in &p.entries {
            out.conds.push(e.when.iter().map(|c| {
                let (kind, value, text) = c.raw();
                abi::LootCond { kind, value, text: Str::new(text), ..abi::LootCond::DEFAULT }
            }).collect());
        }
    }
    let mut n = 0;
    for (pi, p) in pools.iter().enumerate() {
        let first = cond_index[pi];
        let entries: Vec<abi::LootEntry> = p
            .entries
            .iter()
            .enumerate()
            .map(|(ei, e)| {
                let name = Str::new(&names[n + ei]);
                abi::LootEntry { item: name, weight: e.weight, min: e.min, max: e.max, conditions: Slice::new(&out.conds[first + 1 + ei]), smelt: Str::new(&e.smelt), ..abi::LootEntry::DEFAULT }
            })
            .collect();
        n += p.entries.len();
        out.entries.push(entries);
    }
    for (pi, p) in pools.iter().enumerate() {
        out.pools.push(abi::LootPool { rolls_min: p.rolls.0, rolls_max: p.rolls.1, conditions: Slice::new(&out.conds[cond_index[pi]]), entries: Slice::new(&out.entries[pi]), ..abi::LootPool::DEFAULT });
    }
    out._names = names;
    out
}

/// A loot table being registered; sent when dropped.
pub struct LootBuilder<'a> {
    reg: &'a mut Registry,
    name: String,
    pools: Vec<Pool>,
}

impl LootBuilder<'_> {
    pub fn pool(mut self, p: Pool) -> Self {
        self.pools.push(p);
        self
    }
}

impl Drop for LootBuilder<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let raw = raw_pools(self.reg.ns, &self.pools);
        let desc = abi::LootDesc { name: Str::new(&self.name), pools: Slice::new(&raw.pools), ..abi::LootDesc::DEFAULT };
        unsafe { (host().reg_loot)(self.reg.raw, &desc) }
    }
}

/// Changes to tables of any mod (`table` may end in `*`).
pub struct LootModBuilder<'a> {
    reg: &'a mut Registry,
    table: String,
}

impl LootModBuilder<'_> {
    fn send(&mut self, op: u32, pool: Option<&Pool>, from: &str, to: &str) {
        let pools: Vec<Pool> = pool.into_iter().cloned().collect();
        let raw = raw_pools(self.reg.ns, &pools);
        let (from, to) = (if from.is_empty() { String::new() } else { qualify(self.reg.ns, from) }, if to.is_empty() { String::new() } else { qualify(self.reg.ns, to) });
        let desc = abi::LootModDesc {
            table: Str::new(&self.table),
            op,
            pool: raw.pools.first().copied().unwrap_or(abi::LootPool::DEFAULT),
            from: Str::new(&from),
            to: Str::new(&to),
            ..abi::LootModDesc::DEFAULT
        };
        unsafe { (host().reg_loot_modifier)(self.reg.raw, &desc) }
    }
    pub fn add_pool(mut self, p: Pool) -> Self {
        self.send(abi::loot_op::ADD_POOL, Some(&p), "", "");
        self
    }
    pub fn replace(mut self, from: &str, to: &str) -> Self {
        self.send(abi::loot_op::REPLACE_ITEM, None, from, to);
        self
    }
    pub fn remove(mut self, item: &str) -> Self {
        self.send(abi::loot_op::REMOVE_ITEM, None, item, "");
        self
    }
    pub fn clear(mut self) -> Self {
        self.send(abi::loot_op::CLEAR, None, "", "");
        self
    }
}

/// Every entity of a region that has a component, with the values side by side.
pub struct SystemBatch<'a, T> {
    world: World,
    ids: &'a [u64],
    values: &'a mut [T],
}

impl<T> SystemBatch<'_, T> {
    pub fn world(&self) -> World {
        self.world
    }
    pub fn ids(&self) -> &[u64] {
        self.ids
    }
    pub fn values(&mut self) -> &mut [T] {
        self.values
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u64, &mut T)> {
        self.ids.iter().copied().zip(self.values.iter_mut())
    }
}

impl Registry {
    /// A system over a per-entity component (`Registry::entity_component`): once per tick and
    /// region with all values in one slice, no host call per entity.
    pub fn system<T: Value>(&mut self, comp: &crate::Component<T>, f: impl Fn(&mut SystemBatch<T>) + Send + Sync + 'static) {
        let cb = callback(move |a: &mut abi::SystemArgs| {
            if a.stride as usize != size_of::<T>() || a.count == 0 {
                return;
            }
            let n = a.count as usize;
            let mut b = SystemBatch { world: World::from_raw(a.world), ids: unsafe { std::slice::from_raw_parts(a.ids, n) }, values: unsafe { std::slice::from_raw_parts_mut(a.values as *mut T, n) } };
            f(&mut b);
        });
        unsafe { (host().reg_system)(self.raw, Str::new(comp.name()), Str::new(&T::schema()), cb) }
    }

    /// Handles every value of `event` once per tick (and region), in one call.
    pub fn on<T: Value>(&mut self, event: &Event<T>, f: impl Fn(&EventBatch<T>) + Send + Sync + 'static) {
        let cb = callback(move |a: &mut abi::EventArgs| {
            let stride = a.stride as usize;
            if stride != size_of::<T>() {
                return;
            }
            let values = (0..a.count as usize).map(|i| unsafe { std::ptr::read_unaligned(a.data.add(i * stride) as *const T) }).collect();
            f(&EventBatch { world: World::from_raw(a.world), values });
        });
        unsafe { (host().reg_event_handler)(self.raw, Str::new(event.name()), Str::new(&T::schema()), cb) }
    }

    /// Answers `question` (`question::*` or a mod's own) before it happens.
    pub fn intercept(&mut self, question: &Question, f: impl Fn(&Ask) -> Verdict + Send + Sync + 'static) {
        let cb = callback(move |a: &mut abi::InterceptArgs| {
            let ask = Ask { world: World::from_raw(a.world), question: unsafe { std::mem::transmute::<&str, &'static str>(a.question.as_str()) }, query: a.query };
            let v = f(&ask);
            unsafe {
                (*a.query).verdict = match v {
                    Verdict::Abstain => abi::verdict::ABSTAIN,
                    Verdict::Allow => abi::verdict::ALLOW,
                    Verdict::Deny => abi::verdict::DENY,
                }
            }
        });
        unsafe { (host().reg_interceptor)(self.raw, Str::new(question.name()), cb) }
    }

    /// A step on value `chain` (`chain::*` or a mod's own) at `stage` (`stage::*`).
    pub fn modify(&mut self, chain: &Chain, stage: u32, f: impl Fn(&mut Modify) + Send + Sync + 'static) {
        let cb = callback(move |a: &mut abi::ModifyArgs| {
            let mut m = Modify { world: World::from_raw(a.world), query: unsafe { *a.ctx }, base: a.base, value: a.value };
            f(&mut m);
            a.value = m.value;
        });
        unsafe { (host().reg_modifier)(self.raw, Str::new(chain.name()), stage, cb) }
    }

    /// An attribute of living entities (`attribute::*` are the engine's).
    pub fn attribute(&mut self, attribute: &Attribute, default: f64, min: f64, max: f64) {
        unsafe { (host().reg_attribute)(self.raw, Str::new(attribute.name()), default, min, max) }
    }

    /// A loot table: a block's or entity kind's is what it drops, a `LootTable` is one of its own.
    pub fn loot<'a>(&'a mut self, table: &dyn LootSource) -> LootBuilder<'a> {
        LootBuilder { reg: self, name: table.table_name().to_string(), pools: Vec::new() }
    }

    pub fn loot_modify<'a>(&'a mut self, table: &dyn LootSource) -> LootModBuilder<'a> {
        LootModBuilder { reg: self, table: table.table_name().to_string() }
    }

    /// Flags of a stack component (`abi::stack_flags`): kept on the server, not saved.
    pub fn stack_component<T: Value>(&mut self, c: &crate::Component<T>, flags: u32) {
        unsafe { (host().reg_stack_component)(self.raw, Str::new(c.name()), Str::new(&T::schema()), flags) }
    }
}


impl World {
    /// In `ticks` ticks `event` fires with `value` (a calm-down, a hunger pulse). The timer is
    /// saved with `entity` and goes with it when it is removed; one per entity and event, so
    /// setting it again moves it.
    pub fn after<T: Value>(&self, entity: u64, ticks: u32, event: &Event<T>, value: T) {
        let id = event.id();
        if id == u32::MAX {
            return;
        }
        let b = unsafe { std::slice::from_raw_parts(&value as *const T as *const u8, size_of::<T>()) };
        unsafe { (host().entity_after)(self.raw(), entity, ticks, id, Slice::new(b)) }
    }

    pub fn emit<T: Value>(&self, event: &Event<T>, value: T) {
        let id = event.id();
        if id == u32::MAX {
            return;
        }
        let b = unsafe { std::slice::from_raw_parts(&value as *const T as *const u8, size_of::<T>()) };
        unsafe { (host().emit)(self.raw(), id, Slice::new(b)) }
    }

    /// Asks the interceptors of `question`; true if allowed.
    pub fn ask(&self, question: &Question, q: &mut abi::Query) -> bool {
        unsafe { (host().ask)(self.raw(), Str::new(question.name()), q) != 0 }
    }

    pub fn compute(&self, chain: &Chain, base: f64, q: &abi::Query) -> f64 {
        unsafe { (host().compute)(self.raw(), Str::new(chain.name()), base, q, 0) }
    }

    /// Like `compute`, with the steps that led to the value ("4, base at stage 200: 4 -> 2 = 2").
    pub fn explain(&self, chain: &Chain, base: f64, q: &abi::Query) -> (f64, String) {
        let v = unsafe { (host().compute)(self.raw(), Str::new(chain.name()), base, q, 1) };
        (v, unsafe { (host().explanation)().as_str().to_string() })
    }

    pub fn attribute(&self, entity: u64, attribute: &Attribute) -> f64 {
        unsafe { (host().attribute)(self.raw(), entity, Str::new(attribute.name())) }
    }

    pub fn explain_attribute(&self, entity: u64, attribute: &Attribute) -> (f64, String) {
        let v = unsafe { (host().attribute_explain)(self.raw(), entity, Str::new(attribute.name())) };
        (v, unsafe { (host().explanation)().as_str().to_string() })
    }

    /// Adds or replaces modifier `id` on an attribute for `ticks` (0 = until removed).
    pub fn add_modifier(&self, entity: u64, attr: &Attribute, id: &str, op: u32, amount: f64, ticks: u64) {
        unsafe { (host().attribute_modifier)(self.raw(), entity, Str::new(attr.name()), Str::new(id), op, amount, ticks) }
    }

    pub fn remove_modifier(&self, entity: u64, attr: &Attribute, id: &str) {
        unsafe { (host().attribute_remove)(self.raw(), entity, Str::new(attr.name()), Str::new(id)) }
    }

    pub fn roll_loot(&self, table: &dyn LootSource, ctx: &LootCtx) -> Vec<ItemStack> {
        let table = table.table_name();
        let c = abi::LootCtx { tool: ctx.tool, killer: ctx.killer, by_player: ctx.by_player as u32, on_fire: ctx.on_fire as u32, ..abi::LootCtx::DEFAULT };
        let mut buf = vec![ItemStack::EMPTY; 16];
        let n = unsafe { (host().roll_loot)(self.raw(), Str::new(table), &c, buf.as_mut_ptr(), buf.len()) };
        if n > buf.len() {
            buf.resize(n, ItemStack::EMPTY);
            unsafe { (host().roll_loot)(self.raw(), Str::new(table), &c, buf.as_mut_ptr(), buf.len()) };
        }
        buf.truncate(n);
        buf
    }

    /// Breaks a block as if mined with `tool` by `player` (0 = nobody), with its loot when
    /// `drops`. False if nothing broke or an interceptor said no.
    pub fn break_block(&self, pos: [i32; 3], tool: ItemStack, player: u64, drops: bool) -> bool {
        unsafe { (host().break_block)(self.raw(), &pos, tool, player, drops as u32) != 0 }
    }
}

/// A query for `World::ask` and value chains.
pub fn query() -> abi::Query {
    abi::Query::DEFAULT
}
