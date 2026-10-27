//! Furnace and chest: blocks with an entity holding their inventory and state.

use weft_sdk::*;

use crate::blocks::{CHEST, FURNACE};
use crate::items::FUEL;

pub use base_api::{SMELT_TIME, SMELTED, SMELTING_STATE as SMELTING, Smelted, Smelting};
pub static FURNACE_MENU: Menu = Menu::named("furnace");
pub static CHEST_MENU: Menu = Menu::named("chest");

pub fn register(r: &mut Registry) {
    r.entity_component(&SMELTING, Smelting { burn: 0, burn_max: 0, cook: 0, cook_max: 0 }, component_flags::SAVE | component_flags::SYNC_MENU);
    r.system(&SMELTING, tick_furnaces);
    // The furnace's state syncs to whoever has it open: the screen binds to it directly.
    r.machine(&FURNACE, &FURNACE_MENU)
        .block(|b| {
            // Front towards the player; unlit and lit.
            b.sound_group("stone")
                .facing(Facings::HORIZONTAL, Toward::Player)
                .prop("lit", ["false", "true"])
                .states(|p, s| {
                    let lit = p.is("lit", "true");
                    let mut f = ["furnace_side"; 6];
                    f[2] = "furnace_top";
                    f[3] = "furnace_top";
                    f[p.facing().face() as usize] = if lit { "furnace_front_lit" } else { "furnace_front" };
                    let s = s.faces(f);
                    if lit { s.light_rgb(13, 11, 8) } else { s }
                })
                .hardness(3.5)
                .mineable("pickaxe").needs_tool(0)
                .tab(&crate::tabs::FUNCTIONAL)
        })
        .group("input", Slots::single())
        .group("fuel", Slots::single().accepts("fuel"))
        .group("output", Slots::single().output())
        .recipes(&crate::recipes::SMELTING)
        .attach(&SMELTING)
        .screen(|ui| {
            ui.panel("panel", S::col().pad(7.0).gap(4.0).stretch(), |ui| {
                ui.title();
                ui.row(S::row().gap(8.0), |ui| {
                    ui.spacer();
                    ui.column(S::col().gap(2.0), |ui| {
                        ui.group("input");
                        ui.progress_bind_dir("flame", S::default().size(14.0, 14.0), abi::ui_dir::UP, |c| c.menu_component(&SMELTING).map_or(0.0, |s| ratio(s.burn, s.burn_max)));
                        ui.group("fuel");
                    });
                    ui.recipe_arrow_bind(|c| c.menu_component(&SMELTING).map_or(0.0, |s| ratio(s.cook, s.cook_max)));
                    ui.group("output");
                    ui.spacer();
                });
                ui.player_inventory(true);
            });
        });

    r.machine(&CHEST, &CHEST_MENU)
        .block(|b| {
            b.sound_group("wood")
                .facing(Facings::HORIZONTAL, Toward::Player)
                .states(|p, s| {
                    let d = p.facing();
                    let mut f = ["chest_side"; 6];
                    f[2] = "chest_top";
                    f[3] = "chest_top";
                    f[d.face() as usize] = "chest_front";
                    s.model_box_faces([1.0, 0.0, 1.0], [15.0, 14.0, 15.0], f)
                })
                .hardness(2.5)
                .mineable("axe")
                .tab(&crate::tabs::FUNCTIONAL)
        })
        .group("chest", Slots::chest(3))
        .screen(|ui| {
            ui.panel("panel", S::col().pad(7.0).gap(4.0), |ui| {
                ui.title();
                ui.group("chest");
                ui.player_inventory(true);
            });
        });
}

fn ratio(a: u32, b: u32) -> f32 {
    if b == 0 { 0.0 } else { a as f32 / b as f32 }
}

/// Every furnace of a region at once, through its state (a system: no host call to reach it).
fn tick_furnaces(b: &mut SystemBatch<Smelting>) {
    let w = b.world();
    for (id, s) in b.iter_mut() {
        let Some(st) = w.entity(id) else { continue };
        let pos = [st.pos[0].floor() as i32, st.pos[1].floor() as i32, st.pos[2].floor() as i32];
        let inv = w.inventory(id);
        let (input_slot, fuel_slot, output_slot) = (inv.group("input"), inv.group("fuel"), inv.group("output"));
        let (input, fuel, output) = (input_slot.get(0), fuel_slot.get(0), output_slot.get(0));
        let was_lit = s.burn > 0;
        if s.burn > 0 {
            s.burn -= 1;
        }
        let recipe = (!input.is_empty()).then(|| recipe_process(&crate::recipes::SMELTING, input.item)).flatten();
        let fits = recipe.is_some_and(|(out, _)| {
            output.is_empty() || (output.stacks_with(&out) && output.count + out.count <= max_stack(out.item))
        });
        if s.burn == 0 && fits {
            if let Some(ticks) = FUEL.on_item(fuel.item).filter(|_| !fuel.is_empty()) {
                s.burn = ticks;
                s.burn_max = ticks;
                fuel_slot.set(0, use_one(fuel));
            }
        }
        if s.burn > 0 && fits {
            let (out, time) = recipe.unwrap();
            let q = abi::Query { entity: id, item: input.item, ..query() };
            s.cook_max = w.compute(&SMELT_TIME, time as f64, &q).round().max(1.0) as u32;
            s.cook += 1;
            if s.cook >= s.cook_max {
                s.cook = 0;
                input_slot.set(0, input.shrink(1));
                let merged = if output.is_empty() { out } else { output.grow(out.count) };
                output_slot.set(0, merged);
                w.emit(&SMELTED, Smelted { furnace: id, input: input.item, output: out });
            }
        } else if s.cook > 0 {
            s.cook = s.cook.saturating_sub(2);
        }
        let lit = s.burn > 0;
        if lit != was_lit && FURNACE.is(w.get(pos)) {
            w.set_prop(pos, "lit", if lit { "true" } else { "false" });
        }
    }
}
