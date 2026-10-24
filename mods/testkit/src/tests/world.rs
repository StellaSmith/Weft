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
