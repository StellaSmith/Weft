//! Server half: tests send operations as text on `testkit:op` and get an answer on
//! `testkit:reply`. Text keeps both sides trivial and the log readable; this is not a hot path.
//! Positions may be written relative to the player's block as `~dx`.

use std::sync::atomic::{AtomicU32, Ordering};

use voxide_sdk::abi::entry_kind as ek;
use voxide_sdk::*;

/// How many of each event arrived since `events reset`, in `EVENT_NAMES` order.
static COUNTS: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
const EVENT_NAMES: [&str; 8] = ["block_broken", "block_placed", "item_picked_up", "entity_died", "entity_hurt", "player_joined", "eaten", "smelted"];

value! {
    struct Eaten { player: u64, stack: ItemStack, hunger: u32, saturation: f32 }
}
value! {
    struct Smelted { furnace: u64, input: u32, output: ItemStack }
}
static EATEN: Event<Eaten> = Event::new("eaten");
static SMELTED: Event<Smelted> = Event::new("smelted");
static GOLD: Block = Block::named("base:gold_block");
/// Shared with everyone who sees the entity, for the sync test.
pub static MARK: Component<u32> = Component::new("testkit:mark");
/// Server-side stack data that is not saved either.
pub static SECRET: Component<u32> = Component::new("testkit:secret");

fn count(i: usize, n: usize) {
    COUNTS[i].fetch_add(n as u32, Ordering::Relaxed);
}

pub fn register(r: &mut Registry) {
    r.entity_component(&MARK, 0, component_flags::SYNC_WATCHERS, &["base:pig"]);
    r.rule("count_marks", 0);
    // While the rule is on, every marked pig counts up once a tick.
    r.system(&MARK, |b| {
        if b.world().rule("testkit:count_marks") == 0 {
            return;
        }
        for m in b.values().iter_mut().filter(|m| **m > 0) {
            *m += 1;
        }
    });
    r.stack_component(&SECRET, abi::stack_flags::SERVER_ONLY | abi::stack_flags::NO_SAVE);
    // Switches for the tests of the extension mechanisms, off unless a test turns them on.
    r.rule("protect_gold", 0);
    r.rule("half_damage", 0);
    r.intercept(question::BREAK_BLOCK, |q| if GOLD.is(q.state()) && q.world().rule("testkit:protect_gold") != 0 { Verdict::Deny } else { Verdict::Abstain });
    r.modify(chain::DAMAGE, stage::FINAL, |m| {
        if m.world().rule("testkit:half_damage") != 0 {
            m.value *= 0.5;
        }
    });
    r.on(&BLOCK_BROKEN, |b| count(0, b.len()));
    r.on(&BLOCK_PLACED, |b| count(1, b.len()));
    r.on(&ITEM_PICKED_UP, |b| count(2, b.len()));
    r.on(&ENTITY_DIED, |b| count(3, b.len()));
    r.on(&ENTITY_HURT, |b| count(4, b.len()));
    r.on(&PLAYER_JOINED, |b| count(5, b.len()));
    r.on(&EATEN, |b| count(6, b.len()));
    r.on(&SMELTED, |b| count(7, b.len()));
    r.packet("op", |p| {
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
        p.reply("reply", &out);
    });
}

fn block_state(name: &str) -> Result<u32, String> {
    let (n, i) = name.split_once('#').map_or((name, 0), |(n, i)| (n, i.parse().unwrap_or(0)));
    let n = match n {
        "air" => "core:air".to_string(),
        n if n.contains(':') => n.to_string(),
        n => format!("base:{n}"),
    };
    find_block(&n).map(|s| s + i).ok_or_else(|| format!("no block {n}"))
}

fn item(name: &str) -> Result<u32, String> {
    let n = if name.contains(':') { name.to_string() } else { format!("base:{name}") };
    find_item(&n).ok_or_else(|| format!("no item {n}"))
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
            let k = find_entity(&if kind.contains(':') { kind.to_string() } else { format!("base:{kind}") }).ok_or_else(|| format!("no entity {kind}"))?;
            let p = a.pos()?;
            Ok(w.spawn_kind(k, [p[0] as f64 + 0.5, p[1] as f64, p[2] as f64 + 0.5]).to_string())
        }
        "entity" => {
            let id: u64 = a.word()?.trim_start_matches('#').parse().map_err(|_| "bad id".to_string())?;
            let e = w.entity(id).ok_or("no such entity")?;
            Ok(format!("{} {:.2} {:.2} {:.2} {}", name_of(ek::ENTITY, e.kind), e.pos[0], e.pos[1], e.pos[2], e.health))
        }
        "near" => {
            let p = a.pos()?;
            let r: f64 = a.num()?;
            let ids = w.near([p[0] as f64 + 0.5, p[1] as f64, p[2] as f64 + 0.5], r, None);
            let parts: Vec<String> = ids.iter().filter(|&&id| id != player).filter_map(|&id| w.entity(id).map(|e| format!("{id}:{}", name_of(ek::ENTITY, e.kind)))).collect();
            Ok(parts.join(" "))
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
            let m = find_menu(name).ok_or_else(|| format!("no menu {name}"))?;
            let p = a.pos()?;
            if !w.open_menu_kind_at(player, m, p) {
                return Err(format!("no block entity at {p:?} ({})", block_name(w.get(p))));
            }
            Ok(String::new())
        }
        "vmenu" => {
            let name = a.word()?;
            let m = find_menu(name).ok_or_else(|| format!("no menu {name}"))?;
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
            let (v, text) = w.explain_attribute(owner, name);
            Ok(format!("{v} | {text}"))
        }
        "attr_mod" => {
            let owner = a.owner(w, player)?;
            let (name, id, op) = (a.word()?, a.word()?, a.num::<u32>()?);
            let (amount, ticks) = (a.num::<f64>()?, a.num::<u64>()?);
            w.add_modifier(owner, name, id, op, amount, ticks);
            Ok(String::new())
        }
        "attr_rm" => {
            let owner = a.owner(w, player)?;
            let (name, id) = (a.word()?, a.word()?);
            w.remove_modifier(owner, name, id);
            Ok(String::new())
        }
        "loot" => {
            let table = a.word()?.to_string();
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
                for s in w.roll_loot(&table, &ctx) {
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
            let (v, text) = w.explain(chain::DAMAGE, amount, &q);
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
