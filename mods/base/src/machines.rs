//! Furnace and chest: blocks with an entity holding their inventory and state.

use voxide_sdk::abi::slot_flags as sf;
use voxide_sdk::*;

use crate::blocks::FURNACE;
use crate::items::{FUEL, REMAINDER};
use crate::player::{CRAFTING, player_slots};

value! {
    pub struct Smelting { pub burn: u32, pub burn_max: u32, pub cook: u32, pub cook_max: u32 }
}

pub static SMELTING: Component<Smelting> = Component::new("base:smelting");

value! {
    /// Fired when a furnace finishes one item.
    pub struct Smelted { pub furnace: u64, pub input: u32, pub output: ItemStack }
}

pub static SMELTED: Event<Smelted> = Event::new("smelted");
/// Value chain for a furnace's cooking time: base is the recipe's time, `entity` the furnace,
/// `item` the input.
pub const SMELT_TIME: &str = "base:smelt_time";
pub static FURNACE_ENTITY: Entity = Entity::new();
pub static CHEST_ENTITY: Entity = Entity::new();
pub static FURNACE_MENU: Menu = Menu::new();
pub static CHEST_MENU: Menu = Menu::new();

const INPUT: u32 = 0;
const FUEL_SLOT: u32 = 1;
const OUTPUT: u32 = 2;

pub fn register(r: &mut Registry) {
    // Synced to whoever has the furnace open: the screen binds to it directly.
    r.entity_component(&SMELTING, Smelting { burn: 0, burn_max: 0, cook: 0, cook_max: 0 }, component_flags::SAVE | component_flags::SYNC_MENU, &["base:furnace"]);
    r.entity("furnace").handle(&FURNACE_ENTITY).flags(entity_flags::NO_GRAVITY | entity_flags::INVULNERABLE).on_tick(tick_furnaces);
    r.entity("chest").handle(&CHEST_ENTITY).flags(entity_flags::NO_GRAVITY | entity_flags::INVULNERABLE);
    r.block_entity("furnace", "furnace", 3);
    r.block_entity("chest", "chest", 27);

    r.menu("furnace").handle(&FURNACE_MENU)
        .setup(|m| {
            m.slots(m.owner(), INPUT, 1, 0);
            m.slots(m.owner(), FUEL_SLOT, 1, 0);
            m.slots(m.owner(), OUTPUT, 1, sf::OUTPUT);
            m.slots(PLAYER, 9, 27, 0);
            m.slots(PLAYER, 0, 9, 0);
        })
        .screen(|ui| {
            ui.panel("panel", S::col().pad(7.0).gap(4.0).stretch(), |ui| {
                ui.text("container.furnace");
                ui.row(S::row().gap(8.0), |ui| {
                    ui.spacer();
                    ui.column(S::col().gap(2.0), |ui| {
                        ui.slot(0);
                        ui.progress_bind_dir("flame", S::default().size(14.0, 14.0), abi::ui_dir::UP, |c| c.menu_component(&SMELTING).map_or(0.0, |s| ratio(s.burn, s.burn_max)));
                        ui.slot(1);
                    });
                    ui.progress_bind("arrow", S::default().size(24.0, 16.0), |c| c.menu_component(&SMELTING).map_or(0.0, |s| ratio(s.cook, s.cook_max)));
                    ui.slot(2);
                    ui.spacer();
                });
                player_slots(ui, 3, true);
            });
        });

    r.menu("chest").handle(&CHEST_MENU)
        .setup(|m| {
            m.slots(m.owner(), 0, 27, 0);
            m.slots(PLAYER, 9, 27, 0);
            m.slots(PLAYER, 0, 9, 0);
        })
        .screen(|ui| {
            ui.panel("panel", S::col().pad(7.0).gap(4.0), |ui| {
                ui.text("container.chest");
                ui.slots(0, 27, 9);
                player_slots(ui, 27, true);
            });
        });
}

fn ratio(a: u32, b: u32) -> f32 {
    if b == 0 { 0.0 } else { a as f32 / b as f32 }
}

/// Called from the block registrations: right click opens the menu of the block's entity.
pub fn open_on_use(menu: &'static Menu) -> impl Fn(&mut Use) -> bool + Send + Sync + 'static {
    move |u| {
        let w = u.world();
        let owner = w.block_entity(u.pos());
        if owner == 0 {
            return false;
        }
        w.open_menu(u.player(), menu, owner);
        true
    }
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
        let (input, fuel, output) = (inv.get(INPUT), inv.get(FUEL_SLOT), inv.get(OUTPUT));
        let Some(s) = w.component_mut(id, &SMELTING) else { continue };
        let was_lit = s.burn > 0;
        if s.burn > 0 {
            s.burn -= 1;
        }
        let recipe = (!input.is_empty()).then(|| recipe_process("base:smelting", input.item)).flatten();
        let fits = recipe.is_some_and(|(out, _)| {
            output.is_empty() || (output.stacks_with(&out) && output.count + out.count <= max_stack(out.item))
        });
        if s.burn == 0 && fits {
            if let Some(ticks) = FUEL.on_item(fuel.item).filter(|_| !fuel.is_empty()) {
                s.burn = ticks;
                s.burn_max = ticks;
                // Lava buckets leave the bucket behind.
                let rest = match REMAINDER.on_item(fuel.item).and_then(|r| find_item(r.as_str())) {
                    Some(left) if fuel.count == 1 => ItemStack::new(left, 1),
                    _ => fuel.shrink(1),
                };
                inv.set(FUEL_SLOT, rest);
            }
        }
        if s.burn > 0 && fits {
            let (out, time) = recipe.unwrap();
            let q = abi::Query { entity: id, item: input.item, ..query() };
            let time = w.compute(SMELT_TIME, time as f64, &q).round().max(1.0) as u32;
            let Some(s) = w.component_mut(id, &SMELTING) else { continue };
            s.cook_max = time;
            s.cook += 1;
            if s.cook >= s.cook_max {
                s.cook = 0;
                inv.set(INPUT, input.shrink(1));
                let merged = if output.is_empty() { out } else { output.grow(out.count) };
                inv.set(OUTPUT, merged);
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
                let facing = FURNACE.index(state) % 4;
                w.set(pos, FURNACE.state(facing + if lit { 4 } else { 0 }));
            }
        }
    }
}
