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
    let diamond = find_item("base:diamond").unwrap_or(0);
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
