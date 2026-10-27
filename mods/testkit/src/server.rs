//! Server half: tests send operations as text on `testkit:op` and get an answer on
//! `testkit:reply`. Text keeps both sides trivial and the log readable; this is not a hot path.
//! Positions may be written relative to the player's block as `~dx`.

use std::sync::atomic::{AtomicU32, Ordering};

use weft_sdk::abi::entry_kind as ek;
use weft_sdk::*;

/// How many of each event arrived since `events reset`, in `EVENT_NAMES` order.
static COUNTS: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
const EVENT_NAMES: [&str; 8] = ["block_broken", "block_placed", "item_picked_up", "entity_died", "entity_hurt", "player_joined", "item_used", "smelted"];

use base_api::SMELTED;
static GOLD: Block = Block::named("base:gold_block");
/// Shared with everyone who sees the entity, for the sync test.
pub static MARK: Component<u32> = Component::named("testkit:mark");
/// Server-side stack data that is not saved either.
pub static SECRET: Component<u32> = Component::named("testkit:secret");
static PIG: Entity = Entity::named("base:pig");
pub static COUNT_MARKS: Rule = Rule::named("count_marks");
pub static PROTECT_GOLD: Rule = Rule::named("protect_gold");
pub static HALF_DAMAGE: Rule = Rule::named("half_damage");
/// Mobs may not attack while it is on.
pub static PEACE: Rule = Rule::named("peace");
/// Players can't be hurt (`question::HURT`) or use items (`question::USE_ITEM`) while on.
pub static NO_HURT: Rule = Rule::named("no_hurt");
pub static NO_USE: Rule = Rule::named("no_use");
/// Fired by `after` timers; counted.
static RANG: Event<u64> = Event::named("rang");
static RINGS: AtomicU32 = AtomicU32::new(0);
pub static OP: Channel = Channel::named("op");
pub static REPLY: Channel = Channel::named("reply");
/// A key whose presses reach the server: the test counts them there.
pub static PING: Key = Key::named("ping");
static PINGS: AtomicU32 = AtomicU32::new(0);

fn count(i: usize, n: usize) {
    COUNTS[i].fetch_add(n as u32, Ordering::Relaxed);
}

pub fn register(r: &mut Registry) {
    r.entity_component(&MARK, 0, component_flags::SYNC_WATCHERS);
    r.attach(&PIG, &MARK);
    r.rule(&COUNT_MARKS, 0);
    // While the rule is on, every marked pig counts up once a tick.
    r.system(&MARK, |b| {
        if b.world().rule(&COUNT_MARKS) == 0 {
            return;
        }
        for m in b.values().iter_mut().filter(|m| **m > 0) {
            *m += 1;
        }
    });
    r.stack_component(&SECRET, abi::stack_flags::SERVER_ONLY | abi::stack_flags::NO_SAVE);
    // Switches for the tests of the extension mechanisms, off unless a test turns them on.
    r.rule(&PROTECT_GOLD, 0);
    r.rule(&PEACE, 0);
    r.intercept(&question::ATTACK, |q| {
        let w = q.world();
        let mob = w.entity(q.entity()).is_some_and(|e| !w.each_player().any(|p| p == q.entity()) && e.health > 0.0);
        if mob && w.rule(&PEACE) != 0 { Verdict::Deny } else { Verdict::Abstain }
    });
    r.rule(&NO_HURT, 0);
    r.rule(&NO_USE, 0);
    let player = |w: &World, id: u64| w.each_player().any(|p| p == id);
    r.intercept(&question::HURT, move |q| if player(&q.world(), q.entity()) && q.world().rule(&NO_HURT) != 0 { Verdict::Deny } else { Verdict::Abstain });
    r.intercept(&question::USE_ITEM, |q| if q.world().rule(&NO_USE) != 0 { Verdict::Deny } else { Verdict::Abstain });
    r.on(&RANG, |b| {
        RINGS.fetch_add(b.len() as u32, Ordering::Relaxed);
    });
    r.rule(&HALF_DAMAGE, 0);
    r.intercept(&question::BREAK_BLOCK, |q| if GOLD.is(q.state()) && q.world().rule(&PROTECT_GOLD) != 0 { Verdict::Deny } else { Verdict::Abstain });
    r.modify(&chain::DAMAGE, stage::FINAL, |m| {
        if m.world().rule(&HALF_DAMAGE) != 0 {
            m.value *= 0.5;
        }
    });
    r.on(&BLOCK_BROKEN, |b| count(0, b.len()));
    r.on(&BLOCK_PLACED, |b| count(1, b.len()));
    r.on(&ITEM_PICKED_UP, |b| count(2, b.len()));
    r.on(&ENTITY_DIED, |b| count(3, b.len()));
    r.on(&ENTITY_HURT, |b| count(4, b.len()));
    r.on(&PLAYER_JOINED, |b| count(5, b.len()));
    r.on(&ITEM_USED, |b| count(6, b.len()));
    r.on(&SMELTED, |b| count(7, b.len()));
    r.key(&PING, "F10").on_press_server(|_| {
        PINGS.fetch_add(1, Ordering::Relaxed);
    });
    r.packet(&OP, |p| {
        let data = p.data();
        if data.len() < 4 {
            return;
        }
        let seq = &data[..4];
        let text = String::from_utf8_lossy(&data[4..]).to_string();
        let answer = match run(&p.world(), p.player(), &text) {
            Ok(s) => format!("ok {s}"),
            Err(e) => format!("err {e}"),
        };
        let mut out = seq.to_vec();
        out.extend_from_slice(answer.as_bytes());
        p.reply(&REPLY, &out);
    });
}

/// `stone`, `base:furnace[lit=true]`: base's when no namespace is given.
fn block_state(name: &str) -> Result<u32, String> {
    let n = match name {
        "air" => "core:air".to_string(),
        n if n.split('[').next().unwrap_or("").contains(':') => n.to_string(),
        n => format!("base:{n}"),
    };
    Block::lookup(&n).ok_or_else(|| format!("no block {n}"))
}

fn item(name: &str) -> Result<u32, String> {
    let n = if name.contains(':') { name.to_string() } else { format!("base:{name}") };
    Item::lookup(&n).ok_or_else(|| format!("no item {n}"))
}

fn stack_text(s: ItemStack) -> String {
    if s.is_empty() { "- 0 0".into() } else { format!("{} {} {}", item_name(s.item), s.count, s.damage()) }
}

struct Args<'a> {
    it: std::str::SplitWhitespace<'a>,
    me: [i32; 3],
}

impl<'a> Args<'a> {
    fn word(&mut self) -> Result<&'a str, String> {
        self.it.next().ok_or_else(|| "missing argument".to_string())
    }
    fn num<T: std::str::FromStr>(&mut self) -> Result<T, String> {
        let w = self.word()?;
        w.parse().map_err(|_| format!("not a number: {w}"))
    }
    fn coord(&mut self, axis: usize) -> Result<i32, String> {
        let w = self.word()?;
        match w.strip_prefix('~') {
            Some(d) => Ok(self.me[axis] + if d.is_empty() { 0 } else { d.parse::<i32>().map_err(|_| format!("bad offset {w}"))? }),
            None => w.parse().map_err(|_| format!("bad coordinate {w}")),
        }
    }
    fn pos(&mut self) -> Result<[i32; 3], String> {
        Ok([self.coord(0)?, self.coord(1)?, self.coord(2)?])
    }
    /// `me`, a block position (its block entity) or an entity id.
    fn owner(&mut self, w: &World, player: u64) -> Result<u64, String> {
        let first = self.word()?;
        if first == "me" {
            return Ok(player);
        }
        if let Some(rest) = first.strip_prefix('#') {
            return rest.parse().map_err(|_| format!("bad entity {first}"));
        }
        let mut again = Args { it: first.split_whitespace(), me: self.me };
        let x = again.coord(0)?;
        let p = [x, self.coord(1)?, self.coord(2)?];
        match w.block_entity(p) {
            0 => Err(format!("no block entity at {p:?} ({})", block_name(w.get(p)))),
            id => Ok(id),
        }
    }
}

fn run(w: &World, player: u64, text: &str) -> Result<String, String> {
    let me = w.entity(player).map(|e| [e.pos[0].floor() as i32, e.pos[1].floor() as i32, e.pos[2].floor() as i32]).unwrap_or_default();
    let mut words = text.split_whitespace();
    let op = words.next().unwrap_or("");
    let mut a = Args { it: words, me };
    match op {
        "set" => {
            let p = a.pos()?;
            let s = block_state(a.word()?)?;
            if !w.set(p, s) {
                return Err(format!("{p:?} is not loaded"));
            }
            Ok(String::new())
        }
        "fill" => {
            let (p, q) = (a.pos()?, a.pos()?);
            let s = block_state(a.word()?)?;
            let (lo, hi) = ([p[0].min(q[0]), p[1].min(q[1]), p[2].min(q[2])], [p[0].max(q[0]), p[1].max(q[1]), p[2].max(q[2])]);
            let volume = (0..3).map(|i| (hi[i] - lo[i] + 1) as i64).product::<i64>();
            if volume > 1 << 18 {
                return Err(format!("fill of {volume} blocks is too big"));
            }
            let mut missed = 0;
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    for x in lo[0]..=hi[0] {
                        missed += !w.set([x, y, z], s) as u32;
                    }
                }
            }
            if missed > 0 { Err(format!("{missed} blocks not loaded")) } else { Ok(volume.to_string()) }
        }
        "get" => Ok(block_name(w.get(a.pos()?)).to_string()),
        "group" => {
            let owner = a.owner(w, player)?;
            let g = w.inventory(owner).group(a.word()?);
            Ok(format!("{} {}", g.len(), (0..g.len()).filter(|&i| !g.get(i).is_empty()).count()))
        }
        "pings" => Ok(PINGS.load(Ordering::Relaxed).to_string()),
        "setprop" => {
            let p = a.pos()?;
            let (name, value) = (a.word()?, a.word()?);
            Ok((w.set_prop(p, name, value) as u32).to_string())
        }
        "sblock" => {
            // `sblock #id x y z [block]`: a block of a moving structure, or set it.
            let id = a.owner(w, player)?;
            let p = a.pos()?;
            match a.word() {
                Ok(b) => Ok((w.set_structure_block(id, p, block_state(b)?) as u32).to_string()),
                Err(_) => w.structure_block(id, p).map(|s| block_name(s).to_string()).ok_or_else(|| "not a structure".to_string()),
            }
        }
        "light" => Ok(w.light(a.pos()?).to_string()),
        "slot" => {
            let owner = a.owner(w, player)?;
            let slot = a.num()?;
            Ok(stack_text(w.inventory(owner).get(slot)))
        }
        "put" => {
            let owner = a.owner(w, player)?;
            let slot = a.num()?;
            let it = item(a.word()?)?;
            let count = a.num().unwrap_or(1);
            w.inventory(owner).set(slot, ItemStack::new(it, count));
            Ok(String::new())
        }
        "give" => {
            let it = item(a.word()?)?;
            let count: u32 = a.num().unwrap_or(1);
            let left = w.inventory(player).insert(ItemStack::new(it, count));
            Ok((count - left.count).to_string())
        }
        "insert" => {
            let p = a.pos()?;
            let owner = match w.block_entity(p) {
                0 => return Err(format!("no block entity at {p:?} ({})", block_name(w.get(p)))),
                id => id,
            };
            let it = item(a.word()?)?;
            let count: u32 = a.num().unwrap_or(1);
            let left = w.inventory(owner).insert(ItemStack::new(it, count));
            Ok((count - left.count).to_string())
        }
        "inv" => {
            let owner = a.owner(w, player).unwrap_or(player);
            let inv = w.inventory(owner);
            let parts: Vec<String> = (0..inv.size()).map(|i| (i, inv.get(i))).filter(|(_, s)| !s.is_empty()).map(|(i, s)| format!("{i}:{}:{}", item_name(s.item), s.count)).collect();
            Ok(parts.join(" "))
        }
        "spawn" => {
            let kind = a.word()?;
            let k = Entity::lookup(&if kind.contains(':') { kind.to_string() } else { format!("base:{kind}") }).ok_or_else(|| format!("no entity {kind}"))?;
            let p = a.pos()?;
            Ok(w.spawn_kind(k, [p[0] as f64 + 0.5, p[1] as f64, p[2] as f64 + 0.5]).to_string())
        }
        "entity" => {
            let id: u64 = match a.word()? {
                "me" => player,
                w => w.trim_start_matches('#').parse().map_err(|_| "bad id".to_string())?,
            };
            let e = w.entity(id).ok_or("no such entity")?;
            Ok(format!("{} {:.2} {:.2} {:.2} {}", name_of(ek::ENTITY, e.kind), e.pos[0], e.pos[1], e.pos[2], e.health))
        }
        "size" => {
            let id: u64 = match a.word()? {
                "me" => player,
                w => w.trim_start_matches('#').parse().map_err(|_| "bad id".to_string())?,
            };
            let s = w.entity(id).ok_or("no such entity")?.size;
            Ok(format!("{:.3} {:.3} {:.3}", s[0], s[1], s[2]))
        }
        "near" => {
            let p = a.pos()?;
            let r: f64 = a.num()?;
            let ids = w.near([p[0] as f64 + 0.5, p[1] as f64, p[2] as f64 + 0.5], r, None);
            let parts: Vec<String> = ids.iter().filter(|&&id| id != player).filter_map(|&id| w.entity(id).map(|e| format!("{id}:{}", name_of(ek::ENTITY, e.kind)))).collect();
            Ok(parts.join(" "))
        }
        "animate" => {
            let id = a.owner(w, player)?;
            w.play_animation(id, a.word().unwrap_or(""), false);
            Ok(String::new())
        }
        "survey" => {
            // Biomes of every block within `r` of the player, and where glow crystals are.
            let r: i32 = a.num()?;
            let me = w.entity(player).ok_or("no player")?;
            let c = [me.pos[0].floor() as i32, me.pos[1].floor() as i32, me.pos[2].floor() as i32];
            let crystal = Block::lookup("testmod:glow_crystal").ok_or("no crystal block")?;
            let cave = Tag::named("cave");
            let mut biomes: std::collections::BTreeMap<&str, u32> = Default::default();
            let (mut in_cave, mut elsewhere) = (0, 0);
            for y in c[1] - r..=c[1] + r {
                for z in c[2] - r..=c[2] + r {
                    for x in c[0] - r..=c[0] + r {
                        let b = w.biome([x, y, z]);
                        *biomes.entry(name_of(ek::BIOME, b)).or_default() += 1;
                        if block_of(w.get([x, y, z])) == block_of(crystal) {
                            if cave.on_biome(b) { in_cave += 1 } else { elsewhere += 1 }
                        }
                    }
                }
            }
            let list: Vec<String> = biomes.iter().map(|(n, k)| format!("{n}={k}")).collect();
            Ok(format!("{} | crystals {in_cave} {elsewhere}", list.join(" ")))
        }
        "drops" => {
            // `drops n`: n dropped stacks of a few kinds (blocks and sprites) in a grid north of
            // the player, 1.1 apart so they do not merge.
            let n: u32 = a.num()?;
            let kinds: Vec<u32> = ["base:dirt", "base:coal", "base:iron_pickaxe", "base:dandelion", "base:stick", "base:torch"].iter().filter_map(|k| Item::lookup(k)).collect();
            let side = (n as f64).sqrt().ceil() as u32;
            for i in 0..n {
                let (x, z) = ((i % side) as f64 * 1.1 - side as f64 * 0.55, -3.0 - (i / side) as f64 * 1.1);
                w.spawn_item([me[0] as f64 + x, me[1] as f64 + 0.5, me[2] as f64 + z], ItemStack::new(kinds[i as usize % kinds.len()], 1));
            }
            Ok(format!("{n} drops"))
        }
        "find" => {
            // `find r name`: the first block of that name within `r` of the player.
            let r: i32 = a.num()?;
            let id = block_of(Block::lookup(a.word()?).ok_or("no such block")?);
            for y in me[1] - r..=me[1] + r {
                for z in me[2] - r..=me[2] + r {
                    for x in me[0] - r..=me[0] + r {
                        if block_of(w.get([x, y, z])) == id {
                            return Ok(format!("{x} {y} {z}"));
                        }
                    }
                }
            }
            Err("not found".into())
        }
        "census" => {
            // `census r a,b`: how many of each block lie within `r` of the player.
            let r: i32 = a.num()?;
            let names: Vec<&str> = a.word()?.split(',').collect();
            let ids: Vec<u32> = names.iter().map(|n| Block::lookup(n).map(block_of).ok_or(format!("no block {n}"))).collect::<Result<_, _>>()?;
            let mut counts = vec![0u32; ids.len()];
            for y in me[1] - r..=me[1] + r {
                for z in me[2] - r..=me[2] + r {
                    for x in me[0] - r..=me[0] + r {
                        let b = block_of(w.get([x, y, z]));
                        if let Some(i) = ids.iter().position(|&id| id == b) {
                            counts[i] += 1;
                        }
                    }
                }
            }
            Ok(names.iter().zip(&counts).map(|(n, k)| format!("{n}={k}")).collect::<Vec<_>>().join(" "))
        }
        "onto" => {
            // `onto #id h [#rider]`: the player (or the rider) on top of an entity `h` high, in
            // the same tick (moving platforms move on while a teleport by command makes its way).
            let id = a.owner(w, player)?;
            let h: f64 = a.num()?;
            let rider = a.owner(w, player).unwrap_or(player);
            let e = w.entity(id).ok_or("no such entity")?;
            w.teleport(rider, None, [e.pos[0], e.pos[1] + h + 0.05, e.pos[2]]);
            Ok(String::new())
        }
        "navigate" => {
            let id = a.owner(w, player)?;
            let p = a.pos()?;
            Ok((w.navigate(id, [p[0] as f64 + 0.5, p[1] as f64, p[2] as f64 + 0.5], 1.0) as u32).to_string())
        }
        "structure" => {
            let name = a.word()?;
            let r: i32 = a.num()?;
            let me = w.entity(player).ok_or("no player")?;
            let p = [me.pos[0].floor() as i32, me.pos[1].floor() as i32, me.pos[2].floor() as i32];
            Ok(w.structure_near(name, p, r).map_or("none".into(), |a| format!("{} {} {}", a[0], a[1], a[2])))
        }
        // A timer on the player: `after 60`, then `rings` counts the ones that fired.
        "after" => {
            w.after(player, a.num()?, &RANG, player);
            Ok(String::new())
        }
        "rings" => Ok(RINGS.load(Ordering::Relaxed).to_string()),
        "heal" => {
            let mut st = w.entity(player).ok_or("no player")?;
            st.health = st.max_health;
            w.set_entity(player, &st);
            Ok(String::new())
        }
        "health" => {
            let id = a.owner(w, player).unwrap_or(player);
            Ok(w.entity(id).map_or("none".into(), |e| format!("{}", e.health)))
        }
        "time" => {
            w.set_time(a.num()?);
            Ok(String::new())
        }
        "mode" => {
            let m = match a.word()? {
                "survival" => 0,
                "creative" => 1,
                "spectator" => 2,
                other => return Err(format!("no game mode {other}")),
            };
            w.set_game_mode(player, m);
            Ok(String::new())
        }
        "menu" => {
            let name = a.word()?;
            let m = Menu::lookup(name).ok_or_else(|| format!("no menu {name}"))?;
            let p = a.pos()?;
            if !w.open_menu_kind_at(player, m, p) {
                return Err(format!("no block entity at {p:?} ({})", block_name(w.get(p))));
            }
            Ok(String::new())
        }
        "vmenu" => {
            let name = a.word()?;
            let m = Menu::lookup(name).ok_or_else(|| format!("no menu {name}"))?;
            let slots: u32 = a.num()?;
            let pos = w.entity(player).map_or([0.0; 3], |e| e.pos);
            let id = w.virtual_inventory(slots, pos);
            let inv = w.inventory(id);
            let mut i = 0;
            while let (Ok(n), Ok(c)) = (a.word(), a.num::<u32>()) {
                inv.set(i, ItemStack::new(item(n)?, c));
                i += 1;
            }
            w.open_menu_kind(player, m, id);
            Ok(id.to_string())
        }
        "remove" => {
            let id: u64 = a.word()?.trim_start_matches('#').parse().map_err(|_| "bad id".to_string())?;
            w.remove(id);
            Ok(String::new())
        }
        "mark" => {
            let id: u64 = a.word()?.trim_start_matches('#').parse().map_err(|_| "bad id".to_string())?;
            let v: u32 = a.num()?;
            *w.component_mut(id, &MARK).ok_or("no mark component")? = v;
            Ok(String::new())
        }
        "secret" => {
            let owner = a.owner(w, player)?;
            let slot: u32 = a.num()?;
            let inv = w.inventory(owner);
            let s = inv.get(slot);
            match a.num::<u32>() {
                Ok(v) => {
                    inv.set(slot, s.with(&SECRET, v));
                    Ok(String::new())
                }
                Err(_) => Ok(SECRET.stack_only(&s).map_or("none".into(), |v| v.to_string())),
            }
        }
        "events" => {
            if a.word() == Ok("reset") {
                COUNTS.iter().for_each(|c| c.store(0, Ordering::Relaxed));
            }
            Ok(EVENT_NAMES.iter().zip(&COUNTS).map(|(n, c)| format!("{n}={}", c.load(Ordering::Relaxed))).collect::<Vec<_>>().join(" "))
        }
        "attr" => {
            let owner = a.owner(w, player)?;
            let name = a.word()?;
            let (v, text) = w.explain_attribute(owner, Attribute::generated(name.to_string()));
            Ok(format!("{v} | {text}"))
        }
        "attr_mod" => {
            let owner = a.owner(w, player)?;
            let (name, id, op) = (a.word()?, a.word()?, a.num::<u32>()?);
            let (amount, ticks) = (a.num::<f64>()?, a.num::<u64>()?);
            w.add_modifier(owner, Attribute::generated(name.to_string()), id, op, amount, ticks);
            Ok(String::new())
        }
        "attr_rm" => {
            let owner = a.owner(w, player)?;
            let (name, id) = (a.word()?, a.word()?);
            w.remove_modifier(owner, Attribute::generated(name.to_string()), id);
            Ok(String::new())
        }
        "loot" => {
            let table = LootTable::generated(a.word()?.to_string());
            let n: u32 = a.num()?;
            let mut ctx = LootCtx::default();
            while let Ok(word) = a.word() {
                match word {
                    "fire" => ctx.on_fire = true,
                    "player" => ctx.by_player = true,
                    tool => ctx.tool = ItemStack::new(item(tool)?, 1),
                }
            }
            let mut totals: std::collections::BTreeMap<String, u32> = Default::default();
            for _ in 0..n {
                for s in w.roll_loot(table, &ctx) {
                    *totals.entry(item_name(s.item).to_string()).or_default() += s.count;
                }
            }
            Ok(totals.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "))
        }
        "break" => {
            let p = a.pos()?;
            let tool = match a.word() {
                Ok(t) => ItemStack::new(item(t)?, 1),
                Err(_) => ItemStack::EMPTY,
            };
            Ok((w.break_block(p, tool, player, true) as u32).to_string())
        }
        "damage" => {
            let amount: f64 = a.num()?;
            let cause: u32 = a.num().unwrap_or(abi::damage::GENERIC);
            let q = abi::Query { entity: player, cause, amount, ..query() };
            let (v, text) = w.explain(&chain::DAMAGE, amount, &q);
            Ok(format!("{v} | {text}"))
        }
        "snapshot" => {
            let (p, q) = (a.pos()?, a.pos()?);
            let mut out = Vec::new();
            for y in p[1].min(q[1])..=p[1].max(q[1]) {
                for z in p[2].min(q[2])..=p[2].max(q[2]) {
                    for x in p[0].min(q[0])..=p[0].max(q[0]) {
                        out.push(w.get([x, y, z]).to_string());
                    }
                }
            }
            Ok(out.join(" "))
        }
        _ => Err(format!("unknown op {op}")),
    }
}
