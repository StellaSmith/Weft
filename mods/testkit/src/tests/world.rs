use std::time::Duration;

use voxide_sdk::*;

use crate::runner::{R, T};

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

/// Edits on the server reach the client block for block; entities show up on both sides.
pub async fn flatlands_sync(t: T) -> R {
    t.flat().await?;
    let o = add(t.feet(), [3, 0, 3]);
    t.fill(o, add(o, [4, 2, 4]), "cobblestone").await?;
    t.fill(add(o, [1, 1, 1]), add(o, [3, 1, 3]), "glass").await?;
    t.set(add(o, [2, 2, 2]), "furnace#5").await?;
    t.set(add(o, [0, 2, 0]), "air").await?;
    t.frames(5).await?;
    t.sync_blocks(o, add(o, [4, 2, 4])).await?;
    let pig = t.op(&format!("spawn pig {} {} {}", o[0] - 6, o[1], o[2])).await?;
    let pig: u64 = pig.parse().map_err(|_| format!("spawn gave '{pig}'"))?;
    t.until("client sees the pig", Duration::from_secs(5), |t| t.c().entities().contains(&pig)).await?;
    t.frames(20).await?;
    let server = t.op(&format!("entity {pig}")).await?;
    let sp: Vec<f64> = server.split_whitespace().skip(1).take(3).filter_map(|v| v.parse().ok()).collect();
    if let (Some(e), [x, y, z]) = (t.c().entity(pig), sp.as_slice()) {
        let d = ((e.pos[0] - x).powi(2) + (e.pos[1] - y).powi(2) + (e.pos[2] - z).powi(2)).sqrt();
        t.check(d < 1.5, format!("pig is {d:.2} blocks off between server and client"));
    }
    Ok(())
}

pub async fn save_and_reload(t: T) -> R {
    t.world(7).await?;
    let p = add(t.feet(), [2, 0, 2]);
    t.set(p, "gold_block").await?;
    t.op("put me 3 diamond 5").await?;
    let name = format!("test-{}", t.name());
    t.c().leave_world();
    t.screen("core:title").await?;
    t.c().open_world(&name, 7);
    t.until("joined again", Duration::from_secs(60), |t| t.flag(client::client_flags::JOINED)).await?;
    t.loaded().await?;
    t.check_eq(t.block(p).await?, "base:gold_block".into(), "placed block after reload");
    t.check_eq(t.slot("me", 3).await?, ("base:diamond".into(), 5, 0), "inventory after reload");
    t.sync_inventory().await
}

/// A player saved in a dimension that is gone (its mod removed) keeps everything and starts at
/// the spawn; the world remembers the mods it was played with.
pub async fn missing_dimension(t: T) -> R {
    t.world(7).await?;
    t.op("put me 3 diamond 5").await?;
    let flat = find_dimension("base:flatlands").ok_or("no flatlands")?;
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
    t.c().open_screen("core:worlds");
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
    t.check_eq(t.state().dim, find_dimension("base:overworld").unwrap_or(u32::MAX), "the player is in the spawn dimension");
    t.check_eq(t.slot("me", 3).await?, ("base:diamond".into(), 5, 0), "and kept the inventory");
    Ok(())
}

/// The engine's void is always there: empty, without gravity, nothing can be placed in it.
pub async fn void_dimension(t: T) -> R {
    t.world(3).await?;
    let void = find_dimension("core:void").ok_or("no core:void")?;
    t.cmd("/dim core:void").await?;
    t.until("in the void", Duration::from_secs(20), |t| t.state().dim == void).await?;
    t.loaded().await?;
    let said = (0..4).any(|i| t.c().chat_line(i).contains("nothing here can be changed"));
    t.check(said, "chat says where the player is");
    let p = add(t.feet(), [0, -1, 0]);
    t.check(t.set(p, "stone").await.is_err(), "no block can be set");
    t.check(t.block(p).await? != "base:stone", "the block stays empty");
    let y = t.state().pos[1];
    t.secs(1.0).await?;
    t.check((t.state().pos[1] - y).abs() < 0.01, format!("no gravity (y {y:.2} -> {:.2})", t.state().pos[1]));
    Ok(())
}

pub async fn dimensions(t: T) -> R {
    t.world(11).await?;
    for (dim, shot) in [("base:flatlands", "flatlands"), ("testmod:mars", "mars"), ("testmod:void", "void"), ("base:overworld", "overworld")] {
        let Some(id) = find_dimension(dim) else {
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
        t.shot(shot).await?;
    }
    Ok(())
}

/// Menus open on virtual inventories and on far away blocks; a menu for a block without an
/// entity is refused with a reason instead of opening something broken.
pub async fn virtual_menus(t: T) -> R {
    t.flat().await?;
    let id = t.op("vmenu base:chest 27 diamond 3 stick 10").await?;
    t.screen("core:menu").await?;
    let diamond = find_item("base:diamond").ok_or("no item base:diamond")?;
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
        let want = find_menu(menu).unwrap_or(u32::MAX);
        t.until(&format!("{menu} open"), Duration::from_secs(5), |t| t.c().menu().is_some_and(|m| m[0] == want)).await?;
        t.screen("core:menu").await?;
    }
    t.c().close_screen();
    t.no_screen().await
}

/// Sand falls, a torch drops when its block goes.
pub async fn sand_and_support(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let air = add(f, [3, 4, -2]);
    t.set(air, "sand").await?;
    let mut falling = false;
    for _ in 0..20 {
        falling |= t.count_near("falling_block", 8.0).await? == 1;
        t.frames(2).await?;
    }
    t.check(falling, "the sand falls as an entity");
    t.secs(2.0).await?;
    t.check_eq(t.block(air).await?, "core:air".into(), "sand left the air");
    t.check_eq(t.block(add(f, [3, 0, -2])).await?, "base:sand".into(), "sand landed on the ground");
    let stone = add(f, [-3, 0, 2]);
    t.set(stone, "stone").await?;
    t.set(add(stone, [0, 1, 0]), "torch").await?;
    t.set(stone, "air").await?;
    t.frames(5).await?;
    t.check_eq(t.block(add(stone, [0, 1, 0])).await?, "core:air".into(), "torch without support");
    let items = t.count_near("item", 6.0).await?;
    t.check(items >= 1, "the torch dropped no item");
    Ok(())
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

/// The engine's debug dimension shows every block state on a floor and nothing in it updates:
/// its water does not flow and blocks cannot be set.
pub async fn debug_blocks(t: T) -> R {
    t.flat().await?;
    t.cmd("/dim core:debug_blocks").await?;
    t.secs(0.5).await?;
    t.loaded().await?;
    let at = t.op("find 40 base:water").await?;
    let p: Vec<i32> = at.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    t.secs(1.0).await?;
    t.check_eq(t.block([p[0] + 1, p[1], p[2]]).await?, "core:air".to_string(), "water next to the source");
    t.cmd(&format!("/setblock {} {} {} base:stone", p[0] + 1, p[1], p[2])).await?;
    t.check_eq(t.block([p[0] + 1, p[1], p[2]]).await?, "core:air".to_string(), "after /setblock");
    t.cmd("/tp -3 1 -3").await?;
    t.c().look(225.0, -25.0);
    t.frames(20).await?;
    t.shot_within("grid", 0.03).await?;
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
    // Dropped items spin and bob with their age: only a rough match.
    t.shot_within("farm", 0.3).await?;
    let b = t.bench(4.0, "item_farm").await?;
    t.note(format!("fps {:.0}, frame {:.2} ms, p99 {:.2} ms (in tests the hidden window's presents are throttled: compare the log's cpu record ms)", b[0], b[1], b[2]));
    Ok(())
}

/// Underground (3D) biomes: deep under the surface whole cubes turn into deep caves, and the
/// test mod's glow crystals (an ore limited to the `cave` biome tag) grow only there.
pub async fn deep_caves(t: T) -> R {
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
    Ok(())
}

/// Trees and flowers are data: birch forests grow the base birches; the test mod's glowing birch
/// and flower are not generated.
pub async fn flora(t: T) -> R {
    t.world(5).await?;
    t.op("mode spectator").await?;
    let blocks = "testmod:glow_log,testmod:glow_flower,base:birch_log,base:oak_log";
    let (mut glow_in, mut glow_out) = (0, 0);
    // Birch forests of seed 5 (found from the base generator's columns), and plains.
    for [x, y, z] in [[100, 83, 0], [-550, 87, 550], [0, 84, -900], [0, 90, 0], [-400, 90, -400]] {
        t.cmd(&format!("/tp {x} {y} {z}")).await?;
        t.secs(0.5).await?;
        t.loaded().await?;
        let survey = t.op("survey 2").await?;
        let birch = survey.contains("base:birch_forest");
        let census = t.op(&format!("census 28 {blocks}")).await?;
        t.note(format!("{x} {z}: {} | {census}", survey.split(" | ").next().unwrap_or("")));
        let n: Vec<u32> = census.split_whitespace().filter_map(|e| e.rsplit_once('=')?.1.parse().ok()).collect();
        let glow = n[0] + n[1];
        if birch { glow_in += glow } else if !survey.contains("birch") { glow_out += glow }
        if x == 100 {
            t.c().look(0.0, 20.0);
            t.frames(30).await?;
            t.shot_within("birch_forest", 0.03).await?;
        }
    }
    // The test mod's glowing birch and flower are not generated.
    t.check_eq(glow_in + glow_out, 0, "no glowing plants in the world");
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
    t.check_eq(t.block(add(farm, [0, 1, 0])).await?, "base:wheat#7".to_string(), "wheat after a jump of 2.5 hours");
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

/// Structures: a well stands where the engine says one is, built as its template says.
pub async fn structures(t: T) -> R {
    t.world(5).await?;
    t.op("mode spectator").await?;
    let at = t.op("structure base:well 2000").await?;
    let a: Vec<i32> = at.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    let [x, y, z] = a[..] else { return Err(format!("no well near spawn: {at}")) };
    t.cmd(&format!("/tp {} {} {}", x + 1, y + 6, z - 3)).await?;
    t.secs(1.0).await?;
    t.loaded().await?;
    t.secs(0.5).await?;
    t.check_eq(t.block([x, y, z]).await?, "base:cobblestone".to_string(), "the well's rim");
    t.check_eq(t.block([x + 1, y, z + 1]).await?, "base:water".to_string(), "its water");
    t.check_eq(t.block([x, y + 3, z]).await?, "base:cobblestone".to_string(), "its roof");
    t.check(t.op("structure testmod:obelisk 3000").await? != "none", "another mod's obelisks are in the world too");
    t.c().look(180.0, -40.0);
    t.secs(0.5).await?;
    t.shot("well").await?;
    Ok(())
}
