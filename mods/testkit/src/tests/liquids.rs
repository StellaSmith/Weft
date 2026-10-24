//! Liquids against vanilla's numbers: water steps every 5 ticks and reaches 7 blocks, lava every
//! 30 ticks and 3 blocks; flowing liquid heads for the nearest way down.

use std::time::Duration;

use voxide_sdk::*;

use crate::runner::{R, T};

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

/// On open flat ground: diamond of radius 7, level 7 at the rim, not faster than vanilla.
pub async fn water_flow(t: T) -> R {
    t.flat().await?;
    let s = add(t.feet(), [12, 0, 0]);
    t.set(s, "water").await?;
    // Ring k arrives after k * 5 ticks: at one second only about four rings exist.
    t.secs(1.0).await?;
    t.check_eq(t.block(add(s, [3, 0, 0])).await?, "base:water#3".into(), "three blocks out after a second");
    t.check_eq(t.block(add(s, [6, 0, 0])).await?, "core:air".into(), "six blocks out after a second");
    t.secs(2.5).await?;
    for (d, want) in [([7, 0, 0], "base:water#7"), ([0, 0, -7], "base:water#7"), ([4, 0, 3], "base:water#7"), ([8, 0, 0], "core:air"), ([4, 0, 4], "core:air"), ([1, 0, 0], "base:water#1")] {
        t.check_eq(t.block(add(s, d)).await?, want.into(), &format!("water at {d:?}"));
    }
    t.look_at(add(s, [0, -1, 0]));
    t.secs(0.3).await?;
    t.shot("spread").await
}

/// Flowing water goes only towards a hole within reach instead of in every direction.
pub async fn water_finds_hole(t: T) -> R {
    t.flat().await?;
    let s = add(t.feet(), [-10, 0, 0]);
    let hole = add(s, [3, -1, 0]);
    t.set(hole, "air").await?;
    t.set(s, "water").await?;
    t.secs(3.0).await?;
    t.check_eq(t.block(hole).await?, "base:water#8".into(), "the hole filled (falling water)");
    t.check_eq(t.block(add(s, [-1, 0, 0])).await?, "core:air".into(), "no spread away from the hole");
    t.check_eq(t.block(add(s, [0, 0, 1])).await?, "core:air".into(), "no spread sideways");
    Ok(())
}

/// Two sources with one block between them over solid ground make a third.
pub async fn infinite_water(t: T) -> R {
    t.flat().await?;
    let a = add(t.feet(), [5, 0, 6]);
    t.set(a, "water").await?;
    t.set(add(a, [2, 0, 0]), "water").await?;
    t.secs(1.5).await?;
    t.check_eq(t.block(add(a, [1, 0, 0])).await?, "base:water".into(), "new source between two");
    Ok(())
}

/// Lava touched by water: sources harden to obsidian, flowing lava to cobblestone; lava falling
/// into water makes stone.
pub async fn lava_and_water(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let a = add(f, [8, 0, -8]);
    t.fill(add(a, [-1, -1, -1]), add(a, [3, -1, 1]), "stone").await?;
    t.set(a, "lava").await?;
    t.set(add(a, [2, 0, 0]), "water").await?;
    t.secs(1.0).await?;
    t.check_eq(t.block(a).await?, "base:obsidian".into(), "lava source next to water");

    let b = add(f, [-8, 0, 8]);
    t.set(b, "lava").await?;
    // Lava needs 30 ticks per step.
    t.secs(2.0).await?;
    t.check_eq(t.block(add(b, [1, 0, 0])).await?, "base:lava#2".into(), "lava flowed one block");
    t.set(add(b, [1, 1, 0]), "water").await?;
    t.secs(0.5).await?;
    t.check_eq(t.block(add(b, [1, 0, 0])).await?, "base:cobblestone".into(), "flowing lava under water");

    let c = add(f, [8, 0, 8]);
    t.set(c, "water").await?;
    t.set(add(c, [0, 2, 0]), "lava").await?;
    t.secs(4.0).await?;
    t.check_eq(t.block(c).await?, "base:stone".into(), "lava falling into water");
    Ok(())
}

/// Lava burns: a pig in it dies, a player loses health and keeps burning after getting out.
pub async fn lava_burns(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let pool = add(f, [6, -1, 0]);
    t.fill(add(pool, [0, 0, 0]), add(pool, [1, 0, 1]), "lava").await?;
    let pig: u64 = t.op(&format!("spawn pig {} {} {}", pool[0], pool[1], pool[2])).await?.parse().map_err(|_| "no pig".to_string())?;
    let mut gone = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        if t.op(&format!("entity {pig}")).await.is_err() {
            gone = true;
            break;
        }
        t.secs(0.5).await?;
    }
    t.check(gone, "a pig in lava did not die within 8 seconds");
    t.c().teleport([pool[0] as f64 + 0.5, pool[1] as f64 + 0.1, pool[2] as f64 + 0.5]);
    t.until("hurt by lava", Duration::from_secs(5), |t| t.state().health <= 16.0).await?;
    t.c().teleport([f[0] as f64 + 0.5, f[1] as f64, f[2] as f64 + 0.5]);
    t.secs(0.5).await?;
    let out = t.state().health;
    t.secs(2.5).await?;
    let later = t.state().health;
    t.check(later < out, format!("still burning after leaving lava: health {out} -> {later}"));
    Ok(())
}

/// The test mod's oil uses the base game's bucket without either knowing about buckets.
pub async fn mod_liquid_bucket(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let pit = add(f, [1, -1, 0]);
    t.set(pit, "testmod:oil").await?;
    t.op("put me 0 bucket 1").await?;
    t.c().select(0);
    t.look_at(pit);
    t.frames(3).await?;
    t.press("core:use");
    let oil_bucket = find_item("testmod:oil_bucket").unwrap_or(u32::MAX);
    t.until("oil scooped", Duration::from_secs(5), |t| t.c().inventory_slot(0).item == oil_bucket).await?;
    t.check_eq(t.block(pit).await?, "core:air".into(), "oil taken from the pit");
    // Right clicks repeat at most every quarter second.
    t.secs(0.3).await?;
    t.look_at(add(pit, [0, -1, 0]));
    t.frames(3).await?;
    t.press("core:use");
    t.until("oil poured", Duration::from_secs(5), |t| t.c().inventory_slot(0).item == find_item("base:bucket").unwrap_or(u32::MAX)).await?;
    t.check_eq(t.block(pit).await?, "testmod:oil".into(), "oil back in the pit");
    Ok(())
}

/// A mod's block tagged `sand` holds cactus and sugar cane; dirt does not hold cactus.
pub async fn mod_sand_soil(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let sand = add(f, [2, -1, 0]);
    t.set(sand, "testmod:red_sand").await?;
    t.op("put me 0 cactus 4").await?;
    t.c().select(0);
    t.look_at(sand);
    t.frames(3).await?;
    t.press("core:use");
    t.until("cactus placed", Duration::from_secs(5), |t| t.c().inventory_slot(0).count == 3).await?;
    t.check_eq(t.block(add(sand, [0, 1, 0])).await?, "base:cactus".into(), "cactus on red sand");
    let dirt = add(f, [-2, -1, 0]);
    t.set(dirt, "dirt").await?;
    t.secs(0.3).await?;
    t.look_at(dirt);
    t.frames(3).await?;
    t.press("core:use");
    t.secs(0.5).await?;
    t.check_eq(t.block(add(dirt, [0, 1, 0])).await?, "core:air".into(), "cactus refused on dirt");
    t.check_eq(t.c().inventory_slot(0).count, 3, "cactus kept after a refused placement");
    Ok(())
}
