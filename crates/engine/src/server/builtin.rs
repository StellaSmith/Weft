//! The engine's own chat commands: name, usage (parsed and completed by `commands`) and handler
//! side by side, so a listed command always has one.

use glam::{DVec3, IVec3};
use weft_abi::{ItemStack, damage};

use super::Server;
use crate::commands::Args;
use crate::content::Content;
use crate::world::GameMode;

/// What a handler gets besides the server.
pub struct Ctx<'a> {
    pub c: &'static Content,
    /// Index of the player who typed it.
    pub pi: usize,
    pub a: &'a Args,
    /// Where the player stands.
    pub here: DVec3,
    /// The player's dimension and entity.
    pub me: (u32, u64),
}

impl Ctx<'_> {
    /// Entities a `targets` argument selects, the player without it.
    fn targets(&self, s: &Server, key: &str) -> Vec<(u32, u64)> {
        match self.a.get(key) {
            Some(crate::commands::Value::Targets(sel)) => s.select(self.pi, sel, self.here),
            _ => vec![self.me],
        }
    }
}

fn block_at(a: &Args) -> Option<IVec3> {
    a.pos("pos").map(|p| p.floor().as_ivec3())
}

type Handler = fn(&mut Server, &Ctx) -> String;

pub const COMMANDS: &[(&str, &str, Handler)] = &[
    ("help", "[command:word]", help),
    ("tp", "<pos:pos> | <to:targets> | <who:targets> <pos:pos> | <who:targets> <to:targets>", tp),
    ("dim", "<dim:dim> [who:targets]", dim),
    ("give", "<who:targets> <item:item> [count:int] | <item:item> [count:int]", give),
    ("time", "set <time:time> | add <ticks:int> | query", time),
    ("summon", "<entity:entity> [pos:pos] [count:int]", summon),
    ("kill", "[who:targets]", kill),
    ("setblock", "<pos:pos> <block:block>", set_block),
    ("insert", "<pos:pos> <item:item> [count:int]", insert),
    ("block", "<pos:pos>", block),
    ("gamemode", "survival [who:targets] | creative [who:targets] | spectator [who:targets]", game_mode),
    ("heal", "[who:targets]", heal),
    ("tick", "query | rate <tps:int> | freeze | unfreeze | step [ticks:int] | sprint <ticks:int>", tick),
    ("regions", "", regions),
    ("rule", "[name:word(#core:rules)] [value:int]", rule),
];

fn help(_s: &mut Server, x: &Ctx) -> String {
    let (c, a) = (x.c, x.a);
    match a.text("command") {
        Some(n) => c.commands.iter().find(|d| d.name == n).map_or(format!("unknown command /{n}"), |d| format!("/{} {}", d.name, d.usage)),
        None => c.commands.iter().map(|d| format!("/{} {}", d.name, d.usage)).collect::<Vec<_>>().join("\n"),
    }
}

fn tp(s: &mut Server, x: &Ctx) -> String {
    let (c, a, pi, here, me) = (x.c, x.a, x.pi, x.here, x.me);
    let who = x.targets(s, "who");
    let dest = match (a.pos("pos"), a.get("to")) {
        (Some(p), _) => Some((me.0, p)),
        (None, Some(crate::commands::Value::Targets(sel))) => {
            s.select(pi, sel, here).first().and_then(|&(d, id)| s.position_of(d, id).map(|p| (d, p)))
        }
        _ => None,
    };
    let Some((to_dim, pos)) = dest else { return "no destination".into() };
    for &(d, id) in &who {
        s.teleport(c, d, id, to_dim, pos);
    }
    format!("teleported {} to {:.1} {:.1} {:.1}", who.len(), pos.x, pos.y, pos.z)
}

fn dim(s: &mut Server, x: &Ctx) -> String {
    let (c, a) = (x.c, x.a);
    let d = a.id("dim").unwrap_or(0);
    let spawn = s.dims[d as usize].spawn_point(c);
    let who = x.targets(s, "who");
    for &(from, id) in &who {
        s.teleport(c, from, id, d, spawn);
    }
    format!("now in {}", a.text("dim").unwrap_or_default())
}

fn give(s: &mut Server, x: &Ctx) -> String {
    let (c, a) = (x.c, x.a);
    let count = a.int("count").unwrap_or(1).clamp(1, 1 << 20) as u32;
    let (item, iname) = (a.id("item").unwrap_or(0), a.text("item").unwrap_or_default().to_string());
    let mut given = 0;
    for (d, id) in x.targets(s, "who") {
        if let Some(r) = s.dims[d as usize].entity_region(id) {
            given += count - r.entities.insert_item(c, id, ItemStack::new(item, count)).count;
        }
    }
    format!("gave {given} x {iname}")
}

fn time(s: &mut Server, x: &Ctx) -> String {
    let (c, a, me) = (x.c, x.a, x.me);
    let d = &mut s.dims[me.0 as usize];
    let len = c.dims[d.id as usize].day_length.max(1) as u64;
    if a.get("query").is_some() {
        return format!("time is {} (day {}, tick {})", d.time % len, d.time / len, d.time);
    }
    let now = match (a.int("time"), a.int("ticks")) {
        (Some(t), _) => d.time - d.time % len + t.max(0) as u64,
        (None, Some(add)) => d.time.saturating_add_signed(add),
        _ => d.time,
    };
    d.set_time(now);
    format!("time set to {}", now % len)
}

fn summon(s: &mut Server, x: &Ctx) -> String {
    let (c, a, here, me) = (x.c, x.a, x.here, x.me);
    let count = a.int("count").unwrap_or(1).clamp(1, 100_000);
    let pos = a.pos("pos").unwrap_or(here);
    let (kind, kname) = (a.id("entity").unwrap_or(0), a.text("entity").unwrap_or_default().to_string());
    if c.game.is_block_entity(kind) {
        return format!("{kname} belongs to a block, place the block instead");
    }
    let r = s.dims[me.0 as usize].ensure_region(c, pos);
    for _ in 0..count {
        r.spawn(c, kind, pos);
    }
    format!("summoned {count} {kname}")
}

fn kill(s: &mut Server, x: &Ctx) -> String {
    let c = x.c;
    let who = x.targets(s, "who");
    for &(d, id) in &who {
        if let Some(r) = s.dims[d as usize].entity_region(id) {
            r.damage(c, id, f32::MAX, 0, damage::VOID);
        }
    }
    format!("killed {}", who.len())
}

fn set_block(s: &mut Server, x: &Ctx) -> String {
    let (c, a, dim) = (x.c, x.a, x.me.0);
    let (Some(p), Some(state)) = (block_at(a), a.id("block")) else { return String::new() };
    if !c.dims[dim as usize].editable {
        return format!("{} cannot be changed", c.dims[dim as usize].name);
    }
    match s.dims[dim as usize].region_at(p) {
        Some(r) => {
            r.set(c, p, state);
            r.flush_neighbors(c);
            r.finish_tick(c);
            format!("set {}", a.text("block").unwrap_or_default())
        }
        None => "not loaded".into(),
    }
}

fn insert(s: &mut Server, x: &Ctx) -> String {
    let (c, a, dim) = (x.c, x.a, x.me.0);
    let (Some(p), Some(item)) = (block_at(a), a.id("item")) else { return String::new() };
    let count = a.int("count").unwrap_or(1).clamp(1, 1 << 20) as u32;
    match s.dims[dim as usize].region_at(p) {
        Some(r) => match r.entities.anchors.get(&p).copied() {
            Some(owner) => {
                let left = r.entities.insert_item(c, owner, ItemStack::new(item, count));
                format!("inserted {} x {}", count - left.count, a.text("item").unwrap_or_default())
            }
            None => "no block entity there".into(),
        },
        None => "not loaded".into(),
    }
}

fn block(s: &mut Server, x: &Ctx) -> String {
    let (c, a, dim) = (x.c, x.a, x.me.0);
    let Some(p) = block_at(a) else { return String::new() };
    let d = &s.dims[dim as usize];
    format!("{} in {}", d.get(p).map_or("unloaded".into(), |s| c.state_name(s)), c.dims[d.id as usize].name)
}

fn game_mode(s: &mut Server, x: &Ctx) -> String {
    let a = x.a;
    let mode = GameMode::ALL.into_iter().find(|m| a.get(m.name()).is_some()).unwrap_or_default();
    let who: Vec<usize> = x.targets(s, "who").iter().filter_map(|t| s.player_index(t.1)).collect();
    for &p in &who {
        s.set_mode(p, mode);
    }
    format!("game mode: {} for {}", mode.name(), who.len())
}

fn heal(s: &mut Server, x: &Ctx) -> String {
    let c = x.c;
    for (d, id) in x.targets(s, "who") {
        if let Some(r) = s.dims[d as usize].entity_region(id) {
            let max = r.max_health(c, id);
            if let Some(e) = r.entities.get_mut(id) {
                e.health = max;
            }
        }
    }
    "healed".into()
}

/// Vanilla's `/tick`: the world's speed, frozen with steps, or a sprint through many ticks as fast
/// as they run (for tests that need hours of world time).
fn tick(s: &mut Server, x: &Ctx) -> String {
    let a = x.a;
    if let Some(tps) = a.int("tps") {
        s.tick_rate = tps.clamp(1, 1000) as u32;
        return format!("{} ticks per second", s.tick_rate);
    }
    if a.get("freeze").is_some() {
        s.frozen = true;
        return "the world is frozen".into();
    }
    if a.get("unfreeze").is_some() {
        (s.frozen, s.steps) = (false, 0);
        return "the world runs".into();
    }
    if a.get("step").is_some() {
        if !s.frozen {
            return "only a frozen world steps (/tick freeze)".into();
        }
        let n = a.int("ticks").unwrap_or(1).clamp(1, 1 << 20) as u32;
        s.steps += n;
        return format!("stepping {n} ticks");
    }
    if a.get("sprint").is_some() {
        let n = a.int("ticks").unwrap_or(1).clamp(1, 1 << 24) as u32;
        s.sprint = Some((n, n, std::time::Instant::now()));
        return format!("sprinting {n} ticks");
    }
    format!("{} ticks per second{}", s.tick_rate, if s.frozen { ", frozen" } else { "" })
}

fn regions(s: &mut Server, x: &Ctx) -> String {
    let c = x.c;
    let parts: Vec<String> = s
        .dims
        .iter()
        .filter(|d| !d.regions.is_empty())
        .map(|d| {
            let sizes: Vec<String> = d.regions.iter().map(|r| format!("{}/{}", r.cubes.len(), r.entities.list.len())).collect();
            format!("{}: {} ({})", c.dims[d.id as usize].name, d.regions.len(), sizes.join(" "))
        })
        .collect();
    format!("regions {}", parts.join(", "))
}

fn rule(s: &mut Server, x: &Ctx) -> String {
    let (c, a) = (x.c, x.a);
    let rules = &c.game.rules;
    let Some(name) = a.text("name") else {
        return rules.iter().zip(s.rules.iter()).map(|(r, v)| format!("{} = {v}", r.0)).collect::<Vec<_>>().join("\n");
    };
    let Some(i) = rules.iter().position(|r| r.0 == name || r.0.rsplit(':').next() == Some(name)) else { return format!("no rule {name}") };
    if let Some(v) = a.int("value") {
        let mut now = s.rules.to_vec();
        now[i] = v;
        s.rules = now.into();
    }
    format!("{} = {}", rules[i].0, s.rules[i])
}
