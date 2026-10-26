//! Furnace and chest: blocks with an entity holding their inventory and state.

use weft_sdk::*;

use crate::blocks::{CHEST, FURNACE};
use crate::items::FUEL;
use crate::player::CRAFTING;

value! {
    pub struct Smelting { pub burn: u32, pub burn_max: u32, pub cook: u32, pub cook_max: u32 }
}

pub static SMELTING: Component<Smelting> = Component::named("base:smelting");

value! {
    /// Fired when a furnace finishes one item.
    pub struct Smelted { pub furnace: u64, pub input: u32, pub output: ItemStack }
}

pub static SMELTED: Event<Smelted> = Event::named("smelted");
/// Value chain for a furnace's cooking time: base is the recipe's time, `entity` the furnace,
/// `item` the input.
pub static SMELT_TIME: Chain = Chain::named("smelt_time");
pub static FURNACE_MENU: Menu = Menu::named("furnace");
pub static CHEST_MENU: Menu = Menu::named("chest");

pub fn register(r: &mut Registry) {
    // The furnace's state syncs to whoever has it open: the screen binds to it directly.
    r.machine(&FURNACE, &FURNACE_MENU)
        .block(|b| {
            // Front towards the player; unlit and lit.
            b.sounds("stone")
                .facing_variants(Facings::HORIZONTAL, Toward::Player, 2, |d, lit, s| {
                    let mut f = ["furnace_side"; 6];
                    f[2] = "furnace_top";
                    f[3] = "furnace_top";
                    f[d.face() as usize] = if lit == 1 { "furnace_front_lit" } else { "furnace_front" };
                    let s = s.faces(f);
                    if lit == 1 { s.light_rgb(13, 11, 8) } else { s }
                })
                .hardness(3.5)
                .mineable("pickaxe").needs_tool(0)
                .tab(&crate::tabs::FUNCTIONAL)
        })
        .group("input", Slots::one())
        .group("fuel", Slots::one().accepts("fuel"))
        .group("output", Slots::one().output())
        .recipes(&crate::recipes::SMELTING)
        .component(&SMELTING, Smelting { burn: 0, burn_max: 0, cook: 0, cook_max: 0 }, component_flags::SAVE | component_flags::SYNC_MENU)
        .on_tick(tick_furnaces)
        .screen(|ui| {
            ui.panel("panel", S::col().pad(7.0).gap(4.0).stretch(), |ui| {
                ui.text("container.furnace");
                ui.row(S::row().gap(8.0), |ui| {
                    ui.spacer();
                    ui.column(S::col().gap(2.0), |ui| {
                        ui.group("input");
                        ui.progress_bind_dir("flame", S::default().size(14.0, 14.0), abi::ui_dir::UP, |c| c.menu_component(&SMELTING).map_or(0.0, |s| ratio(s.burn, s.burn_max)));
                        ui.group("fuel");
                    });
                    ui.progress_bind("arrow", S::default().size(24.0, 16.0), |c| c.menu_component(&SMELTING).map_or(0.0, |s| ratio(s.cook, s.cook_max)));
                    ui.group("output");
                    ui.spacer();
                });
                ui.player_inventory(true);
            });
        });

    r.machine(&CHEST, &CHEST_MENU)
        .block(|b| {
            b.sounds("wood")
                .facing(Facings::HORIZONTAL, Toward::Player, |d, s| {
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
                ui.text("container.chest");
                ui.group("chest");
                ui.player_inventory(true);
            });
        });
}

fn ratio(a: u32, b: u32) -> f32 {
    if b == 0 { 0.0 } else { a as f32 / b as f32 }
}

pub fn open_crafting(u: &mut Use) -> bool {
    u.world().open_menu(u.player(), &CRAFTING, 0);
    true
}

fn tick_furnaces(t: &EntityTick) {
    let w = t.world();
    for &id in t.ids() {
        let Some(st) = w.entity(id) else { continue };
        let pos = [st.pos[0].floor() as i32, st.pos[1].floor() as i32, st.pos[2].floor() as i32];
        let inv = w.inventory(id);
        let (input_slot, fuel_slot, output_slot) = (inv.group("input"), inv.group("fuel"), inv.group("output"));
        let (input, fuel, output) = (input_slot.get(0), fuel_slot.get(0), output_slot.get(0));
        let Some(s) = w.component_mut(id, &SMELTING) else { continue };
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
            let time = w.compute(&SMELT_TIME, time as f64, &q).round().max(1.0) as u32;
            let Some(s) = w.component_mut(id, &SMELTING) else { continue };
            s.cook_max = time;
            s.cook += 1;
            if s.cook >= s.cook_max {
                s.cook = 0;
                input_slot.set(0, input.shrink(1));
                let merged = if output.is_empty() { out } else { output.grow(out.count) };
                output_slot.set(0, merged);
                w.emit(&SMELTED, Smelted { furnace: id, input: input.item, output: out });
            }
        } else if let Some(s) = w.component_mut(id, &SMELTING).filter(|s| s.cook > 0) {
            s.cook = s.cook.saturating_sub(2);
        }
        let Some(s) = w.component(id, &SMELTING) else { continue };
        let lit = s.burn > 0;
        if lit != was_lit {
            let state = w.get(pos);
            if FURNACE.is(state) {
                let (d, _) = Facings::HORIZONTAL.parse(FURNACE.index(state));
                w.set(pos, FURNACE.state(Facings::HORIZONTAL.index(d, lit as u32)));
            }
        }
    }
}
