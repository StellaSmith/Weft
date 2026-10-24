//! Events, interceptors, value chains, attributes and loot tables (stage 3).

use std::collections::HashMap;
use std::time::Duration;

use voxide_sdk::*;

use crate::runner::{R, T};

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

fn at(p: [i32; 3]) -> String {
    format!("{} {} {}", p[0], p[1], p[2])
}

fn counts(s: &str) -> HashMap<String, u32> {
    s.split_whitespace().filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.parse().unwrap_or(0))).collect()
}

/// First number of an `attr`/`damage` answer ("6.08 | damage: 8, ...").
fn value(s: &str) -> f64 {
    s.split('|').next().and_then(|v| v.trim().parse().ok()).unwrap_or(f64::NAN)
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

pub async fn interceptor_protects(t: T) -> R {
    t.flat().await?;
    let p = add(t.feet(), [2, 0, 0]);
    t.set(p, "gold_block").await?;
    t.cmd("/rule testkit:protect_gold 1").await?;
    t.check_eq(t.op(&format!("break {} diamond_pickaxe", at(p))).await?, "0".into(), "break refused");
    t.op("put me 0 diamond_pickaxe 1").await?;
    t.c().select(0);
    t.look_at(p);
    t.frames(3).await?;
    t.hold("core:attack", 1.5);
    t.secs(2.0).await?;
    t.check_eq(t.block(p).await?, "base:gold_block".into(), "a player cannot break it either");
    t.cmd("/rule testkit:protect_gold 0").await?;
    t.check_eq(t.op(&format!("break {} diamond_pickaxe", at(p))).await?, "1".into(), "break allowed again");
    t.check_eq(t.block(p).await?, "core:air".into(), "broken");
    Ok(())
}

/// Damage passes the base game's armour step and the test kit's final step, in stage order,
/// and the result can be explained.
pub async fn damage_chain(t: T) -> R {
    t.flat().await?;
    let bare = t.op("damage 8 0").await?;
    t.check((value(&bare) - 8.0).abs() < 1e-6, format!("no armour: {bare}"));
    // Iron chestplate: 6 points, 4% each.
    t.op("put me 37 iron_chestplate 1").await?;
    let armoured = t.op("damage 8 0").await?;
    t.check((value(&armoured) - 6.08).abs() < 1e-4, format!("iron chestplate: {armoured}"));
    let fall = t.op(&format!("damage 8 {}", abi::damage::FALL)).await?;
    t.check((value(&fall) - 8.0).abs() < 1e-6, format!("armour does not soften falls: {fall}"));
    t.cmd("/rule testkit:half_damage 1").await?;
    let both = t.op("damage 8 0").await?;
    t.check((value(&both) - 3.04).abs() < 1e-4, format!("armour then halved: {both}"));
    t.check(both.contains("base at stage 200") && both.contains("testkit at stage 300"), format!("explanation names both steps: {both}"));
    t.note(both);
    t.cmd("/rule testkit:half_damage 0").await?;
    Ok(())
}

pub async fn attributes(t: T) -> R {
    t.flat().await?;
    t.check_eq(value(&t.op("attr me movement_speed").await?), 1.0, "speed without modifiers");
    t.op("attr_mod me movement_speed testkit:haste 1 0.5 40").await?;
    t.op("attr_mod me max_health testkit:vigor 0 10 0").await?;
    let speed = t.op("attr me movement_speed").await?;
    t.check_eq(value(&speed), 1.5, "speed with +50% of base");
    t.check(speed.contains("testkit:haste"), format!("explained: {speed}"));
    t.check_eq(value(&t.op("attr me max_health").await?), 30.0, "max health +10");
    t.secs(2.5).await?;
    t.check_eq(value(&t.op("attr me movement_speed").await?), 1.0, "the 40 tick modifier ran out");
    // Permanent ones are saved with the player.
    let name = format!("test-{}", t.name());
    t.c().leave_world();
    t.screen("core:title").await?;
    t.c().open_world(&name, 1);
    t.until("joined again", Duration::from_secs(60), |t| t.flag(client::client_flags::JOINED)).await?;
    t.loaded().await?;
    t.check_eq(value(&t.op("attr me max_health").await?), 30.0, "max health modifier after reload");
    // One maximum everywhere: /heal fills to it and the client hears the same.
    t.cmd("/heal").await?;
    t.check_eq(t.op("health").await?, "30".to_string(), "healed to the modified maximum");
    t.until("the client has 30", Duration::from_secs(2), |t| t.state().health == 30.0).await?;
    t.op("attr_rm me max_health testkit:vigor").await?;
    t.check_eq(value(&t.op("attr me max_health").await?), 20.0, "removed");
    Ok(())
}

/// Vanilla's chances, measured over many rolls; a modifier from another mod adds to a table.
pub async fn loot_tables(t: T) -> R {
    t.flat().await?;
    let oak = counts(&t.op("loot base:oak_leaves 4000").await?);
    let saplings = oak.get("base:sapling").copied().unwrap_or(0);
    t.check((140..=260).contains(&saplings), format!("saplings from 4000 oak leaves: {saplings} (vanilla 5%: 200)"));
    let apples = oak.get("base:apple").copied().unwrap_or(0);
    t.check((5..=40).contains(&apples), format!("apples from 4000 oak leaves: {apples} (vanilla 0.5%: 20)"));
    let birch = counts(&t.op("loot base:birch_leaves 2000").await?);
    t.check(!birch.contains_key("base:apple"), "birch leaves gave apples");
    let zombie = counts(&t.op("loot base:zombie 2000").await?);
    let flesh = zombie.get("base:rotten_flesh").copied().unwrap_or(0);
    t.check((1700..=2300).contains(&flesh), format!("rotten flesh from 2000 zombies: {flesh} (0..2 each)"));
    let tin = zombie.get("testmod:tin_ingot").copied().unwrap_or(0);
    t.check((50..=160).contains(&tin), format!("tin from 2000 zombies through testmod's modifier: {tin} (5%: 100)"));
    let pig = counts(&t.op("loot base:pig 200 fire").await?);
    t.check(pig.contains_key("base:cooked_porkchop") && !pig.contains_key("base:porkchop"), format!("burning pigs drop cooked meat: {pig:?}"));
    let grass = counts(&t.op("loot base:tall_grass 4000").await?);
    let seeds = grass.get("base:wheat_seeds").copied().unwrap_or(0);
    t.check((380..=620).contains(&seeds), format!("seeds from 4000 tall grass: {seeds} (vanilla 12.5%: 500)"));
    let stone = counts(&t.op("loot base:stone 10").await?);
    t.check_eq(stone.get("base:cobblestone").copied(), Some(10), "drops() is a one entry table");
    Ok(())
}

/// Harvest levels from modern vanilla's tags: iron ore needs a stone pickaxe.
pub async fn tool_tiers(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let a = add(f, [3, 0, 3]);
    t.set(a, "iron_ore").await?;
    t.op(&format!("break {} wood_pickaxe", at(a))).await?;
    t.frames(5).await?;
    t.check_eq(t.count_near("core:item", 8.0).await?, 0, "wooden pickaxe on iron ore drops nothing");
    let b = add(f, [-3, 0, 3]);
    t.set(b, "iron_ore").await?;
    t.op(&format!("break {} stone_pickaxe", at(b))).await?;
    t.frames(5).await?;
    t.check_eq(t.count_near("core:item", 8.0).await?, 1, "stone pickaxe on iron ore drops raw iron");
    let d = add(f, [0, 0, 4]);
    t.set(d, "diamond_ore").await?;
    t.op(&format!("break {} stone_pickaxe", at(d))).await?;
    t.frames(5).await?;
    t.check_eq(t.count_near("core:item", 8.0).await?, 1, "stone pickaxe on diamond ore adds nothing");
    // Tiers are numbers: the test mod's steel pickaxe (250) mines what iron (200) does, and its
    // reinforced stone (250), which iron cannot; diamond ore (300) stays out of its reach.
    let mut drops = 1;
    for (block, tool, gets) in [("testmod:reinforced_stone", "iron_pickaxe", false), ("testmod:reinforced_stone", "testmod:steel_pickaxe", true), ("gold_ore", "testmod:steel_pickaxe", true), ("obsidian", "testmod:steel_pickaxe", false)] {
        let p = add(f, [drops * 2 - 6, 0, -4]);
        t.set(p, block).await?;
        t.op(&format!("break {} {tool}", at(p))).await?;
        t.frames(5).await?;
        drops += gets as i32;
        t.check_eq(t.count_near("core:item", 12.0).await?, drops as usize, &format!("{tool} on {block}: {}", if gets { "drops" } else { "nothing" }));
    }
    Ok(())
}

/// Blocks face only the directions they allow, one state each: the test mod's vent (up, north,
/// east) has three, the furnace four times lit or not; placing picks the allowed one nearest
/// the player's look.
pub async fn facing_states(t: T) -> R {
    static VENT: Block = Block::named("testmod:vent");
    static FURNACE: Block = Block::named("base:furnace");
    static TORCH: Block = Block::named("base:torch");
    t.flat().await?;
    t.check_eq(VENT.state_count(), 3, "the vent's states");
    t.check_eq(FURNACE.state_count(), 8, "the furnace's states");
    t.check_eq(TORCH.state_count(), 5, "the torch's states");
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

/// The owner's example: another mod's component on the base game's food plus the `eaten` event.
pub async fn healing_food(t: T) -> R {
    t.flat().await?;
    let s = t.state();
    t.c().teleport([s.pos[0], s.pos[1] + 8.0, s.pos[2]]);
    t.until("falling", Duration::from_secs(5), |t| !t.flag(client::client_flags::ON_GROUND)).await?;
    t.until("landed", Duration::from_secs(10), |t| t.flag(client::client_flags::ON_GROUND)).await?;
    t.until("hurt", Duration::from_secs(3), |t| t.state().health < 20.0).await?;
    let before = t.state().health;
    t.cmd("/feed 10").await?;
    t.op("put me 0 cooked_beef 2").await?;
    t.c().select(0);
    t.secs(0.3).await?;
    t.press("core:use");
    t.until("healed", Duration::from_secs(3), |t| t.state().health >= before + 2.0).await?;
    let after = t.state().health;
    t.check(after <= before + 3.0, format!("health {before} -> {after}: two from testmod's food_heal (plus maybe a regen tick)"));
    Ok(())
}
