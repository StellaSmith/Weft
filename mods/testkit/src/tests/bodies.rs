//! Stage 8: moving collisions with the test mod's lift, skiff and crate.

use crate::runner::{R, T};

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

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
    let p = pos_of(&t, &format!("#{id}")).await?;
    t.op(&format!("onto #{id} 0.5")).await?;
    t.secs(2.0).await?;
    let (me, lift) = (t.state().pos, pos_of(&t, &format!("#{id}")).await?);
    t.check(lift[1] > p[1] + 1.2, "the lift went up");
    t.check((me[1] - (lift[1] + 0.5)).abs() < 0.15, format!("the player rides on top (feet {:.2}, deck {:.2})", me[1], lift[1] + 0.5));
    // Up to the top, then all the way down.
    t.secs(5.0).await?;
    let (me, lift) = (t.state().pos, pos_of(&t, &format!("#{id}")).await?);
    t.check(lift[1] < p[1] + 1.5, "the lift came down");
    t.check((me[1] - (lift[1] + 0.5)).abs() < 0.15, format!("the player rode it down (feet {:.2}, deck {:.2})", me[1], lift[1] + 0.5));
    t.check_eq(t.op("health").await?, "20".to_string(), "health after the ride");
    Ok(())
}

/// Motion in the frame of what one stands on: the camera glides with a lift (no 0.05 block
/// jump a tick), and an idol riding the skiff stands still on its deck, not walking in place.
pub async fn ride_frames(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let id = spawn(&t, "testmod:lift", add(f, [3, 0, 0])).await?;
    t.op(&format!("onto #{id} 0.5")).await?;
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
    t.op(&format!("onto #{skiff} 0.5 #{idol}")).await?;
    t.secs(1.0).await?;
    let (s, i) = (pos_of(&t, &format!("#{skiff}")).await?, pos_of(&t, &format!("#{idol}")).await?);
    t.check((i[0] - s[0]).abs() < 1.6 && (i[1] - (s[1] + 0.5)).abs() < 0.1, format!("the idol rides the skiff (idol {i:?}, skiff {s:?})"));
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
    t.op(&format!("onto #{skiff} 0.5 #{idol}")).await?;
    // Into a leg, away from its turn (it turns every 3 seconds of its age).
    t.secs(0.5).await?;
    let x = |t: &T, id| t.c().entity(id).map_or(0.0, |e| e.pos[0]);
    // Per frame: the skiff's speed, and how far the rider moved on its deck.
    let (mut speeds, mut slips) = (Vec::new(), Vec::new());
    let (mut last, mut rel, mut at) = (x(&t, skiff), x(&t, idol) - x(&t, skiff), std::time::Instant::now());
    for _ in 0..90 {
        t.frames(1).await?;
        let (now, now_rel, when) = (x(&t, skiff), x(&t, idol) - x(&t, skiff), std::time::Instant::now());
        let dt = (when - at).as_secs_f64();
        if dt > 0.0005 {
            speeds.push((now - last).abs() / dt);
        }
        slips.push((now_rel - rel).abs());
        (last, rel, at) = (now, now_rel, when);
    }
    // The turn may fall into the window: judge 90% of the frames.
    let tail = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        (v[v.len() / 20], v[v.len() - 1 - v.len() / 20])
    };
    let ((lo, hi), (_, slip)) = (tail(&mut speeds), tail(&mut slips));
    t.note(format!("skiff speed over {} frames {lo:.2}..{hi:.2} blocks/s, rider moves on the deck up to {slip:.4} a frame", speeds.len()));
    t.check(lo > 3.0 && hi < 9.0, "the skiff glides (no stops or jumps)");
    t.check(slip < 0.02, "its rider stays on its spot of the deck");
    // Riding it too: the deck keeps its place in front of the camera.
    t.op(&format!("onto #{skiff} 0.5")).await?;
    t.secs(0.5).await?;
    let mut steps = Vec::new();
    let mut last = x(&t, skiff) - t.state().shown[0];
    for _ in 0..90 {
        t.frames(1).await?;
        let now = x(&t, skiff) - t.state().shown[0];
        steps.push((now - last).abs());
        last = now;
    }
    let (_, big) = tail(&mut steps);
    t.note(format!("skiff seen from its rider: steps up to {big:.4}"));
    t.check(big < 0.02, "the skiff stays put in front of a rider");
    Ok(())
}

/// Jumping on a skiff flying 6 blocks a second lands on it again: riders move with it up to its
/// carry height.
pub async fn skiff(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let id = spawn(&t, "testmod:skiff", add(f, [0, 3, 0])).await?;
    let p = pos_of(&t, &format!("#{id}")).await?;
    t.op(&format!("onto #{id} 0.5")).await?;
    t.secs(0.5).await?;
    // Jumping through its turn (it flies back after 3 seconds), then landing.
    t.hold("core:jump", 2.5);
    t.secs(3.5).await?;
    // Both as the server has them: the player's position arrives every tick from its client.
    let (me, skiff) = (pos_of(&t, "me").await?, pos_of(&t, &format!("#{id}")).await?);
    t.check((skiff[0] - p[0]).abs() > 5.0, "the skiff flew on");
    // The skiff moves on while the teleport arrives, so the player may stand anywhere on the
    // deck (1.5 + 0.3 from its middle); the server also hears the player a tick late (0.3).
    t.check((me[0] - skiff[0]).abs() < 2.1 && (me[2] - skiff[2]).abs() < 2.1, format!("the player is still over it (x {:.2}, skiff {:.2})", me[0], skiff[0]));
    t.check((me[1] - (skiff[1] + 0.5)).abs() < 0.15, format!("and back on its deck (feet {:.2}, deck {:.2})", me[1], skiff[1] + 0.5));
    Ok(())
}

/// A crate falls like any entity, can be stood on, and mobs (soft bodies) standing in one spot
/// drift apart.
pub async fn crate_and_crowd(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let id = spawn(&t, "testmod:crate", add(f, [0, 3, -3])).await?;
    let a = spawn(&t, "testmod:idol", add(f, [3, 0, -3])).await?;
    let b = spawn(&t, "testmod:idol", add(f, [3, 0, -3])).await?;
    t.secs(1.5).await?;
    let c = pos_of(&t, &format!("#{id}")).await?;
    t.check((c[1] - f[1] as f64).abs() < 0.01, format!("the crate landed (y {:.2})", c[1]));
    let (pa, pb) = (pos_of(&t, &format!("#{a}")).await?, pos_of(&t, &format!("#{b}")).await?);
    t.check(((pa[0] - pb[0]).powi(2) + (pa[2] - pb[2]).powi(2)).sqrt() > 0.4, "two idols in one spot pushed each other apart");
    t.cmd(&format!("/tp {} {} {}", c[0], c[1] + 1.1, c[2])).await?;
    t.secs(1.0).await?;
    let me = t.state().pos;
    t.check((me[1] - (c[1] + 1.0)).abs() < 0.01, format!("the player stands on the crate (feet {:.2})", me[1]));
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

/// Navigation finds the way around a wall through its only gap, where walking straight at the
/// target would stop at the wall.
pub async fn pathfinding(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    // A wall 3 high across z -6..6 at x + 4, open at z + 5.
    t.fill(add(f, [4, 0, -6]), add(f, [4, 2, 6]), "cobblestone").await?;
    t.fill(add(f, [4, 0, 5]), add(f, [4, 1, 5]), "air").await?;
    let id = spawn(&t, "testmod:idol", add(f, [1, 0, 0])).await?;
    let goal = add(f, [8, 0, 0]);
    let found = t.op(&format!("navigate #{id} {} {} {}", goal[0], goal[1], goal[2])).await?;
    t.check_eq(found, "1".to_string(), "the walk is accepted");
    t.secs(8.0).await?;
    let p = pos_of(&t, &format!("#{id}")).await?;
    let d = ((p[0] - goal[0] as f64 - 0.5).powi(2) + (p[2] - goal[2] as f64 - 0.5).powi(2)).sqrt();
    t.check(d < 1.5, format!("the idol walked around the wall ({:.1} {:.1}, {d:.1} from the target)", p[0], p[2]));
    Ok(())
}

/// A zombie's goals: it finds its way around a wall to the player and hits it.
/// Mobs do not notice a creative player (its `visibility` is 0), and do again in survival.
pub async fn creative_unseen(t: T) -> R {
    t.flat().await?;
    t.op("mode creative").await?;
    let f = t.feet();
    let id = spawn(&t, "zombie", add(f, [6, 0, 0])).await?;
    t.secs(3.0).await?;
    let z = pos_of(&t, &format!("#{id}")).await?;
    t.check((z[0] - f[0] as f64).abs() > 4.0 || (z[2] - f[2] as f64).abs() > 4.0, format!("the zombie left the creative player alone (x {:.1})", z[0]));
    t.op("mode survival").await?;
    t.secs(4.0).await?;
    let z = pos_of(&t, &format!("#{id}")).await?;
    let me = t.state().pos;
    t.check(((z[0] - me[0]).powi(2) + (z[2] - me[2]).powi(2)).sqrt() < 2.5, format!("in survival it came (x {:.1})", z[0]));
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
    t.shot_within("pig", 0.03).await?;
    t.cmd("/morph").await?;
    t.cmd(&format!("/tp {} {} {}", f[0], f[1], f[2] + 3)).await?;
    t.secs(0.3).await?;
    t.hold("core:forward", 1.0);
    t.secs(1.1).await?;
    let z = t.state().pos[2];
    t.check(z > f[2] as f64 - 1.6, format!("as a player again it does not fit (z {z:.2})"));
    Ok(())
}

pub async fn zombie_chases(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    t.fill(add(f, [4, 0, -6]), add(f, [4, 2, 6]), "cobblestone").await?;
    t.fill(add(f, [4, 0, 5]), add(f, [4, 1, 5]), "air").await?;
    let id = spawn(&t, "zombie", add(f, [8, 0, 0])).await?;
    t.secs(10.0).await?;
    let z = pos_of(&t, &format!("#{id}")).await?;
    let health: f32 = t.op("health").await?.parse().unwrap_or(20.0);
    t.check(z[0] < f[0] as f64 + 4.0, format!("the zombie came round the wall (x {:.1})", z[0]));
    t.check(health < 20.0, format!("and hit the player (health {health})"));
    Ok(())
}
