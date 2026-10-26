//! Machine progress reaches the client through menu-synced components.

use std::time::Duration;


use super::add;
use crate::runner::{R, T};

use base_api::SMELTING_STATE as SMELTING;

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

