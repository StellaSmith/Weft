//! Mobs: zombies and spiders at night or in the dark, pigs and cows on grass by day.

use voxide_sdk::*;

use crate::blocks::GRASS;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Brain {
    pub target: u64,
    pub timer: u32,
    pub wander: [f32; 2],
    pub cooldown: u32,
    pub panic: u32,
    /// Position last tick while walking (NaN when it stood still), to notice being stuck.
    pub last: [f32; 2],
}

pub static BRAIN: Component<Brain> = Component::new();
pub static ZOMBIE: Entity = Entity::new();
pub static SPIDER: Entity = Entity::new();
pub static PIG: Entity = Entity::new();
pub static COW: Entity = Entity::new();

struct Drop(&'static str, u32, u32);

pub fn register(r: &mut Registry) {
    r.component("brain", &BRAIN, Brain { last: [f32::NAN; 2], ..Default::default() }, 0, &["base:zombie", "base:spider", "base:pig", "base:cow"]);
    r.entity("zombie", &ZOMBIE)
        .size(0.6, 1.95)
        .health(20.0)
        .speed(2.6)
        .hostile()
        .flags(entity_flags::BURNS_IN_SUN)
        .texture("zombie", 64, 32)
        .model(biped(true))
        .on_tick(|t| hostile_ai(t, 3.0, false))
        .on_death(|e| drops(e, &[Drop("rotten_flesh", 0, 2)]));
    r.entity("spider", &SPIDER)
        .size(1.3, 0.9)
        .health(16.0)
        .speed(3.6)
        .hostile()
        .texture("spider", 64, 32)
        .model(spider())
        .on_tick(|t| hostile_ai(t, 2.0, true))
        .on_death(|e| drops(e, &[Drop("string", 0, 2)]));
    r.entity("pig", &PIG)
        .size(0.9, 0.9)
        .health(10.0)
        .speed(2.5)
        .passive()
        .texture("pig", 64, 64)
        .model(quadruped([10.0, 8.0, 16.0], 6.0, [8.0, 8.0, 8.0]))
        .on_tick(passive_ai)
        .on_hurt(panic)
        .on_death(|e| drops(e, &[Drop("porkchop", 1, 3)]));
    r.entity("cow", &COW)
        .size(0.9, 1.3)
        .health(10.0)
        .speed(2.2)
        .passive()
        .texture("cow", 64, 64)
        .model(quadruped([12.0, 10.0, 18.0], 12.0, [8.0, 8.0, 6.0]))
        .on_tick(passive_ai)
        .on_hurt(panic)
        .on_death(|e| drops(e, &[Drop("beef", 1, 3), Drop("leather", 0, 2)]));
    r.hook_tick("core:player", spawn_around_players);
}

fn drops(e: &mut EntityEvent, list: &[Drop]) {
    let w = e.world();
    let Some(st) = w.entity(e.entity()) else { return };
    for d in list {
        let n = d.1 + w.random_below(d.2 - d.1 + 1);
        if let (Some(item), true) = (find_item(&format!("base:{}", d.0)), n > 0) {
            w.spawn_item([st.pos[0], st.pos[1] + 0.5, st.pos[2]], ItemStack::new(item, n));
        }
    }
}

fn panic(e: &mut EntityEvent) {
    if let Some(b) = e.world().component(e.entity(), &BRAIN) {
        b.panic = 60;
        b.wander = [0.0, 0.0];
    }
}

fn dir_to(from: [f64; 3], to: [f64; 3]) -> ([f32; 2], f64) {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    let d = (dx * dx + dz * dz).sqrt().max(1e-6);
    ([(dx / d) as f32, (dz / d) as f32], d)
}

fn yaw_of(dir: [f32; 2]) -> f32 {
    (-dir[0]).atan2(-dir[1])
}

/// Chase the nearest player in range, hit when close, jump over steps.
fn hostile_ai(t: &EntityTick, damage: f32, only_dark: bool) {
    let w = t.world();
    let players = w.players();
    for &id in t.ids() {
        let Some(mut st) = w.entity(id) else { continue };
        let Some(b) = w.component(id, &BRAIN) else { continue };
        b.cooldown = b.cooldown.saturating_sub(1);
        b.timer = b.timer.saturating_sub(1);
        let feet = [st.pos[0].floor() as i32, (st.pos[1] + 0.5).floor() as i32, st.pos[2].floor() as i32];
        let aggressive = !only_dark || w.daylight() < 0.5 || w.sky_light(feet) < 10;
        let target = players
            .iter()
            .filter_map(|&p| w.entity(p).filter(|s| s.health > 0.0).map(|s| (p, s)))
            .map(|(p, s)| (p, s, dir_to(st.pos, s.pos)))
            .filter(|(_, s, (_, d))| *d < 18.0 && (s.pos[1] - st.pos[1]).abs() < 8.0)
            .min_by(|a, b| a.2.1.total_cmp(&b.2.1));
        let Some(b) = w.component(id, &BRAIN) else { continue };
        match target.filter(|_| aggressive) {
            Some((p, ps, (dir, dist))) => {
                let stuck = st.on_ground != 0 && (st.pos[0] as f32 - b.last[0]).abs() + (st.pos[2] as f32 - b.last[1]).abs() < 0.02;
                b.last = [st.pos[0] as f32, st.pos[2] as f32];
                if dist > 1.0 {
                    w.walk(id, dir, 1.0, stuck);
                } else {
                    b.last = [f32::NAN; 2];
                }
                st.yaw = yaw_of(dir);
                st.pitch = 0.0;
                w.set_entity(id, &st);
                let Some(b) = w.component(id, &BRAIN) else { continue };
                if dist < 1.6 && (ps.pos[1] - st.pos[1]).abs() < 1.8 && b.cooldown == 0 {
                    b.cooldown = 20;
                    w.damage(p, damage, id, damage::ATTACK);
                }
            }
            None => wander(&w, id, &mut st, 0.6),
        }
    }
}

fn passive_ai(t: &EntityTick) {
    let w = t.world();
    for &id in t.ids() {
        let Some(mut st) = w.entity(id) else { continue };
        let Some(b) = w.component(id, &BRAIN) else { continue };
        if b.panic > 0 {
            b.panic -= 1;
            if b.panic % 20 == 0 || b.wander == [0.0, 0.0] {
                let a = (w.random_below(628) as f32) / 100.0;
                b.wander = [a.cos(), a.sin()];
            }
            let dir = b.wander;
            w.walk(id, dir, 1.6, st.on_ground != 0 && w.random_below(8) == 0);
            st.yaw = yaw_of(dir);
            w.set_entity(id, &st);
            continue;
        }
        wander(&w, id, &mut st, 0.5);
    }
}

fn wander(w: &World, id: u64, st: &mut EntityState, speed: f32) {
    let Some(b) = w.component(id, &BRAIN) else { return };
    if b.timer == 0 {
        b.timer = 40 + w.random_below(80);
        b.wander = if w.random_below(3) == 0 {
            [0.0, 0.0]
        } else {
            let a = (w.random_below(628) as f32) / 100.0;
            [a.cos(), a.sin()]
        };
    }
    b.timer -= 1;
    let dir = b.wander;
    if dir == [0.0, 0.0] {
        b.last = [f32::NAN; 2];
    } else {
        let stuck = st.on_ground != 0 && (st.pos[0] as f32 - b.last[0]).abs() + (st.pos[2] as f32 - b.last[1]).abs() < 0.01;
        b.last = [st.pos[0] as f32, st.pos[2] as f32];
        w.walk(id, dir, speed, stuck);
        st.yaw = yaw_of(dir);
        w.set_entity(id, st);
    }
}

/// Every two seconds, try to spawn mobs in a ring around each player.
fn spawn_around_players(t: &EntityTick) {
    let w = t.world();
    if w.time() % 40 != 0 {
        return;
    }
    for &p in t.ids() {
        let Some(ps) = w.entity(p) else { continue };
        let near = w.near(ps.pos, 64.0, None);
        let count = |k: &Entity| near.iter().filter(|&&id| w.entity(id).is_some_and(|s| s.kind == k.id())).count();
        let hostiles = count(&ZOMBIE) + count(&SPIDER);
        let animals = count(&PIG) + count(&COW);
        for _ in 0..4 {
            let a = (w.random_below(628) as f64) / 100.0;
            let r = 20.0 + w.random_below(26) as f64;
            let x = (ps.pos[0] + a.cos() * r).floor() as i32;
            let z = (ps.pos[2] + a.sin() * r).floor() as i32;
            let Some(y) = ground(&w, x, ps.pos[1].floor() as i32, z) else { continue };
            let at = [x as f64 + 0.5, y as f64, z as f64 + 0.5];
            let light = w.sky_light([x, y, z]) as f32 * w.daylight();
            let dark = light.max(w.block_light([x, y, z]) as f32) < 7.0;
            if dark && hostiles < 8 {
                let kind = if w.random_below(3) == 0 { &SPIDER } else { &ZOMBIE };
                w.spawn(kind, at);
            } else if !dark && animals < 6 && w.random_below(20) == 0 && GRASS.is(w.get([x, y - 1, z])) {
                let kind = if w.random_below(2) == 0 { &PIG } else { &COW };
                for _ in 0..2 + w.random_below(2) {
                    w.spawn(kind, at);
                }
            }
        }
    }
}

/// First standable spot scanning down from above the player.
fn ground(w: &World, x: i32, y0: i32, z: i32) -> Option<i32> {
    use voxide_sdk::abi::block_flags as bf;
    for y in (y0 - 16..y0 + 16).rev() {
        let below = w.flags([x, y - 1, z]);
        if below & bf::SOLID != 0 && below & bf::LIQUID == 0 && w.flags([x, y, z]) & (bf::SOLID | bf::LIQUID) == 0 && w.flags([x, y + 1, z]) & bf::SOLID == 0 {
            return Some(y);
        }
    }
    None
}
