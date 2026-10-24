use std::time::Duration;

use voxide_sdk::*;

use crate::runner::{R, T};

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

fn item(name: &str) -> u32 {
    find_item(name).unwrap_or(u32::MAX)
}

fn at(p: [i32; 3]) -> String {
    format!("{} {} {}", p[0], p[1], p[2])
}

value! {
    /// The base game's hunger component, declared here the same way so the engine checks both
    /// agree on its layout.
    pub struct Hunger { pub food: f32, pub saturation: f32, pub exhaustion: f32, pub air: f32, pub armor: f32, pub timer: u32 }
}
static HUNGER: Component<Hunger> = Component::new("base:hunger");

/// Smelting with coal, the lit state, and a lava bucket leaving its bucket behind.
pub async fn furnace(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let a = add(f, [2, 0, -2]);
    let b = add(f, [-2, 0, -2]);
    t.set(a, "furnace").await?;
    t.set(b, "furnace").await?;
    t.op("events reset").await?;
    t.op(&format!("insert {} raw_iron 1", at(a))).await?;
    t.op(&format!("insert {} coal 1", at(a))).await?;
    t.op(&format!("insert {} sand 1", at(b))).await?;
    t.op(&format!("insert {} lava_bucket 1", at(b))).await?;
    t.secs(1.0).await?;
    let lit = t.block(a).await?;
    t.check(lit.starts_with("base:furnace#") && lit.trim_start_matches("base:furnace#").parse::<u32>().unwrap_or(0) >= 4, format!("burning furnace is {lit}"));
    t.check_eq(t.wait_slot(&at(a), 2, 20.0).await?, ("base:iron_ingot".into(), 1, 0), "furnace output");
    t.check_eq(t.wait_slot(&at(b), 2, 5.0).await?.0, "base:glass".into(), "sand smelted with lava");
    t.check_eq(t.slot(&at(b), 1).await?.0, "base:bucket".into(), "lava bucket leaves the bucket");
    let events = t.op("events").await?;
    t.check(events.contains("smelted=2"), format!("smelted events: {events}"));
    t.look_at(a);
    t.frames(2).await?;
    t.press("core:use");
    t.screen("core:menu").await?;
    t.shot("menu").await?;
    t.c().close_screen();
    t.no_screen().await
}

/// Shaped crafting with tag ingredients (`#planks`) at a crafting table.
pub async fn crafting_table_tags(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let table = add(f, [2, 0, 0]);
    t.set(table, "crafting_table").await?;
    t.op("put me 0 birch_planks 8").await?;
    t.look_at(table);
    t.frames(2).await?;
    t.press("core:use");
    t.screen("core:menu").await?;
    // Crafting table menu: 0-8 grid, 9 result, 10-36 main, 37-45 hotbar.
    let planks = item("base:birch_planks");
    t.click_ui("slot:37", 0);
    t.until("planks on the cursor", Duration::from_secs(5), |t| t.c().cursor_stack().item == planks).await?;
    t.click_ui("slot:0", 1);
    t.frames(3).await?;
    t.click_ui("slot:3", 1);
    let stick = item("base:stick");
    let made = t.until("sticks in the result", Duration::from_secs(5), |t| t.c().menu_slot(9).item == stick).await;
    t.check(made.is_ok(), "two planks in a column give no sticks (tag ingredient #planks)");
    t.click_ui("slot:4", 1);
    t.click_ui("slot:6", 1);
    t.click_ui("slot:7", 1);
    t.frames(5).await?;
    t.click_ui("slot:37", 0);
    t.c().close_screen();
    t.no_screen().await
}

/// Breaking blocks with and without the right tool: drops, pick up, tool wear.
pub async fn mining_and_wear(t: T) -> R {
    t.flat().await?;
    t.op("put me 0 stone_pickaxe 1").await?;
    t.c().select(0);
    let f = t.feet();
    let stone = add(f, [2, 0, 0]);
    t.set(stone, "stone").await?;
    t.look_at(stone);
    t.frames(2).await?;
    // A stone pickaxe takes 0.56 s; letting go right after keeps the block behind intact.
    t.hold("core:attack", 0.8);
    t.secs(1.2).await?;
    t.check_eq(t.block(stone).await?, "core:air".into(), "stone mined");
    t.check_eq(t.slot("me", 0).await?.2, 1, "pickaxe wear");
    t.c().teleport([stone[0] as f64 + 0.5, stone[1] as f64, stone[2] as f64 + 0.5]);
    let cobble = item("base:cobblestone");
    let picked = t.until("cobblestone picked up", Duration::from_secs(5), |t| (0..36).any(|i| t.c().inventory_slot(i).item == cobble)).await;
    t.check(picked.is_ok(), "no cobblestone after mining stone with a pickaxe");
    // Dirt by hand drops dirt.
    t.c().select(4);
    let dirt = add(f, [0, 0, 2]);
    t.set(dirt, "dirt").await?;
    t.look_at(dirt);
    t.frames(2).await?;
    t.hold("core:attack", 0.9);
    t.secs(1.4).await?;
    t.check_eq(t.block(dirt).await?, "core:air".into(), "dirt dug by hand");
    t.c().teleport([dirt[0] as f64 + 0.5, dirt[1] as f64, dirt[2] as f64 + 0.5]);
    let d = item("base:dirt");
    let got = t.until("dirt picked up", Duration::from_secs(5), |t| (0..36).any(|i| t.c().inventory_slot(i).item == d)).await;
    t.check(got.is_ok(), "digging dirt by hand gave nothing");
    Ok(())
}

/// Shift-click puts armour into its slots; hits wear it.
pub async fn armor(t: T) -> R {
    t.flat().await?;
    for (slot, name) in [(0, "iron_helmet"), (1, "diamond_chestplate"), (2, "gold_leggings"), (3, "leather_boots")] {
        t.op(&format!("put me {slot} {name} 1")).await?;
    }
    t.press("core:inventory");
    t.screen("core:menu").await?;
    for i in 36..40 {
        t.c().key("ShiftLeft", 0.3);
        t.frames(1).await?;
        t.click_ui(&format!("slot:{i}"), 0);
        t.frames(4).await?;
    }
    t.until("armour worn", Duration::from_secs(5), |t| (36..40).all(|s| !t.c().inventory_slot(s).is_empty())).await?;
    t.shot("worn").await?;
    t.c().close_screen();
    t.no_screen().await?;
    let f = t.feet();
    t.op(&format!("spawn zombie {}", at(add(f, [1, 0, 1])))).await?;
    t.until("hit by the zombie", Duration::from_secs(15), |t| t.state().health < 20.0).await?;
    t.secs(0.5).await?;
    let mut worn = 0;
    for s in 36..40 {
        worn += t.slot("me", s).await?.2;
    }
    t.check(worn > 0, "armour took no damage from a zombie hit");
    let hp = t.state().health;
    t.check(hp > 17.0, format!("armour did not reduce the hit: health {hp}"));
    Ok(())
}

/// Pouring and scooping water.
pub async fn bucket(t: T) -> R {
    t.flat().await?;
    t.op("put me 0 water_bucket 1").await?;
    t.c().select(0);
    let f = t.feet();
    // A one block hole: the source cannot spread, so it is still there to scoop.
    let water = add(f, [1, -1, 0]);
    t.set(water, "air").await?;
    let ground = add(water, [0, -1, 0]);
    t.look_at(ground);
    t.frames(2).await?;
    t.press("core:use");
    t.until("water poured", Duration::from_secs(5), |t| t.c().inventory_slot(0).item == item("base:bucket")).await?;
    t.check_eq(t.block(water).await?, "base:water".into(), "water source");
    t.secs(1.5).await?;
    t.shot("poured").await?;
    t.look_at(water);
    t.frames(2).await?;
    t.press("core:use");
    t.until("water scooped", Duration::from_secs(5), |t| t.c().inventory_slot(0).item == item("base:water_bucket")).await?;
    Ok(())
}

/// 12 blocks down costs 9 health like vanilla.
pub async fn fall_damage(t: T) -> R {
    t.flat().await?;
    t.until("full health", Duration::from_secs(5), |t| t.state().health >= 20.0).await?;
    let s = t.state();
    t.c().teleport([s.pos[0], s.pos[1] + 12.0, s.pos[2]]);
    t.until("falling", Duration::from_secs(5), |t| !t.flag(client::client_flags::ON_GROUND)).await?;
    t.until("landed", Duration::from_secs(10), |t| t.flag(client::client_flags::ON_GROUND)).await?;
    t.secs(0.5).await?;
    let hp = t.state().health;
    t.check((10.0..=12.0).contains(&hp), format!("health after a 12 block fall: {hp}, vanilla 11"));
    Ok(())
}

/// Sneaking at an edge stops at it instead of falling off.
pub async fn sneak_edge(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let base = add(f, [5, 0, 5]);
    t.fill(base, add(base, [0, 2, 0]), "stone").await?;
    t.c().teleport([base[0] as f64 + 0.5, base[1] as f64 + 3.0, base[2] as f64 + 0.5]);
    t.until("on the pillar", Duration::from_secs(5), |t| t.flag(client::client_flags::ON_GROUND)).await?;
    t.c().look(30.0, 0.0);
    t.hold("core:sneak", 4.0);
    t.frames(2).await?;
    t.hold("core:forward", 2.0);
    t.secs(1.0).await?;
    t.hold("core:left", 1.0);
    t.secs(1.5).await?;
    let y = t.state().pos[1];
    t.check(y > base[1] as f64 + 2.9, format!("fell off while sneaking: y {y:.2}"));
    Ok(())
}

pub async fn commands(t: T) -> R {
    t.world(3).await?;
    let help = t.cmd("/help give").await?;
    t.check(help.contains("/give"), format!("/help give said '{help}'"));
    let bad = t.cmd("/give nothing_here").await?;
    t.check(bad.contains("usage") || bad.contains("unknown"), format!("bad item reply '{bad}'"));
    t.cmd("/summon pig 3").await?;
    t.frames(3).await?;
    t.check_eq(t.count_near("base:pig", 8.0).await?, 3, "summoned pigs");
    let feed = t.cmd("/feed 9").await?;
    t.check(feed.contains('9'), format!("/feed said '{feed}'"));
    let time = t.cmd("/time query").await?;
    t.check(time.contains("time is"), format!("/time query said '{time}'"));
    Ok(())
}

pub async fn mobs(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    for (kind, d) in [("zombie", [0, 0, -5]), ("pig", [3, 0, -5]), ("cow", [-3, 0, -5]), ("spider", [5, 0, -6])] {
        t.op(&format!("spawn {kind} {}", at(add(f, d)))).await?;
    }
    t.until("client sees four mobs", Duration::from_secs(5), |t| t.c().entities().len() >= 4).await?;
    let kinds: Vec<&str> = t.c().entities().iter().filter_map(|&id| t.c().entity(id)).map(|e| name_of(abi::entry_kind::ENTITY, e.kind)).collect();
    for k in ["base:zombie", "base:pig", "base:cow", "base:spider"] {
        t.check(kinds.contains(&k), format!("client has no {k} ({kinds:?})"));
    }
    // No screenshot: their AI wanders from the first tick, so no two frames agree.
    Ok(())
}

/// The test mod's generator powers its electric furnace through cables.
pub async fn energy(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let g = add(f, [2, 0, -3]);
    t.set(g, "testmod:generator").await?;
    t.set(add(g, [-1, 0, 0]), "testmod:cable").await?;
    t.set(add(g, [-2, 0, 0]), "testmod:cable").await?;
    let e = add(g, [-3, 0, 0]);
    t.set(e, "testmod:electric_furnace").await?;
    t.op(&format!("insert {} coal 4", at(g))).await?;
    t.op(&format!("insert {} iron_ore 2", at(e))).await?;
    let out = t.wait_slot(&at(e), 1, 20.0).await?;
    t.check_eq(out.0, "base:iron_ingot".into(), "electric furnace output");
    Ok(())
}

pub async fn eating(t: T) -> R {
    t.flat().await?;
    t.cmd("/feed 10").await?;
    t.op("put me 0 cooked_beef 3").await?;
    t.c().select(0);
    t.until("hunger synced", Duration::from_secs(5), |t| t.c().view().component(&HUNGER).is_some_and(|h| h.food == 10.0)).await?;
    t.press("core:use");
    t.until("ate", Duration::from_secs(5), |t| t.c().inventory_slot(0).count == 2).await?;
    t.until("food went up", Duration::from_secs(5), |t| t.c().view().component(&HUNGER).is_some_and(|h| h.food == 18.0)).await?;
    Ok(())
}

pub async fn drop_and_pickup(t: T) -> R {
    t.flat().await?;
    t.op("put me 0 cobblestone 10").await?;
    t.c().select(0);
    t.c().look(0.0, 0.0);
    t.frames(2).await?;
    t.press("core:drop");
    t.until("one dropped", Duration::from_secs(5), |t| t.c().inventory_slot(0).count == 9).await?;
    t.check(t.count_near("core:item", 8.0).await? == 1, "no item entity after dropping");
    t.secs(2.2).await?;
    t.hold("core:forward", 1.0);
    t.until("picked up again", Duration::from_secs(6), |t| t.c().inventory_slot(0).count == 10).await?;
    Ok(())
}
