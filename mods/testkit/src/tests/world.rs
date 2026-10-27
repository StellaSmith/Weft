use std::time::Duration;

use weft_sdk::abi::entry_kind as ek;
use weft_sdk::terrain::Column;
use weft_sdk::*;

use super::add;
use super::probe;
use crate::runner::{R, T};

/// A player saved in a dimension that is gone (its mod removed) keeps everything and starts at
/// the spawn; the world remembers the mods it was played with.
pub async fn missing_dimension(t: T) -> R {
    t.world(7).await?;
    t.op("put me 3 diamond 5").await?;
    let flat = Dimension::lookup("base:flatlands").ok_or("no flatlands")?;
    t.cmd("/dim base:flatlands").await?;
    t.until("in the flatlands", Duration::from_secs(20), |t| t.state().dim == flat).await?;
    let name = format!("test-{}", t.name());
    t.c().leave_world();
    t.screen("core:title").await?;
    let dir = format!("{}/saves/{name}", t.c().run_dir());
    let world = std::fs::read_to_string(format!("{dir}/world.txt")).map_err(|e| e.to_string())?;
    t.check(world.contains("mod.base = ") && world.contains("mod.testkit = "), "world.txt lists the mods");
    std::fs::write(format!("{dir}/world.txt"), world + "mod.ghost = 1.0\n").map_err(|e| e.to_string())?;
    for f in std::fs::read_dir(format!("{dir}/players")).map_err(|e| e.to_string())?.flatten() {
        let src = std::fs::read_to_string(f.path()).map_err(|e| e.to_string())?;
        std::fs::write(f.path(), src.replace("dim = base:flatlands", "dim = ghost:gone")).map_err(|e| e.to_string())?;
    }
    // Opened from the list like a player would: the missing mod is named first.
    t.c().open_screen(&screens::WORLDS);
    t.screen("core:worlds").await?;
    t.frames(2).await?;
    // A click lasts 50 ms; buttons act on release.
    t.click_ui(&format!("text:{name} |"), 0);
    t.secs(0.2).await?;
    t.click_ui("id:play", 0);
    t.screen("core:missing_mods").await?;
    t.frames(2).await?;
    t.shot("missing_mods").await?;
    t.click_ui("id:continue", 0);
    t.until("joined again", Duration::from_secs(60), |t| t.flag(client::client_flags::JOINED)).await?;
    t.loaded().await?;
    t.check_eq(t.state().dim, Dimension::lookup("base:overworld").unwrap_or(u32::MAX), "the player is in the spawn dimension");
    t.check_eq(t.slot("me", 3).await?, ("base:diamond".into(), 5, 0), "and kept the inventory");
    Ok(())
}

/// Menus open on virtual inventories and on far away blocks; a menu for a block without an
/// entity is refused with a reason instead of opening something broken.
pub async fn virtual_menus(t: T) -> R {
    t.flat().await?;
    let id = t.op("vmenu base:chest 27 diamond 3 stick 10").await?;
    t.screen("core:menu").await?;
    let diamond = Item::lookup("base:diamond").ok_or("no item base:diamond")?;
    t.until("chest contents on the client", Duration::from_secs(5), |t| t.c().menu_slot(0).item == diamond).await?;
    t.check_eq(t.c().menu_slot(1).count, 10, "second virtual slot");
    t.shot("virtual_chest").await?;
    t.c().close_screen();
    t.no_screen().await?;
    t.op(&format!("remove #{id}")).await?;

    let f = t.feet();
    let plain = add(f, [2, -1, 0]);
    match t.op(&format!("menu base:furnace {} {} {}", plain[0], plain[1], plain[2])).await {
        Ok(_) => t.fail("a furnace menu opened on grass"),
        Err(e) => {
            t.check(e.contains("no block entity"), format!("unexpected refusal: {e}"));
        }
    }
    t.frames(5).await?;
    t.check(t.state().screen.len == 0, "a screen opened for the refused menu");

    // Out of reach and one after another.
    let machines = [("base:furnace", "base:furnace", [20, 0, 0]), ("base:chest", "base:chest", [-20, 0, 5]), ("testmod:generator", "testmod:generator", [0, 0, 24])];
    for (block, menu, d) in machines {
        let p = add(f, d);
        t.set(p, block).await?;
        t.op(&format!("menu {menu} {} {} {}", p[0], p[1], p[2])).await?;
        let want = Menu::lookup(menu).unwrap_or(u32::MAX);
        t.until(&format!("{menu} open"), Duration::from_secs(5), |t| t.c().menu().is_some_and(|m| m[0] == want)).await?;
        t.screen("core:menu").await?;
    }
    t.c().close_screen();
    t.no_screen().await
}

/// Fresh world load time and idle frame time on this machine (thresholds are generous: they
/// catch large regressions, the numbers themselves go to the report).
pub async fn load_benchmark(t: T) -> R {
    let start = std::time::Instant::now();
    t.world(5).await?;
    t.idle().await?;
    let s = t.state();
    t.note(format!("idle after {:.2} s with {} cubes (generated {}, {:.0} us each)", start.elapsed().as_secs_f32(), s.cubes, s.generated, s.gen_us));
    let b = t.bench(4.0, "idle").await?;
    t.check(b[2] < 50.0, format!("p99 frame {:.1} ms", b[2]));
    t.check(b[4] < 25.0, format!("server tick {:.1} ms", b[4]));
    Ok(())
}

use base_api::OVERWORLD;

/// Testmod edits base's overworld from outside: a biome of its own tried before base's desert,
/// with its own surface and boulders, and a rock needle from a feature written in testmod.
pub async fn foreign_terrain(t: T) -> R {
    t.world(5).await?;
    t.op("mode spectator").await?;
    let seed = 5;
    // testmod's `NEEDLE`.
    let needle = OVERWORLD.column(seed, 1000, 1000);
    let beside = OVERWORLD.column(seed, 1010, 1000);
    t.note(format!("needle {} beside {}", needle.height, beside.height));
    t.check(needle.height - beside.height > 60, "the needle rises from the land around it");
    t.cmd(&format!("/tp 1000 {} 1000", needle.height + 3)).await?;
    t.secs(0.5).await?;
    t.loaded().await?;
    let top = t.op(&format!("get 1000 {} 1000", needle.height)).await?;
    t.check(top != "core:air", format!("the needle is generated where the terrain says ({top})"));

    let grid: Vec<(i32, i32, Column)> = (-80..80).flat_map(|i| (-80..80).map(move |j| (i * 64, j * 64))).map(|(x, z)| (x, z, OVERWORLD.column(seed, x, z))).collect();
    let count = |name: &str| grid.iter().filter(|g| name_of(ek::BIOME, g.2.biome) == name).count();
    t.note(format!("of {} columns: desert {}, scorched {}", grid.len(), count("base:desert"), count("testmod:scorched")));
    let scorched = grid
        .into_iter()
        .find(|&(x, z, c)| name_of(ek::BIOME, c.biome) == "testmod:scorched" && !OVERWORLD.surface_carved(seed, x, z));
    let (x, z, c) = scorched.ok_or("no scorched biome in base's overworld")?;
    t.note(format!("scorched at {x} {z}, height {}", c.height));
    t.cmd(&format!("/tp {x} {} {z}", c.height + 3)).await?;
    t.secs(0.5).await?;
    t.loaded().await?;
    let ground = t.op(&format!("get {x} {} {z}", c.height)).await?;
    t.check(ground == "testmod:regolith" || ground == "testmod:basalt", format!("the scorched biome's surface ({ground})"));
    let census = t.op("census 24 testmod:basalt,testmod:regolith").await?;
    t.note(census.clone());
    let n: Vec<u32> = census.split_whitespace().filter_map(|e| e.rsplit_once('=')?.1.parse().ok()).collect();
    t.check(n.first().is_some_and(|&b| b > 0), "boulders from testmod's decoration");
    t.check(n.get(1).is_some_and(|&r| r > 100), "the biome's regolith");
    Ok(())
}

/// `/tick`: a frozen world keeps its time, steps exactly, and a sprint runs a day at once.
pub async fn tick_control(t: T) -> R {
    t.flat().await?;
    let time = async || -> Result<u64, String> {
        let r = t.cmd("/time query").await?;
        r.rsplit("tick ").next().and_then(|v| v.trim_end_matches(')').parse().ok()).ok_or(format!("time: {r}"))
    };
    t.cmd("/tick freeze").await?;
    let a = time().await?;
    t.secs(0.5).await?;
    t.check_eq(time().await?, a, "time while frozen");
    t.cmd("/tick step 10").await?;
    t.secs(1.0).await?;
    t.check_eq(time().await?, a + 10, "time after ten steps");
    t.cmd("/tick unfreeze").await?;
    let b = time().await?;
    t.cmd("/tick sprint 24000").await?;
    t.until("sprint done", Duration::from_secs(60), |t| (0..20).any(|i| t.c().chat_line(i).contains("sprinted"))).await?;
    t.check(time().await? >= b + 24000, "a day went by in the sprint");
    Ok(())
}

/// Frame time with thousands of dropped items in view (item farms).
pub async fn item_farm(t: T) -> R {
    t.flat().await?;
    t.op("drops 3000").await?;
    t.c().look(0.0, -30.0);
    t.secs(2.0).await?;
    t.note(format!("{} entities on the client", t.c().entities().len()));
    t.shot("farm").await?;
    let b = t.bench(4.0, "item_farm").await?;
    t.note(format!("fps {:.0}, frame {:.2} ms, p99 {:.2} ms (in tests the hidden window's presents are throttled: compare the log's cpu record ms)", b[0], b[1], b[2]));
    Ok(())
}

/// Plants grow by world time: wheat ripens when the clock jumps ahead, and sugar cane in an area
/// nobody was in catches up when the player comes back.
pub async fn growth(t: T) -> R {
    t.flat().await?;
    t.op("time 1000").await?;
    let f = t.feet();
    let farm = add(f, [2, -1, 0]);
    t.set(farm, "farmland").await?;
    t.set(add(farm, [0, 1, 0]), "wheat").await?;
    // Sugar cane needs water beside the block it stands on.
    let cane = add(f, [-2, 0, 0]);
    t.set(cane, "sugar_cane").await?;
    t.secs(0.2).await?;
    t.check_eq(t.block(cane).await?, "core:air".to_string(), "sugar cane without water breaks off");
    t.set(add(cane, [0, -1, 1]), "water").await?;
    t.set(cane, "sugar_cane").await?;
    t.op("time 200000").await?;
    t.secs(0.5).await?;
    t.check_eq(t.block(add(farm, [0, 1, 0])).await?, "base:wheat[age=7]".to_string(), "wheat after a jump of 2.5 hours");
    t.check_eq(t.block(add(cane, [0, 2, 0])).await?, "base:sugar_cane".to_string(), "sugar cane after the jump");
    // Far away the cane's area unloads; time passes; coming back, it has grown.
    let cane2 = add(f, [-4, 0, 0]);
    t.set(add(cane2, [0, -1, -1]), "water").await?;
    t.set(cane2, "sugar_cane").await?;
    t.cmd("/tp 3000 10 3000").await?;
    t.secs(3.0).await?;
    t.check_eq(t.block(cane2).await?, "core:air".to_string(), "far away, the cane's area is not here");
    t.op("time 400000").await?;
    t.cmd(&format!("/tp {} {} {}", f[0], f[1], f[2])).await?;
    t.secs(1.0).await?;
    t.loaded().await?;
    t.secs(0.5).await?;
    t.check_eq(t.block(add(cane2, [0, 2, 0])).await?, "base:sugar_cane".to_string(), "sugar cane grown while unloaded");
    Ok(())
}

/// What a player carries over leaving and opening a world again: placed blocks, the inventory,
/// a moving structure with a block added, permanent attribute modifiers; server-only stack data
/// is not saved, and expired modifiers are gone before.
pub async fn persistence(t: T) -> R {
    t.world(7).await?;
    let p = add(t.feet(), [2, 0, 2]);
    t.set(p, "gold_block").await?;
    t.op("put me 3 diamond 5").await?;
    // A moving structure with a block added, in a cube nothing else changed.
    let skiff = t.op(&format!("spawn testmod:skiff {} {} {}", p[0], p[1] + 6, p[2] + 8)).await?;
    let skiff = skiff.trim().trim_start_matches('#').to_string();
    t.op(&format!("sblock #{skiff} 0 1 0 base:glass")).await?;
    t.op("put me 0 iron_sword 1").await?;
    // A machine's contents are kept, and its entity can't be made without the block.
    let chest = add(p, [-3, 0, 0]);
    t.set(chest, "chest").await?;
    t.op(&format!("insert {} {} {} coal 5", chest[0], chest[1], chest[2])).await?;
    t.check_eq(t.op(&format!("spawn base:chest {} {} {}", p[0], p[1] + 2, p[2])).await?, "0".to_string(), "no chest entity without a chest");
    let said = t.cmd("/summon base:furnace").await?;
    t.check(said.contains("belongs to a block"), format!("/summon refuses a furnace: {said}"));
    t.op("secret me 0 1234").await?;
    t.check_eq(t.op("secret me 0").await?, "1234".into(), "server keeps the secret");
    t.frames(5).await?;
    t.check_eq(crate::server::SECRET.stack_only(&t.c().inventory_slot(0)), None, "client never sees the secret");
    t.check_eq(value(&t.op("attr me core:movement_speed").await?), 1.0, "speed without modifiers");
    t.op("attr_mod me core:movement_speed testkit:haste 1 0.5 40").await?;
    t.op("attr_mod me core:max_health testkit:vigor 0 10 0").await?;
    let speed = t.op("attr me core:movement_speed").await?;
    t.check_eq(value(&speed), 1.5, "speed with +50% of base");
    t.check(speed.contains("testkit:haste"), format!("explained: {speed}"));
    t.check_eq(value(&t.op("attr me core:max_health").await?), 30.0, "max health +10");
    t.secs(2.5).await?;
    t.check_eq(value(&t.op("attr me core:movement_speed").await?), 1.0, "the 40 tick modifier ran out");

    let name = format!("test-{}", t.name());
    t.c().leave_world();
    t.screen("core:title").await?;
    t.c().open_world(&name, 7);
    t.until("joined again", Duration::from_secs(60), |t| t.flag(client::client_flags::JOINED)).await?;
    t.loaded().await?;
    t.check_eq(t.block(p).await?, "base:gold_block".into(), "placed block after reload");
    t.check_eq(t.slot("me", 3).await?, ("base:diamond".into(), 5, 0), "inventory after reload");
    t.check_eq(t.op(&format!("sblock #{skiff} 0 1 0")).await?, "base:glass".to_string(), "the structure and its added block after reload");
    t.check_eq(t.slot("me", 0).await?.0, "base:iron_sword".into(), "the sword was saved");
    t.check_eq(t.op(&format!("inv {} {} {}", chest[0], chest[1], chest[2])).await?, "0:base:coal:5".to_string(), "the chest kept its coal");
    t.check_eq(t.op("secret me 0").await?, "none".into(), "the secret was not");
    t.check_eq(value(&t.op("attr me core:max_health").await?), 30.0, "max health modifier after reload");
    // One maximum everywhere: /heal fills to it and the client hears the same.
    t.cmd("/heal").await?;
    t.check_eq(t.op("health").await?, "30".to_string(), "healed to the modified maximum");
    t.until("the client has 30", Duration::from_secs(2), |t| t.state().health == 30.0).await?;
    t.op("attr_rm me core:max_health testkit:vigor").await?;
    t.check_eq(value(&t.op("attr me core:max_health").await?), 20.0, "removed");
    t.sync_inventory().await
}

/// First number of an `attr` answer ("1.5 | ...").
fn value(s: &str) -> f64 {
    s.split('|').next().and_then(|v| v.trim().parse().ok()).unwrap_or(f64::NAN)
}

/// Dimensions: each mod's own one is reached and drawn as itself; the engine's void is empty,
/// without gravity and refuses blocks; the debug dimension shows every state and never updates.
pub async fn dimensions(t: T) -> R {
    t.world(11).await?;
    type Look = Option<fn(&crate::runner::Shot) -> (f32, bool)>;
    let looks: [(&str, Look); 4] = [
        (
            "base:flatlands",
            Some(|s| {
                let v = s.share(0.0, 0.6, 1.0, 0.9, probe::grass);
                (v, v > 0.6)
            }),
        ),
        (
            "testmod:mars",
            Some(|s| {
                let v = s.share(0.0, 0.0, 1.0, 0.9, |c| c[0] as i32 > c[1] as i32 + 40 && c[0] as i32 > c[2] as i32 + 60);
                (v, v > 0.6)
            }),
        ),
        (
            "testmod:void",
            Some(|s| {
                let v = s.share(0.0, 0.0, 1.0, 0.25, probe::dark);
                (v, v > 0.9)
            }),
        ),
        // Generated terrain: reaching it is the check, the picture is for looking at.
        ("base:overworld", None),
    ];
    for (dim, look) in looks {
        let Some(id) = Dimension::lookup(dim) else {
            t.fail(format!("no dimension {dim}"));
            continue;
        };
        t.cmd(&format!("/dim {dim}")).await?;
        t.until(&format!("in {dim}"), Duration::from_secs(20), |t| t.state().dim == id).await?;
        t.loaded().await?;
        t.idle().await?;
        t.settled().await?;
        t.c().look(30.0, -20.0);
        t.frames(3).await?;
        let shot = t.shot(dim.split(':').nth(1).unwrap_or(dim)).await?;
        if let Some(look) = look {
            let (v, ok) = look(&shot);
            t.probe(&format!("{dim} drawn as itself"), v, ok);
        }
        if dim == "testmod:void" {
            // testmod's island generator places a chest (`ISLAND_CHEST`) without its entity.
            t.check_eq(t.op("insert 4 64 0 coal 3").await, Ok("3".to_string()), "a chest placed by a generator has its block entity");
        }
    }

    let void = Dimension::lookup("core:void").ok_or("no core:void")?;
    t.cmd("/dim core:void").await?;
    t.until("in the void", Duration::from_secs(20), |t| t.state().dim == void).await?;
    t.loaded().await?;
    let said = (0..4).any(|i| t.c().chat_line(i).contains("nothing here can be changed"));
    t.check(said, "chat says where the player is");
    let p = add(t.feet(), [0, -1, 0]);
    t.check(t.set(p, "stone").await.is_err(), "no block can be set in the void");
    t.check(t.block(p).await? != "base:stone", "the void stays empty");
    let y = t.state().pos[1];
    t.secs(1.0).await?;
    t.check((t.state().pos[1] - y).abs() < 0.01, format!("no gravity in the void (y {y:.2} -> {:.2})", t.state().pos[1]));

    t.cmd("/dim core:debug_blocks").await?;
    t.secs(0.5).await?;
    t.loaded().await?;
    let at = t.op("find 40 base:water").await?;
    let w: Vec<i32> = at.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    let [x, y, z] = w[..] else { return Err(format!("no water among the debug blocks: {at}")) };
    t.secs(1.0).await?;
    t.check_eq(t.block([x + 1, y, z]).await?, "core:air".to_string(), "debug water does not flow");
    t.cmd(&format!("/setblock {} {y} {z} base:stone", x + 1)).await?;
    t.check_eq(t.block([x + 1, y, z]).await?, "core:air".to_string(), "debug blocks refuse /setblock");
    t.cmd("/tp -3 1 -3").await?;
    t.c().look(225.0, -25.0);
    t.frames(20).await?;
    let grid = t.shot("debug_blocks").await?;
    // Many different blocks in view: no color class covers most of the floor.
    let biggest = [probe::grass, probe::dark, probe::light, probe::brown].map(|f| grid.share(0.0, 0.5, 1.0, 0.9, f)).into_iter().fold(0.0f32, f32::max);
    t.probe("debug blocks: largest share of one color class", biggest, biggest < 0.6);
    Ok(())
}

/// World generation from data, sampled with seed 5: deep caves (an underground biome) with the
/// test mod's glow crystals only there, birches in birch forests and oaks on the plains, and
/// structures where the engine says they are.
pub async fn worldgen(t: T) -> R {
    t.world(5).await?;
    t.op("mode spectator").await?;
    let (mut caves, mut inside, mut outside) = (0, 0, 0);
    for (i, x) in [0, 200].into_iter().enumerate() {
        t.cmd(&format!("/tp {x} -60 {}", i * 150)).await?;
        t.secs(0.5).await?;
        t.loaded().await?;
        let survey = t.op("survey 12").await?;
        let (biomes, crystals) = survey.split_once(" | crystals ").ok_or(format!("survey: {survey}"))?;
        caves += biomes.split_whitespace().filter_map(|e| e.strip_prefix("base:deep_caves=")).filter_map(|n| n.parse::<u32>().ok()).sum::<u32>();
        let n: Vec<u32> = crystals.split_whitespace().filter_map(|n| n.parse().ok()).collect();
        inside += n.first().copied().unwrap_or(0);
        outside += n.get(1).copied().unwrap_or(0);
        t.note(format!("{x}: {survey}"));
    }
    t.check(caves > 0, "deep caves were found underground");
    t.check(inside > 0, format!("glow crystals grow in deep caves ({inside})"));
    t.check_eq(outside, 0, "glow crystals outside deep caves");

    // Birch forests of seed 5 (found from the base generator's columns), then plains.
    for ([x, y, z], forest) in [([100, 83, 0], true), ([-550, 87, 550], true), ([0, 90, 0], false), ([-400, 90, -400], false)] {
        t.cmd(&format!("/tp {x} {y} {z}")).await?;
        t.secs(0.5).await?;
        t.loaded().await?;
        let census = t.op("census 28 base:birch_log,base:oak_log,testmod:glow_log,testmod:glow_flower").await?;
        let n: Vec<u32> = census.split_whitespace().filter_map(|e| e.rsplit_once('=')?.1.parse().ok()).collect();
        t.note(format!("{x} {z}: {census}"));
        if forest {
            t.check(n[0] > 0, format!("birches in the birch forest at {x} {z}"));
        } else {
            t.check(n[1] > 0, format!("oaks on the plains at {x} {z}"));
        }
        t.check_eq(n[2] + n[3], 0, &format!("the test mod's glowing plants are not generated ({x} {z})"));
    }

    t.check(t.op("structure testmod:obelisk 3000").await? != "none", "another mod's obelisks are in the world too");

    // Testmod puts base's forest on Mars; the engine scatters its trees there like at home.
    static MARS: Dimension = Dimension::named("testmod:mars");
    let seed = 5;
    let forest = |x: i32, z: i32| name_of(ek::BIOME, MARS.column(seed, x, z).biome) == "base:forest";
    let spot = (-60..60).flat_map(|i| (-60..60).map(move |j| (i * 32, j * 32))).find(|&(x, z)| [(0, 0), (20, 0), (-20, 0), (0, 20), (0, -20)].iter().all(|&(dx, dz)| forest(x + dx, z + dz)));
    let (x, z) = spot.ok_or("no forest on Mars")?;
    let h = MARS.column(seed, x, z).height;
    t.cmd("/dim testmod:mars").await?;
    t.until("on Mars", Duration::from_secs(20), |t| t.state().dim == MARS.id()).await?;
    t.cmd(&format!("/tp {x} {} {z}", h + 3)).await?;
    t.secs(0.5).await?;
    t.loaded().await?;
    let census = t.op("census 20 base:oak_log,base:birch_log,base:grass").await?;
    t.note(format!("Mars forest at {x} {z}: {census}"));
    let n: Vec<u32> = census.split_whitespace().filter_map(|e| e.rsplit_once('=')?.1.parse().ok()).collect();
    t.check(n[0] + n[1] > 0, "trees grow in base's forest on Mars");
    t.check(n[2] > 0, "the forest brings its grass to Mars");
    Ok(())
}

