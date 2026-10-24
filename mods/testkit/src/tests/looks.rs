//! Stage 6 and 7: particles, text effects and conditions, shaders from mods, glow masks, models.

use std::time::Duration;

use voxide_sdk::*;

use crate::runner::{R, T};

static SPARK: Particle = Particle::named("testmod:spark");

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

/// Bursts from the client and from the server (a broken block's dust) live and die out.
pub async fn particles(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let pos = [f[0] as f64 + 0.5, f[1] as f64 + 1.5, f[2] as f64 - 2.5];
    drop(t.c().particles(&SPARK, pos).count(200).spread(0.5).speed(1.0));
    t.until("client particles", Duration::from_secs(2), |t| t.state().particles >= 200).await?;
    // Sparks live at most 1.2 s.
    t.until("particles gone", Duration::from_secs(4), |t| t.state().particles == 0).await?;
    let b = add(f, [0, 0, -2]);
    t.set(b, "dirt").await?;
    t.op(&format!("break {} {} {}", b[0], b[1], b[2])).await?;
    // Vanilla's 4x4x4 pieces, sent by the server.
    t.until("block dust", Duration::from_secs(3), |t| t.state().particles >= 60).await?;
    t.until("dust settled", Duration::from_secs(4), |t| t.state().particles == 0).await?;
    // Huge counts stay cheap: simulated in parallel, drawn in one instanced draw.
    for _ in 0..10 {
        drop(t.c().particles(&SPARK, pos).count(10_000).spread(4.0).speed(1.0));
    }
    t.frames(2).await?;
    let n = t.state().particles;
    let b = t.bench(0.6, "100k particles").await?;
    t.note(format!("{n} particles: {:.0} fps, frame {:.2} ms, gpu {:.2} ms", b[0], b[1], b[3]));
    t.check(n >= 99_000 && b[1] < 33.0, format!("{n} particles in {:.1} ms frames", b[1]));
    Ok(())
}

/// The test mod's looks in one picture: a glow crystal at night (veins glowing, no light),
/// wisps drawn by an entity shader, sparks, the rune's reach box and the daydream screen effect.
pub async fn shaders_and_glow(t: T) -> R {
    t.world(9).await?;
    t.idle().await?;
    t.op("time 18000").await?;
    let f = t.feet();
    // A level stage of its own, so terrain changes do not move the picture.
    t.fill(add(f, [-4, -1, -7]), add(f, [4, -1, 3]), "stone").await?;
    t.fill(add(f, [-4, 0, -7]), add(f, [4, 5, 3]), "air").await?;
    t.fill(add(f, [-1, 0, -4]), add(f, [1, 2, -4]), "testmod:glow_crystal").await?;
    // Off to the side and back a little: it drifts, so it stays small in the picture.
    t.op(&format!("spawn testmod:wisp {} {} {}", f[0] + 3, f[1] + 1, f[2] - 5)).await?;
    t.check(t.count_near("wisp", 8.0).await? == 1, "a wisp floats nearby");
    t.op("put me 0 testmod:rune 1").await?;
    t.c().select(0);
    t.c().look(0.0, -5.0);
    t.settled().await?;
    t.secs(1.0).await?;
    t.shot("night_glow").await?;
    t.check(t.c().post_effect("testmod:dream", 1.0), "the daydream effect exists");
    t.check(!t.c().post_effect("testmod:nonexistent", 1.0), "an unknown effect is refused");
    t.frames(3).await?;
    t.shot("dream").await?;
    t.c().post_effect("testmod:dream", 0.0);
    Ok(())
}

/// Models: the orb (built in code, a sphere mesh and a child bone) turns by its file's idle
/// animation; the idol (a Blockbench file with bones and mesh wings) bows when used, a server
/// triggered animation that holds its last frame.
pub async fn models(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    t.op(&format!("spawn testmod:orb {} {} {}", f[0], f[1] + 1, f[2] - 3)).await?;
    t.op(&format!("spawn testmod:idol {} {} {}", f[0], f[1], f[2] + 3)).await?;
    t.check(t.count_near("orb", 8.0).await? == 1 && t.count_near("idol", 8.0).await? == 1, "an orb and an idol stand nearby");
    t.c().look(0.0, 0.0);
    t.settled().await?;
    t.secs(0.5).await?;
    let a = t.grab().await?;
    t.secs(0.5).await?;
    let b = t.grab().await?;
    t.check(T::differ(&a, &b) > 0.001, "the orb's ring turns by its keyframes");
    t.look_at(add(f, [0, 0, 3]));
    t.frames(3).await?;
    t.press("core:use");
    t.secs(1.5).await?;
    let a = t.grab().await?;
    t.secs(0.3).await?;
    let b = t.grab().await?;
    t.check(T::differ(&a, &b) == 0.0, "the bow holds its last frame");
    t.shot("idol_bow").await?;
    // The test mod gives the player a wave: it moves the first person hand by its `hand` bone.
    t.c().look(90.0, 0.0);
    t.frames(3).await?;
    let a = t.grab().await?;
    t.op("animate me wave").await?;
    t.secs(0.5).await?;
    let b = t.grab().await?;
    t.check(T::differ(&a, &b) > 0.002, "the first person hand waves");
    Ok(())
}

/// Tooltip parts behind `{on_key}` and `{if}` appear while their condition holds; effects run.
pub async fn rich_text(t: T) -> R {
    t.flat().await?;
    t.op("put me 0 testmod:rune 1").await?;
    // A fall hurts the player, which the test mod's `{if:testmod:hurt}` shows.
    let s = t.state();
    t.c().teleport([s.pos[0], s.pos[1] + 6.0, s.pos[2]]);
    t.until("hurt by the fall", Duration::from_secs(5), |t| t.state().health < 20.0).await?;
    t.press("core:inventory");
    t.screen("core:menu").await?;
    let Some([x, y, w, h]) = t.c().ui_rect("slot:36") else { return Err("no hotbar slot in the inventory".into()) };
    t.c().cursor(x + w / 2.0, y + h / 2.0);
    t.frames(4).await?;
    t.shot("rune").await?;
    t.hold("core:sneak", 2.0);
    t.frames(4).await?;
    t.shot("rune_sneak").await?;
    t.c().release_all();
    t.c().close_screen();
    t.no_screen().await
}

/// The machine screen's panel is drawn by the test mod's GUI shader.
pub async fn gui_shader(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let p = add(f, [0, 0, -2]);
    t.set(p, "testmod:generator").await?;
    t.op(&format!("menu testmod:generator {} {} {}", p[0], p[1], p[2])).await?;
    t.screen("core:menu").await?;
    t.frames(3).await?;
    t.shot("generator").await?;
    t.c().close_screen();
    t.no_screen().await
}
