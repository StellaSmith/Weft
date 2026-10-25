//! Goal-driven mob behavior: goals in priority order, and each tick every entity runs the first
//! goal that wants to act. Movement goes through the engine's navigation (`World::navigate`),
//! so mobs walk around walls instead of into them.
//!
//! ```ignore
//! r.entity("zombie").goals(Goals::new().then(attack).then(ai::follow_player(18.0, 1.0)).then(ai::wander(0.6, 10.0)));
//! ```

use crate::{EntityState, EntityTick, World};

/// One behavior: acts this tick and returns true, or returns false to let a lower one act.
pub trait Goal: Send + Sync + 'static {
    fn run(&self, w: &World, id: u64, st: &EntityState) -> bool;
}

impl<F: Fn(&World, u64, &EntityState) -> bool + Send + Sync + 'static> Goal for F {
    fn run(&self, w: &World, id: u64, st: &EntityState) -> bool {
        self(w, id, st)
    }
}

#[derive(Default)]
pub struct Goals(Vec<Box<dyn Goal>>);

impl Goals {
    pub fn new() -> Goals {
        Goals(Vec::new())
    }
    /// Adds a goal below the ones added before.
    pub fn then(mut self, g: impl Goal) -> Goals {
        self.0.push(Box::new(g));
        self
    }
    /// Runs the goals for every entity of the tick.
    pub fn tick(&self, t: &EntityTick) {
        let w = t.world();
        for &id in t.ids() {
            let Some(st) = w.entity(id) else { continue };
            for g in &self.0 {
                if g.run(&w, id, &st) {
                    break;
                }
            }
        }
    }
}

/// Now and then walks somewhere within `radius` blocks at `speed`; idle in between.
pub fn wander(speed: f32, radius: f64) -> impl Goal {
    move |w: &World, id: u64, st: &EntityState| {
        if w.navigating(id) {
            return true;
        }
        if w.random_below(100) != 0 {
            return false;
        }
        let a = w.random_below(628) as f64 / 100.0;
        let r = radius * (0.3 + w.random_below(70) as f64 / 100.0);
        w.navigate(id, [st.pos[0] + a.cos() * r, st.pos[1], st.pos[2] + a.sin() * r], speed);
        true
    }
}

/// The nearest living player within `range` times its `visibility` attribute (0 in creative)
/// and 8 blocks up or down: its id and state.
pub fn nearest_player(w: &World, st: &EntityState, range: f64) -> Option<(u64, EntityState)> {
    let d2 = |s: &EntityState| (s.pos[0] - st.pos[0]).powi(2) + (s.pos[2] - st.pos[2]).powi(2);
    w.each_player()
        .filter_map(|p| w.entity(p).map(|s| (p, s)))
        .filter(|(p, s)| s.health > 0.0 && (s.pos[1] - st.pos[1]).abs() < 8.0 && d2(s) < (range * w.attribute(*p, crate::abi::attribute::VISIBILITY)).powi(2))
        .min_by(|a, b| d2(&a.1).total_cmp(&d2(&b.1)))
}

/// Walks to the nearest player within `range` at `speed`.
pub fn follow_player(range: f64, speed: f32) -> impl Goal {
    move |w: &World, id: u64, st: &EntityState| match nearest_player(w, st, range) {
        Some((_, p)) => {
            w.navigate(id, p.pos, speed);
            true
        }
        None => false,
    }
}
