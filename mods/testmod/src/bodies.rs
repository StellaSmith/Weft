//! Moving collision demos: a lift going up and down, a skiff flying back and forth fast enough
//! that jumping on it only works because riders move with it, and a crate that falls and can
//! be stood on.

use voxide_sdk::*;

/// Deck tiles of `n` x `n` blocks, half a block thick.
fn deck(n: i32) -> Vec<Part> {
    let mut p = Part::new([0.0; 3], abi::part_role::STATIC);
    let half = n as f32 * 8.0;
    for x in 0..n {
        for z in 0..n {
            p = p.cube([x as f32 * 16.0 - half, 0.0, z as f32 * 16.0 - half], [16.0, 8.0, 16.0], [0.0, 0.0]);
        }
    }
    vec![p]
}

pub fn register(r: &mut Registry) {
    // 4 blocks up in 4 seconds, then down again.
    r.entity("lift").size(2.0, 0.5).health(100.0).solid().kinematic().texture("bodies", 64, 64).model(deck(2)).on_tick(|t| {
        let w = t.world();
        for &id in t.ids() {
            let Some(mut st) = w.entity(id) else { continue };
            st.vel = [0.0, if (st.age / 80) % 2 == 0 { 1.0 } else { -1.0 }, 0.0];
            w.set_entity(id, &st);
        }
    });
    // 6 blocks a second along x for 3 seconds each way; riders stay with it up to 3 blocks up.
    r.entity("skiff").size(3.0, 0.5).health(100.0).solid().kinematic().carry_height(3.0).texture("bodies", 64, 64).model(deck(3)).on_tick(|t| {
        let w = t.world();
        for &id in t.ids() {
            let Some(mut st) = w.entity(id) else { continue };
            st.vel = [if (st.age / 60) % 2 == 0 { 6.0 } else { -6.0 }, 0.0, 0.0];
            w.set_entity(id, &st);
        }
    });
    r.entity("crate")
        .size(1.0, 1.0)
        .health(10.0)
        .solid()
        .texture("bodies", 64, 64)
        .model(vec![Part::new([0.0; 3], abi::part_role::STATIC).cube([-8.0, 0.0, -8.0], [16.0, 16.0, 16.0], [0.0, 24.0])]);
}
