//! What reaches the client: machine progress through menu-synced components, components of
//! other entities for everyone who sees them, and stack data kept on the server.

use std::time::Duration;

use voxide_sdk::*;

use crate::runner::{R, T};
use crate::server::{MARK, SECRET};

value! {
    struct Smelting { burn: u32, burn_max: u32, cook: u32, cook_max: u32 }
}
static SMELTING: Component<Smelting> = Component::new("base:smelting");

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

pub async fn menu_sync(t: T) -> R {
    t.flat().await?;
    let p = add(t.feet(), [2, 0, -2]);
    t.set(p, "furnace").await?;
    t.op(&format!("insert {} {} {} raw_gold 2", p[0], p[1], p[2])).await?;
    t.op(&format!("insert {} {} {} coal 1", p[0], p[1], p[2])).await?;
    t.op(&format!("menu base:furnace {} {} {}", p[0], p[1], p[2])).await?;
    t.screen("core:menu").await?;
    t.until("furnace progress on the client", Duration::from_secs(5), |t| t.c().view().menu_component(&SMELTING).is_some_and(|s| s.cook > 0)).await?;
    let first = t.c().view().menu_component(&SMELTING).map_or(0, |s| s.cook);
    t.secs(1.0).await?;
    let later = t.c().view().menu_component(&SMELTING).map_or(0, |s| s.cook);
    t.check(later > first + 10, format!("cook progress {first} -> {later} in a second (20 ticks)"));
    t.check(t.c().view().menu_component(&SMELTING).is_some_and(|s| s.burn_max == 1600), "coal burn time synced");
    t.shot("progress").await?;
    t.c().close_screen();
    t.no_screen().await
}

pub async fn watcher_sync(t: T) -> R {
    t.flat().await?;
    let f = t.feet();
    let pig: u64 = t.op(&format!("spawn pig {} {} {}", f[0] + 3, f[1], f[2] + 3)).await?.parse().map_err(|_| "no pig".to_string())?;
    t.until("pig known", Duration::from_secs(5), |t| t.c().entities().contains(&pig)).await?;
    t.op(&format!("mark #{pig} 77")).await?;
    t.until("mark on the client", Duration::from_secs(5), |t| t.c().view().component_of(pig, &MARK) == Some(77)).await?;
    t.op(&format!("mark #{pig} 78")).await?;
    t.until("changed mark on the client", Duration::from_secs(5), |t| t.c().view().component_of(pig, &MARK) == Some(78)).await?;
    // The test kit's system counts marks up once a tick while the rule is on.
    t.cmd("/rule testkit:count_marks 1").await?;
    t.secs(1.5).await?;
    t.cmd("/rule testkit:count_marks 0").await?;
    let now = t.c().view().component_of(pig, &MARK).unwrap_or(0);
    t.check((100..=120).contains(&now), format!("mark after 1.5 s of a counting system: {now} (78 + about 30 ticks)"));
    Ok(())
}

pub async fn server_only_stack_data(t: T) -> R {
    t.flat().await?;
    t.op("put me 0 iron_sword 1").await?;
    t.op("secret me 0 1234").await?;
    t.check_eq(t.op("secret me 0").await?, "1234".into(), "server keeps the secret");
    t.frames(5).await?;
    let s = t.c().inventory_slot(0);
    t.check_eq(s.item, find_item("base:iron_sword").unwrap_or(u32::MAX), "client has the sword");
    t.check_eq(SECRET.stack_only(&s), None, "client never sees the secret");
    let name = format!("test-{}", t.name());
    t.c().leave_world();
    t.screen("core:title").await?;
    t.c().open_world(&name, 1);
    t.until("joined again", Duration::from_secs(60), |t| t.flag(client::client_flags::JOINED)).await?;
    t.loaded().await?;
    t.check_eq(t.slot("me", 0).await?.0, "base:iron_sword".into(), "the sword was saved");
    t.check_eq(t.op("secret me 0").await?, "none".into(), "the secret was not");
    Ok(())
}
