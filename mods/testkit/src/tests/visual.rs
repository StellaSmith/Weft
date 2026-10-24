//! Scenarios that are mostly about the picture, checked against reference screenshots, plus
//! multiplayer through a second game process.

use std::time::Duration;

use voxide_sdk::*;

use crate::runner::{R, T};

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

fn item(name: &str) -> u32 {
    find_item(name).unwrap_or(u32::MAX)
}

pub async fn held_items(t: T) -> R {
    t.flat().await?;
    for (i, name) in ["wood_pickaxe", "diamond_sword", "glass", "rose", "torch"].iter().enumerate() {
        t.op(&format!("put me {i} {name} 1")).await?;
    }
    t.c().look(0.0, -10.0);
    for (i, name) in ["pickaxe", "sword", "block", "flower", "torch"].iter().enumerate() {
        t.c().select(i as u32);
        t.until("selected", Duration::from_secs(5), |t| t.state().selected == i as u32).await?;
        // The hand lowers and comes back up on a switch.
        t.secs(0.6).await?;
        t.shot(name).await?;
    }
    Ok(())
}

pub async fn tooltips(t: T) -> R {
    t.flat().await?;
    t.op("put me 0 furnace 1").await?;
    t.op("put me 1 coal 5").await?;
    t.press("core:inventory");
    t.screen("core:menu").await?;
    for (slot, name) in [(36, "furnace"), (37, "coal")] {
        let Some([x, y, w, h]) = t.c().ui_rect(&format!("slot:{slot}")) else {
            t.fail(format!("no slot {slot}"));
            continue;
        };
        t.c().cursor(x + w / 2.0, y + h / 2.0);
        t.frames(4).await?;
        t.shot(name).await?;
    }
    t.c().close_screen();
    t.no_screen().await
}

pub async fn night_light(t: T) -> R {
    t.world(9).await?;
    t.idle().await?;
    t.op("time 18000").await?;
    let f = t.feet();
    t.set(add(f, [2, 0, -3]), "glowstone").await?;
    t.set(add(f, [-3, 0, -4]), "red_lamp").await?;
    t.settled().await?;
    t.c().look(0.0, -25.0);
    t.secs(0.5).await?;
    t.shot("night").await
}

pub async fn third_person(t: T) -> R {
    t.flat().await?;
    t.op("put me 0 iron_sword 1").await?;
    t.c().select(0);
    t.press("core:perspective");
    t.c().look(30.0, -15.0);
    t.secs(0.5).await?;
    t.shot("behind").await?;
    t.press("core:perspective");
    t.secs(0.3).await?;
    t.shot("front").await?;
    t.press("core:perspective");
    Ok(())
}

/// Crouching shows in third person: body leaning, legs under it, lower (vanilla's pose).
pub async fn crouch_pose(t: T) -> R {
    t.flat().await?;
    t.press("core:perspective");
    t.c().look(30.0, -15.0);
    t.secs(0.3).await?;
    t.press("core:perspective");
    t.secs(0.3).await?;
    t.hold("core:sneak", 2.0);
    t.secs(1.0).await?;
    t.shot("crouching").await?;
    t.secs(1.2).await?;
    t.press("core:perspective");
    Ok(())
}

/// A ladder takes the facing of the wall it was put on.
pub async fn ladder_placement(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let wall = add(f, [0, 0, -2]);
    t.fill(wall, add(wall, [0, 1, 0]), "cobblestone").await?;
    t.op("put me 0 ladder 8").await?;
    t.c().select(0);
    t.look_at(wall);
    t.frames(3).await?;
    t.press("core:use");
    let at = add(wall, [0, 0, 1]);
    t.until("ladder placed", Duration::from_secs(5), |t| t.c().inventory_slot(0).count == 7).await?;
    // State order follows the wall: -Z, +X, +Z, -X; this one hangs on a wall to its north.
    t.check_eq(t.block(at).await?, "base:ladder".into(), "ladder on the north wall");
    Ok(())
}

/// A torch put against a wall hangs on it, and drops when the wall goes.
pub async fn wall_torch(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let wall = add(f, [0, 0, -2]);
    t.fill(wall, add(wall, [0, 1, 0]), "cobblestone").await?;
    t.op("put me 0 torch 8").await?;
    t.c().select(0);
    t.look_at(wall);
    t.frames(3).await?;
    t.press("core:use");
    let at = add(wall, [0, 0, 1]);
    t.until("torch placed", Duration::from_secs(5), |t| t.c().inventory_slot(0).count == 7).await?;
    t.check_eq(t.block(at).await?, "base:torch#1".into(), "torch on the north wall");
    t.frames(5).await?;
    t.shot("wall_torch").await?;
    t.set(wall, "air").await?;
    t.secs(0.3).await?;
    t.check_eq(t.block(at).await?, "core:air".into(), "the torch fell with its wall");
    Ok(())
}

pub async fn pick_block(t: T) -> R {
    t.flat().await?;
    t.op("mode creative").await?;
    t.until("creative", Duration::from_secs(5), |t| t.state().mode == 1).await?;
    let f = t.feet();
    let b = add(f, [0, -1, -1]);
    t.set(b, "bricks").await?;
    t.look_at(b);
    t.frames(3).await?;
    t.c().click(2, 0.0);
    t.until("bricks picked", Duration::from_secs(5), |t| t.c().inventory_slot(t.state().selected).item == item("base:bricks")).await
}

pub async fn gizmos(t: T) -> R {
    t.flat().await?;
    t.op(&format!("spawn pig {} {} {}", t.feet()[0] + 3, t.feet()[1], t.feet()[2] - 4)).await?;
    t.cmd("/gizmo cubes").await?;
    t.cmd("/gizmo light").await?;
    t.c().look(30.0, -30.0);
    t.secs(0.5).await?;
    t.shot("cubes_light").await?;
    t.cmd("/gizmo cubes").await?;
    t.cmd("/gizmo light").await?;
    Ok(())
}

/// Hosts the test world and lets a second game process join it over the network transport.
pub async fn multiplayer(t: T) -> R {
    t.world(13).await?;
    t.c().host_world();
    t.until("hosting", Duration::from_secs(60), |t| t.flag(client::client_flags::HOSTING)).await?;
    let run = t.c().run_dir();
    let dir = format!("{run}/test-mp");
    let _ = std::fs::create_dir_all(&dir);
    let ticket = format!("{dir}/ticket.txt");
    let script = format!("{dir}/join.txt");
    std::fs::write(&ticket, t.c().ticket()).map_err(|e| e.to_string())?;
    std::fs::write(&script, format!("join {ticket}\nloaded\nchat hello from the second player\nwait 3\nquit\n")).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(exe)
        .args(["--run-dir", &dir, "--script", &script, "--size", "640x360"])
        .spawn()
        .map_err(|e| format!("cannot start the second client: {e}"))?;
    let heard = t
        .until("the second player's chat", Duration::from_secs(90), |t| (0..5).any(|i| t.c().chat_line(i).contains("hello from the second player")))
        .await;
    let seen = t.c().entities().iter().filter_map(|&id| t.c().entity(id)).any(|e| name_of(abi::entry_kind::ENTITY, e.kind) == "core:player");
    if heard.is_err() {
        let _ = child.kill();
    }
    let mut status = None;
    let _ = t
        .until("the second client to quit", Duration::from_secs(30), |_| {
            status = child.try_wait().ok().flatten();
            status.is_some()
        })
        .await;
    if status.is_none() {
        let _ = child.kill();
    }
    heard?;
    t.check(seen, "the host does not see the second player");
    t.check(status.is_some_and(|s| s.success()), format!("second client ended with {status:?}"));
    Ok(())
}

/// A mod key binding is listed in Controls and reaches the mod; a frame hook draws in the world
/// and on the screen.
pub async fn client_hooks(t: T) -> R {
    t.flat().await?;
    let before = crate::runner::PROBES.with(|p| p.get());
    t.c().key("F9", 0.0);
    t.until("probe key seen", Duration::from_secs(3), |_| crate::runner::PROBES.with(|p| p.get()) > before).await?;
    t.c().open_screen("core:controls");
    t.screen("core:controls").await?;
    for _ in 0..40 {
        if t.c().ui_rect("text:test probe").is_some() {
            break;
        }
        t.c().cursor(0.5, 0.5);
        t.c().scroll(-3.0);
        t.frames(2).await?;
    }
    t.check(t.c().ui_rect("text:test probe").is_some(), "Controls does not list the mod's key");
    t.c().close_screen();
    t.no_screen().await?;
    let f = t.feet();
    let b = [f[0] as f64 + 2.0, f[1] as f64, f[2] as f64 - 4.0];
    t.c().look(0.0, -10.0);
    for _ in 0..3 {
        t.c().draw_box(b, [b[0] + 1.0, b[1] + 2.0, b[2] + 1.0], 0xFF4040FF);
        t.c().draw_text([b[0] + 0.5, b[1] + 2.5, b[2] + 0.5], "drawn by testkit", 0xFFFF80FF);
        t.c().screen_text(0.5, 0.3, "screen text from a frame hook", 0x80FFFFFF);
        t.frames(1).await?;
    }
    t.c().draw_box(b, [b[0] + 1.0, b[1] + 2.0, b[2] + 1.0], 0xFF4040FF);
    t.c().draw_text([b[0] + 0.5, b[1] + 2.5, b[2] + 0.5], "drawn by testkit", 0xFFFF80FF);
    t.c().screen_text(0.5, 0.3, "screen text from a frame hook", 0x80FFFFFF);
    t.shot("drawn").await
}
