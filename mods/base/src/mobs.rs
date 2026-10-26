//! Mobs: zombies and spiders at night or in the dark, pigs and cows on grass by day.

use weft_sdk::ai::{self, Goal};
use weft_sdk::*;

value! {
    #[derive(Default)]
    pub struct Brain { pub cooldown: u32, pub panic: u32 }
}

pub static BRAIN: Component<Brain> = Component::named("base:brain");
pub static ZOMBIE: Entity = Entity::named("zombie");
pub static SPIDER: Entity = Entity::named("spider");
pub static PIG: Entity = Entity::named("pig");
pub static COW: Entity = Entity::named("cow");


pub fn register(r: &mut Registry) {
    r.entity_component(&BRAIN, Brain::default(), 0, &[]);
    r.entity(&ZOMBIE)
        .sounds("zombie")
        .size(0.6, 1.95)
        .health(20.0)
        .speed(2.6)
        .hostile()
        .flags(entity_flags::BURNS_IN_SUN)
        .texture("zombie", 64, 32)
        .model(biped(true))
        .attach_with(&BRAIN, Brain::default())
        .goals(hostile(3.0, false));
    r.entity(&SPIDER)
        .sounds("spider")
        .size(1.3, 0.9)
        .health(16.0)
        .speed(3.6)
        .hostile()
        .texture("spider", 64, 32)
        .model(spider())
        .attach_with(&BRAIN, Brain::default())
        .goals(hostile(2.0, true));
    r.entity(&PIG)
        .sounds("pig")
        .size(0.9, 0.9)
        .health(10.0)
        .speed(2.5)
        .passive()
        .texture("pig", 64, 64)
        .model(quadruped([10.0, 8.0, 16.0], 6.0, [8.0, 8.0, 8.0]))
        .attach_with(&BRAIN, Brain::default())
        .goals(passive())
        .on_hurt(panic);
    r.entity(&COW)
        .sounds("cow")
        .size(0.9, 1.3)
        .health(10.0)
        .speed(2.2)
        .passive()
        .texture("cow", 64, 64)
        .model(quadruped([12.0, 10.0, 18.0], 12.0, [8.0, 8.0, 6.0]))
        .attach_with(&BRAIN, Brain::default())
        .goals(passive())
        .on_hurt(panic);
    // Vanilla amounts; meat comes out cooked when the animal burns.
    r.loot(&ZOMBIE).pool(Pool::new().items("rotten_flesh", 0, 2));
    r.loot(&SPIDER).pool(Pool::new().items("string", 0, 2));
    r.loot(&PIG).pool(Pool::new().entry(Entry::item("porkchop").count(1, 3).smelted(&crate::recipes::SMELTING)));
    r.loot(&COW).pool(Pool::new().entry(Entry::item("beef").count(1, 3).smelted(&crate::recipes::SMELTING))).pool(Pool::new().items("leather", 0, 2));
    // Natural spawning, as rules the engine runs: monsters in the dark (spiders more in deep
    // caves), animals in small herds on grass by day, rarely once the world is loaded.
    r.spawn("zombie").light(0, 6).weight(2);
    r.spawn("spider").light(0, 6);
    r.spawn("spider").light(0, 6).biome("#cave").weight(4);
    for animal in ["pig", "cow"] {
        r.spawn(animal).biome("#animals").ground("#animal_spawn").light(7, 15).group(2, 3).chance(0.05);
    }
}


fn panic(e: &mut EntityEvent) {
    if let Some(b) = e.world().component_mut(e.entity(), &BRAIN) {
        b.panic = 60;
    }
}

/// Monsters chase the nearest player and hit it once a second when in reach; spiders only in
/// the dark (by day they leave players alone). Otherwise they wander.
fn hostile(damage: f32, only_dark: bool) -> ai::Goals {
    let angry = move |w: &World, st: &EntityState| {
        let feet = [st.pos[0].floor() as i32, (st.pos[1] + 0.5).floor() as i32, st.pos[2].floor() as i32];
        !only_dark || w.daylight() < 0.5 || w.sky_light(feet) < 10
    };
    ai::Goals::new()
        .then(move |w: &World, id: u64, st: &EntityState| {
            let Some(b) = w.component_mut(id, &BRAIN) else { return false };
            b.cooldown = b.cooldown.saturating_sub(1);
            if let Some((p, ps)) = ai::nearest_player(w, st, 1.6).filter(|_| angry(w, st)) {
                if (ps.pos[1] - st.pos[1]).abs() < 1.8 && b.cooldown == 0 {
                    b.cooldown = 20;
                    w.damage(p, damage, id, damage::ATTACK);
                }
            }
            // Hitting does not stop the chase.
            false
        })
        .then(move |w: &World, id: u64, st: &EntityState| angry(w, st) && ai::follow_player(18.0, 1.0).run(w, id, st))
        .then(ai::wander(0.6, 10.0))
}

/// Animals run about for three seconds after being hurt, otherwise wander.
fn passive() -> ai::Goals {
    ai::Goals::new()
        .then(|w: &World, id: u64, st: &EntityState| {
            let Some(b) = w.component_mut(id, &BRAIN) else { return false };
            if b.panic == 0 {
                return false;
            }
            b.panic -= 1;
            if b.panic % 20 == 19 || !w.navigating(id) {
                let a = w.random_below(628) as f64 / 100.0;
                w.navigate(id, [st.pos[0] + a.cos() * 8.0, st.pos[1], st.pos[2] + a.sin() * 8.0], 1.6);
            }
            true
        })
        .then(ai::wander(0.5, 10.0))
}
