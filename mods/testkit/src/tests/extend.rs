//! Batched events and placement by facing; the other hooks are checked in the scene.

use std::collections::HashMap;
use std::time::Duration;


use super::{add, at};
use crate::runner::{R, T};

fn counts(s: &str) -> HashMap<String, u32> {
    s.split_whitespace().filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.parse().unwrap_or(0))).collect()
}

pub async fn events_batched(t: T) -> R {
    t.flat().await?;
    t.op("events reset").await?;
    let f = t.feet();
    for i in 0..3 {
        let p = add(f, [3, 0, i]);
        t.set(p, "stone").await?;
        t.op(&format!("break {} stone_pickaxe", at(p))).await?;
    }
    t.op("put me 0 dirt 4").await?;
    t.c().select(0);
    let ground = add(f, [0, -1, -2]);
    t.look_at(ground);
    t.frames(3).await?;
    t.press("core:use");
    t.until("dirt placed", Duration::from_secs(5), |t| t.c().inventory_slot(0).count == 3).await?;
    t.cmd("/feed 10").await?;
    t.op("put me 1 bread 2").await?;
    t.c().select(1);
    t.secs(0.3).await?;
    t.press("core:use");
    t.until("ate", Duration::from_secs(5), |t| t.c().inventory_slot(1).count == 1).await?;
    // The broken stone dropped cobblestone next to the player.
    t.c().teleport([f[0] as f64 + 3.5, f[1] as f64, f[2] as f64 + 1.5]);
    t.secs(1.0).await?;
    let n = counts(&t.op("events").await?);
    t.check_eq(n["block_broken"], 3, "block_broken events");
    t.check_eq(n["block_placed"], 1, "block_placed events");
    t.check_eq(n["eaten"], 1, "eaten events (base game's own event)");
    t.check(n["item_picked_up"] >= 1, format!("item_picked_up events: {}", n["item_picked_up"]));
    Ok(())
}

/// Placing picks the allowed facing nearest the player's look: the test mod's vent (up, north,
/// east) put against a wall faces the player.
pub async fn facing_placement(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    t.op("mode creative").await?;
    t.op("put me 0 testmod:vent 4").await?;
    t.c().select(0);
    // Against a wall two blocks south, looking at it: the front turns north, towards the player
    // (looking steeply down it would face up).
    let wall = add(f, [0, 0, 3]);
    t.fill(wall, add(wall, [0, 1, 0]), "cobblestone").await?;
    t.look_at(wall);
    t.frames(2).await?;
    t.press("core:use");
    t.secs(0.3).await?;
    t.check_eq(t.block(add(wall, [0, 0, -1])).await?, "testmod:vent#2".to_string(), "the vent faces north (east, up, north in face order)");
    Ok(())
}


/// One naming rule for every kind of thing: a name without a namespace is the declaring mod's,
/// another mod's is written in full, and both resolve the same way when read.
pub async fn names(t: T) -> R {
    use weft_sdk::*;
    static PROBE: Key = Key::named("probe");
    static OWN_EVENT: Event<u32> = Event::named("marks");
    static COAL: Item = Item::named("base:coal");
    static NOWHERE: Block = Block::named("no_such_block");
    t.flat().await?;
    t.check_eq(PROBE.name().to_string(), "testkit:probe".into(), "a key's relative name");
    t.check_eq(OWN_EVENT.name().to_string(), "testkit:marks".into(), "an event's relative name");
    t.check_eq(screens::PAUSE.name().to_string(), "core:pause".into(), "an engine screen");
    t.check_eq(question::BREAK_BLOCK.name().to_string(), "core:break_block".into(), "an engine question");
    t.check_eq(Some(COAL.id()), Item::lookup("base:coal"), "another mod's item by handle and by typed name");
    t.check_eq(Item::generated("base:coal".into()).id(), COAL.id(), "a generated handle");
    t.check_eq(NOWHERE.id(), u32::MAX, "an unknown block");
    Ok(())
}
