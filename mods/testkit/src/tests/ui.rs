use std::time::Duration;

use voxide_sdk::*;

use crate::runner::{R, T};

/// Main menu screens open, lay out and look like their references.
pub async fn title_screens(t: T) -> R {
    t.screen("core:title").await?;
    t.shot("title").await?;
    for name in ["core:settings", "core:language", "core:controls", "core:worlds"] {
        t.check(t.c().open_screen(name), format!("cannot open {name}"));
        t.screen(name).await?;
        // The world list shows the player's own saves: no fixed picture.
        if name != "core:worlds" {
            t.shot(name.trim_start_matches("core:")).await?;
        }
        t.c().close_screen();
        t.screen("core:title").await?;
    }
    Ok(())
}

pub async fn f3_overlay(t: T) -> R {
    t.world(5).await?;
    t.idle().await?;
    t.press("core:debug");
    t.frames(10).await?;
    t.shot("f3").await
}

fn id(name: &str) -> u32 {
    find_item(name).unwrap_or(u32::MAX)
}

/// The 2x2 grid of the inventory screen, driven by clicks on slots like a player would.
pub async fn inventory_crafting(t: T) -> R {
    t.flat().await?;
    t.op("put me 0 oak_log 16").await?;
    t.press("core:inventory");
    t.screen("core:menu").await?;
    // Menu slots: 0-3 grid, 4 result, 5-8 armour, 9-35 main, 36-44 hotbar.
    let (log, planks) = (id("base:oak_log"), id("base:oak_planks"));
    t.click_ui("slot:36", 0);
    t.until("logs on the cursor", Duration::from_secs(5), |t| t.c().cursor_stack().item == log).await?;
    t.click_ui("slot:0", 1);
    t.until("planks in the result", Duration::from_secs(5), |t| t.c().menu_slot(4).item == planks).await?;
    t.check_eq(t.c().menu_slot(4).count, 4, "planks from one log");
    t.click_ui("slot:36", 0);
    t.until("cursor empty", Duration::from_secs(5), |t| t.c().cursor_stack().is_empty()).await?;
    t.shot("grid").await?;
    t.click_ui("slot:4", 0);
    t.until("planks on the cursor", Duration::from_secs(5), |t| t.c().cursor_stack().item == planks).await?;
    t.click_ui("slot:37", 0);
    t.until("planks in the hotbar", Duration::from_secs(5), |t| t.c().inventory_slot(1).item == planks).await?;
    t.c().close_screen();
    t.no_screen().await?;
    t.check_eq(t.slot("me", 0).await?, ("base:oak_log".into(), 15, 0), "logs left");
    t.check_eq(t.slot("me", 1).await?, ("base:oak_planks".into(), 4, 0), "planks made");
    t.sync_inventory().await
}

pub async fn creative_inventory(t: T) -> R {
    t.flat().await?;
    t.op("mode creative").await?;
    t.until("creative", Duration::from_secs(5), |t| t.state().mode == 1).await?;
    t.press("core:inventory");
    t.screen("core:menu").await?;
    t.shot("tab").await?;
    let stone = id("base:bricks");
    if t.click_ui(&format!("item:{stone}"), 0) {
        t.until("bricks on the cursor", Duration::from_secs(5), |t| t.c().cursor_stack().item == stone).await?;
        t.click_ui("slot:36", 0);
        t.until("bricks in the hotbar", Duration::from_secs(5), |t| t.c().inventory_slot(0).item == stone).await?;
    }
    t.c().close_screen();
    t.no_screen().await?;
    t.check_eq(t.slot("me", 0).await?.0, "base:bricks".into(), "creative pick");
    Ok(())
}

pub async fn chat_completion(t: T) -> R {
    t.world(3).await?;
    t.press("core:chat");
    t.screen("core:chat").await?;
    t.c().type_text("/give @p birch_lo");
    t.frames(2).await?;
    t.c().key("Tab", 0.0);
    t.frames(3).await?;
    let text = t.c().input_text();
    t.check(text.contains("birch_log"), format!("Tab completed to '{text}'"));
    t.c().key("Enter", 0.0);
    t.no_screen().await?;
    t.until("logs given", Duration::from_secs(5), |t| t.c().inventory_slot(0).item == id("base:birch_log")).await?;
    Ok(())
}

/// Commands the client handles itself (`/gizmo`, `/log`) complete like the others.
pub async fn client_command_completion(t: T) -> R {
    t.world(3).await?;
    t.press("core:chat");
    t.screen("core:chat").await?;
    for (typed, want) in [("/giz", "/gizmo"), ("/gizmo cu", "/gizmo cubes"), ("/probe al", "/probe alpha")] {
        t.c().type_text(typed);
        t.frames(2).await?;
        t.c().key("Tab", 0.0);
        t.frames(3).await?;
        let text = t.c().input_text();
        t.check(text.starts_with(want), format!("Tab completed {typed} to '{text}', want {want}"));
        for _ in 0..text.chars().count() {
            t.c().key("Backspace", 0.0);
            t.frames(1).await?;
        }
    }
    t.c().key("Escape", 0.0);
    t.no_screen().await?;
    let reply = t.cmd("/gizmo cubes").await?;
    t.check(reply.contains("cubes on"), format!("/gizmo cubes said '{reply}'"));
    // Any word goes, the list is only for completion (mods' groups).
    let reply = t.cmd("/gizmo testmod:energy").await?;
    t.check(reply.contains("testmod:energy on"), format!("/gizmo testmod:energy said '{reply}'"));
    let reply = t.cmd("/probe beta").await?;
    t.check_eq(reply, "probe beta on the client".into(), "mod client command");
    Ok(())
}
