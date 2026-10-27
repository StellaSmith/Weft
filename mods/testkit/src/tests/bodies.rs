//! Stage 8: moving collisions with the test mod's lift, skiff and crate.

use super::add;
use crate::runner::{R, T};

async fn spawn(t: &T, kind: &str, p: [i32; 3]) -> Result<u64, String> {
    let id = t.op(&format!("spawn {kind} {} {} {}", p[0], p[1], p[2])).await?;
    id.trim().parse().map_err(|_| format!("spawn {kind}: {id}"))
}

/// Server position of an entity (`me`: the player as the server last heard).
async fn pos_of(t: &T, id: &str) -> Result<[f64; 3], String> {
    let line = t.op(&format!("entity {id}")).await?;
    let v: Vec<f64> = line.split_whitespace().skip(1).take(3).filter_map(|s| s.parse().ok()).collect();
    v.try_into().map_err(|_| format!("entity #{id}: {line}"))
}

/// Standing on a lift carries the player up and back down with it, without fall damage.
pub async fn lift(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let id = spawn(&t, "testmod:lift", add(f, [3, 0, 0])).await?;
    // Its box comes from its blocks (2 x 2 planks from block (0, 0, 0) on), no size of its own.
    t.check_eq(t.op(&format!("size {id}")).await?, "3.000 1.000 0.850".to_string(), "the lift's box from its blocks");
    let p = pos_of(&t, &format!("{id}")).await?;
    t.op(&format!("onto {id} 1.0")).await?;
    t.secs(2.0).await?;
    let (me, lift) = (t.state().pos, pos_of(&t, &format!("{id}")).await?);
    t.check(lift[1] > p[1] + 1.2, "the lift went up");
    t.check((me[1] - (lift[1] + 1.0)).abs() < 0.15, format!("the player rides on top (feet {:.2}, deck {:.2})", me[1], lift[1] + 1.0));
    // Up to the top, then all the way down.
    t.secs(5.0).await?;
    let (me, lift) = (t.state().pos, pos_of(&t, &format!("{id}")).await?);
    t.check(lift[1] < p[1] + 1.5, "the lift came down");
    t.check((me[1] - (lift[1] + 1.0)).abs() < 0.15, format!("the player rode it down (feet {:.2}, deck {:.2})", me[1], lift[1] + 1.0));
    t.check_eq(t.op("health").await?, "20".to_string(), "health after the ride");
    Ok(())
}

/// Motion in the frame of what one stands on: the camera glides with a lift (no 0.05 block
/// jump a tick), and an idol riding the skiff stands still on its deck, not walking in place.
pub async fn ride_frames(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let id = spawn(&t, "testmod:lift", add(f, [3, 0, 0])).await?;
    t.op(&format!("onto {id} 1.0")).await?;
    t.secs(0.5).await?;
    let mut steps = Vec::new();
    let mut last = t.state().shown[1];
    for _ in 0..60 {
        t.frames(1).await?;
        let y = t.state().shown[1];
        steps.push(y - last);
        last = y;
    }
    let still = steps.iter().filter(|d| d.abs() < 1e-5).count();
    let big = steps.iter().fold(0.0f64, |a, d| a.max(d.abs()));
    t.note(format!("{still} still frames of 60 while climbing, largest step {big:.4}"));
    t.check(still < 6, "the camera glides with the lift");
    t.check(big < 0.04, "no tick-sized jumps");

    t.cmd(&format!("/tp {} {} {}", f[0], f[1], f[2] + 10)).await?;
    let skiff = spawn(&t, "testmod:skiff", add(f, [0, 3, -6])).await?;
    let idol = spawn(&t, "testmod:idol", add(f, [0, 0, -6])).await?;
    t.op(&format!("onto {skiff} 1.0 {idol}")).await?;
    t.secs(1.0).await?;
    let (s, i) = (pos_of(&t, &format!("{skiff}")).await?, pos_of(&t, &format!("{idol}")).await?);
    t.check((i[0] - s[0]).abs() < 1.6 && (i[1] - (s[1] + 1.0)).abs() < 0.1, format!("the idol rides the skiff (idol {i:?}, skiff {s:?})"));
    let swing = t.c().entity(idol).map_or(1.0, |e| e.moved);
    t.check(swing < 0.15, format!("the riding idol does not walk ({swing:.2})"));
    Ok(())
}

/// A skiff and its rider glide on the client: drawn every frame at the speed the server moves
/// them (6 blocks a second), with no frames standing still and none jumping a tick ahead.
pub async fn skiff_smooth(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let skiff = spawn(&t, "testmod:skiff", add(f, [-8, 3, -6])).await?;
    let idol = spawn(&t, "testmod:idol", add(f, [-8, 0, -6])).await?;
    t.op(&format!("onto {skiff} 1.0 {idol}")).await?;
    // Into a leg, away from its turn (it turns every 3 seconds of its age).
    t.secs(0.5).await?;
    let x = |t: &T, id| t.c().entity(id).map_or(0.0, |e| e.pos[0]);
    // For 3.5 seconds (a turn is in it): it reverses once (overshooting and coming back would
    // reverse three times, jitter many times), never jumps, and its rider stays on its spot.
    let (mut turns, mut dir, mut jump, mut slip) = (0, 0.0f64, 0.0f64, 0.0f64);
    let (mut last, mut rel) = (x(&t, skiff), x(&t, idol) - x(&t, skiff));
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < 3.5 {
        t.frames(1).await?;
        let (now, now_rel) = (x(&t, skiff), x(&t, idol) - x(&t, skiff));
        let d = now - last;
        if d.abs() > 1e-6 {
            if dir != 0.0 && d.signum() != dir {
                turns += 1;
            }
            dir = d.signum();
        }
        (jump, slip) = (jump.max(d.abs()), slip.max((now_rel - rel).abs()));
        (last, rel) = (now, now_rel);
    }
    t.note(format!("{turns} reversals in 3.5 s, largest step {jump:.3}, rider moves on the deck up to {slip:.4} a frame"));
    t.check_eq(turns, 1, "the skiff turns once, without overshooting or jitter");
    t.check(jump < 0.15, "the skiff glides (no jumps)");
    t.check(slip < 0.02, "its rider stays on its spot of the deck");
    // Riding it too: the deck keeps its place in front of the camera.
    t.op(&format!("onto {skiff} 1.0")).await?;
    t.secs(0.5).await?;
    // For 3.5 seconds, so a turn is in it.
    let mut steps = Vec::new();
    let mut last = x(&t, skiff) - t.state().shown[0];
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < 3.5 {
        t.frames(1).await?;
        let now = x(&t, skiff) - t.state().shown[0];
        steps.push((now - last).abs());
        last = now;
    }
    steps.sort_by(|a, b| a.total_cmp(b));
    let big = steps.last().copied().unwrap_or(0.0);
    t.note(format!("skiff seen from its rider: steps up to {big:.4}"));
    t.check(big < 0.01, "the skiff stays put in front of a rider, turns included");
    Ok(())
}

/// Jumping on a skiff flying 6 blocks a second lands on it again: riders move with it up to its
/// carry height.
pub async fn skiff(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let id = spawn(&t, "testmod:skiff", add(f, [0, 3, 0])).await?;
    let p = pos_of(&t, &format!("{id}")).await?;
    t.op(&format!("onto {id} 1.0")).await?;
    t.secs(0.5).await?;
    // Jumping through its turn (it flies back after 3 seconds), then landing.
    t.hold("core:jump", 2.5);
    t.secs(3.5).await?;
    // Both as the server has them: the player's position arrives every tick from its client.
    let (me, skiff) = (pos_of(&t, "me").await?, pos_of(&t, &format!("{id}")).await?);
    t.check((skiff[0] - p[0]).abs() > 5.0, "the skiff flew on");
    // The skiff moves on while the teleport arrives, so the player may stand anywhere on the
    // deck (1.5 + 0.3 from its middle); the server also hears the player a tick late (0.3).
    t.check((me[0] - skiff[0]).abs() < 2.1 && (me[2] - skiff[2]).abs() < 2.1, format!("the player is still over it (x {:.2}, skiff {:.2})", me[0], skiff[0]));
    t.check((me[1] - (skiff[1] + 1.0)).abs() < 0.15, format!("and back on its deck (feet {:.2}, deck {:.2})", me[1], skiff[1] + 1.0));
    Ok(())
}

/// Natural spawning runs from rules: at night monsters appear around the player (in the
/// overworld; the flatlands stay at noon), and the rule switch stops it.
pub async fn natural_spawning(t: T) -> R {
    t.world(5).await?;
    t.idle().await?;
    t.op("time 18000").await?;
    t.cmd("/rule core:mob_spawning 1").await?;
    t.secs(8.0).await?;
    let near = t.op("near ~ ~ ~ 64").await?;
    let monsters = near.split_whitespace().filter(|e| e.ends_with(":base:zombie") || e.ends_with(":base:spider")).count();
    t.check(monsters > 0, format!("monsters spawned at night ({near})"));
    t.check(!near.contains("base:pig") && !near.contains("base:cow"), "no animals at night or outside `animals` biomes");
    t.cmd("/rule core:mob_spawning 0").await?;
    Ok(())
}

/// A mod changes one entity's box and look while it stays the same entity: a player morphed
/// into a pig is 0.9 high, walks through a gap 1 block high and sees from 0.77; crouching still
/// lowers whatever box it has; morphing back restores the player's.
pub async fn morph(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    t.cmd("/morph base:pig 0.9 0.9").await?;
    t.until("small box", std::time::Duration::from_secs(2), |t| t.c().entity(0).is_some()).await?;
    t.secs(0.3).await?;
    // A tunnel one block high: a pig fits.
    t.fill(add(f, [-1, 1, -2]), add(f, [1, 1, -6]), "stone").await?;
    t.fill(add(f, [-1, 0, -2]), add(f, [-1, 0, -6]), "stone").await?;
    t.fill(add(f, [1, 0, -2]), add(f, [1, 0, -6]), "stone").await?;
    t.c().look(0.0, 0.0);
    t.hold("core:forward", 0.9);
    t.secs(1.0).await?;
    let z = t.state().pos[2];
    t.check(z < f[2] as f64 - 2.5, format!("the morphed player went into the low tunnel (z {z:.2})"));
    t.shot("pig").await?;
    t.cmd("/morph").await?;
    t.cmd(&format!("/tp {} {} {}", f[0], f[1], f[2] + 3)).await?;
    t.secs(0.3).await?;
    t.hold("core:forward", 1.0);
    t.secs(1.1).await?;
    let z = t.state().pos[2];
    t.check(z > f[2] as f64 - 1.6, format!("as a player again it does not fit (z {z:.2})"));
    Ok(())
}

/// A zombie's goals: it finds its way around a wall to the player and hits it.
pub async fn zombie_chases(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    t.fill(add(f, [4, 0, -6]), add(f, [4, 2, 6]), "cobblestone").await?;
    t.fill(add(f, [4, 0, 5]), add(f, [4, 1, 5]), "air").await?;
    let id = spawn(&t, "zombie", add(f, [8, 0, 0])).await?;
    t.secs(10.0).await?;
    let z = pos_of(&t, &format!("{id}")).await?;
    let health: f32 = t.op("health").await?.parse().unwrap_or(20.0);
    t.check(z[0] < f[0] as f64 + 4.0, format!("the zombie came round the wall (x {:.1})", z[0]));
    t.check(health < 20.0, format!("and hit the player (health {health})"));
    // Mobs hit through the same question as players: a mod that forbids attacks stops them.
    t.op("heal").await?;
    t.cmd("/rule testkit:peace 1").await?;
    t.secs(2.5).await?;
    let calm: f32 = t.op("health").await?.parse().unwrap_or(0.0);
    t.cmd("/rule testkit:peace 0").await?;
    t.check(calm >= 20.0, format!("a zombie hit while attacks were forbidden (health {calm})"));
    Ok(())
}

/// The skiff is a structure of blocks: a player riding it places a block on its deck and
/// breaks it again while it flies; the block flies with it, collides, and mods read and write
/// its blocks.
pub async fn skiff_blocks(t: T) -> R {
    t.flat().await?;
    t.op("mode creative").await?;
    t.op("put me 0 stone 64").await?;
    t.c().select(0);
    let f = t.feet();
    let id = spawn(&t, "testmod:skiff", add(f, [0, 3, 0])).await?;
    let sb = |p: [i32; 3]| format!("sblock {id} {} {} {}", p[0], p[1], p[2]);
    t.check_eq(t.op(&sb([1, 0, 1])).await?, "base:oak_planks".to_string(), "it is built of planks");
    t.op(&format!("onto {id} 1.0")).await?;
    t.secs(0.4).await?;
    // Down at the deck ahead: the block goes on top of it.
    t.c().look(0.0, -60.0);
    t.secs(0.2).await?;
    t.press("core:use");
    t.secs(0.5).await?;
    t.check_eq(t.op(&sb([0, 1, -1])).await?, "base:stone".to_string(), "a stone placed on the flying deck");
    // Looking at a spot of the deck 0.02 from the edge between two planks (along the way it
    // flies) while riding, the same block stays targeted.
    let st = t.state();
    let o = t.c().entity(id).map_or([0.0; 3], |e| e.pos);
    let spot = [o[0] - 0.5 + 0.98, o[1] + 1.0, o[2] - 0.5 + 0.5 - 1.0];
    let eye = [st.shown[0], st.shown[1] + t.c().entity(0).map_or(1.62, |e| e.size[2] as f64), st.shown[2]];
    let (dx, dy, dz) = (spot[0] - eye[0], spot[1] - eye[1], spot[2] - eye[2]);
    t.c().look((-dx).atan2(-dz).to_degrees() as f32, dy.atan2((dx * dx + dz * dz).sqrt()).to_degrees() as f32);
    t.frames(2).await?;
    let mut targets = std::collections::BTreeSet::new();
    for _ in 0..60 {
        t.frames(1).await?;
        let s = t.state();
        targets.insert((s.target, s.target_frame));
    }
    t.check(targets.len() == 1, format!("one block looked at while riding ({targets:?})"));
    let (a, sa) = (pos_of(&t, &format!("{id}")).await?, t.op(&sb([0, 1, -1])).await?);
    t.secs(1.0).await?;
    let b = pos_of(&t, &format!("{id}")).await?;
    t.check((b[0] - a[0]).abs() > 2.0 && sa == t.op(&sb([0, 1, -1])).await?, "it flies on with the stone on it");
    // A mod's write, then the player breaks the stone.
    t.check_eq(t.op(&format!("{} base:cobblestone", sb([1, 1, 1]))).await?, "1".to_string(), "a mod sets a block of it");
    t.check_eq(t.op(&sb([1, 1, 1])).await?, "base:cobblestone".to_string(), "and reads it back");
    t.c().look(0.0, -60.0);
    t.secs(0.2).await?;
    t.hold("core:attack", 0.1);
    t.secs(0.5).await?;
    t.check_eq(t.op(&sb([0, 1, -1])).await?, "core:air".to_string(), "the stone broken off the flying deck");
    Ok(())
}
