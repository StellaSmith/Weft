//! Survival rules on top of the engine's player: hunger, regeneration, drowning, armour, eating,
//! the HUD and the inventory screen with its 2x2 crafting grid.

use voxide_sdk::abi::slot_flags as sf;
use voxide_sdk::*;

use crate::items::{ARMOR, Armor, FOOD, Food};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hunger {
    pub food: f32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub air: f32,
    /// Armour points worn, kept here so the HUD can show them.
    pub armor: f32,
    pub timer: u32,
}

pub static HUNGER: Component<Hunger> = Component::new();
pub static INVENTORY: Menu = Menu::new();
pub static CRAFTING: Menu = Menu::new();

const MAX_AIR: f32 = 300.0;

pub fn register(r: &mut Registry) {
    r.component(
        "hunger",
        &HUNGER,
        Hunger { food: 20.0, saturation: 5.0, exhaustion: 0.0, air: MAX_AIR, armor: 0.0, timer: 0 },
        component_flags::SAVE | component_flags::SYNC_OWNER,
        &["core:player"],
    );
    r.hook_tick("core:player", tick_players);
    r.hook("core:player", Hook::Hurt, |e| {
        let cause = e.cause();
        if cause == damage::FALL || cause == damage::STARVE || cause == damage::DROWN || cause == damage::VOID {
            return;
        }
        let w = e.world();
        let inv = w.inventory(e.entity());
        let mut points = 0;
        for slot in 36..40 {
            let s = inv.get(slot);
            if let Some(a) = item_prop::<Armor>(s.item, ARMOR) {
                points += a.points;
                let (_, durability, _) = item_info(s.item);
                let damage = s.damage + 1;
                inv.set(slot, if durability > 0 && damage >= durability { ItemStack::EMPTY } else { ItemStack { damage, ..s } });
            }
        }
        let reduced = e.amount() * (1.0 - points.min(20) as f32 * 0.04);
        e.set_amount(reduced);
    });

    r.hud(|ui| {
        ui.row(S::row().w(182.0).gap(2.0), |ui| {
            ui.column(S::col().gap(1.0), |ui| {
                ui.icons("armor", 10, |c| c.component(&HUNGER).map_or(-1.0, |h| if h.armor > 0.0 { h.armor } else { -1.0 }));
                ui.icons("hearts", 10, |c| c.me().map_or(20.0, |s| s.health.ceil()));
            });
            ui.spacer();
            ui.column(S::col().gap(1.0).end(), |ui| {
                ui.icons("air", 10, |c| c.component(&HUNGER).map_or(-1.0, |h| if h.air < MAX_AIR { (h.air.max(0.0) / MAX_AIR * 20.0).ceil() } else { -1.0 }));
                ui.icons("food", 10, |c| c.component(&HUNGER).map_or(20.0, |h| h.food.ceil()));
            });
        });
    });

    // Slots: 0-3 grid, 4 result, 5-8 armour, 9-35 main, 36-44 hotbar.
    r.menu("inventory", &INVENTORY)
        .temp_slots(5)
        .setup(|m| {
            m.slots(TEMP, 0, 4, 0);
            m.slots(TEMP, 4, 1, sf::OUTPUT);
            for (i, f) in [sf::ARMOR_HEAD, sf::ARMOR_CHEST, sf::ARMOR_LEGS, sf::ARMOR_FEET].into_iter().enumerate() {
                m.slots(PLAYER, 36 + i as u32, 1, f);
            }
            m.slots(PLAYER, 9, 27, 0);
            m.slots(PLAYER, 0, 9, 0);
        })
        .on_change(|m| craft_result(m, 0, 2, 4))
        .on_take(|m| consume_grid(m, 0, 4))
        .screen(|ui| {
            ui.panel("panel", S::col().pad(7.0).gap(5.0), |ui| {
                ui.row(S::row().gap(1.0), |ui| {
                    ui.column(S::col(), |ui| {
                        for i in 5..9 {
                            ui.slot(i);
                        }
                    });
                    ui.player_model(51.0, 72.0);
                    ui.spacer();
                    ui.column(S::col().gap(2.0), |ui| {
                        ui.text("container.crafting");
                        ui.row(S::row().gap(4.0), |ui| {
                            ui.slots(0, 4, 2);
                            ui.image("base:gui/arrow_bg", 24.0, 16.0);
                            ui.slot(4);
                        });
                    });
                });
                player_slots(ui, 9, false);
            });
        });

    r.menu("crafting_table", &CRAFTING)
        .temp_slots(10)
        .setup(|m| {
            m.slots(TEMP, 0, 9, 0);
            m.slots(TEMP, 9, 1, sf::OUTPUT);
            m.slots(PLAYER, 9, 27, 0);
            m.slots(PLAYER, 0, 9, 0);
        })
        .on_change(|m| craft_result(m, 0, 3, 9))
        .on_take(|m| consume_grid(m, 0, 9))
        .screen(|ui| {
            ui.panel("panel", S::col().pad(7.0).gap(4.0), |ui| {
                ui.text("container.crafting");
                ui.row(S::row().gap(8.0), |ui| {
                    ui.spacer();
                    ui.slots(0, 9, 3);
                    ui.image("base:gui/arrow_bg", 24.0, 16.0);
                    ui.slot(9);
                    ui.spacer();
                });
                player_slots(ui, 10, true);
            });
        });
    r.player_menu("inventory");
}

/// Main inventory (27) and hotbar (9) starting at menu slot `start`, spaced like vanilla; the
/// title is left out on the player's own inventory screen.
pub fn player_slots(ui: &mut Ui, start: u32, title: bool) {
    ui.column(S::col().gap(2.0), |ui| {
        if title {
            ui.text("container.inventory");
        }
        ui.column(S::col().gap(4.0), |ui| {
            ui.slots(start, 27, 9);
            ui.slots(start + 27, 9, 9);
        });
    });
}

fn craft_result(m: &MenuCtx, start: u32, size: u32, out: u32) {
    let grid: Vec<ItemStack> = (start..start + size * size).map(|i| m.get(i)).collect();
    m.set(out, recipe_match("base:crafting", &grid, size, size));
}

fn consume_grid(m: &MenuCtx, start: u32, count: u32) {
    for i in start..start + count {
        let s = m.get(i);
        if !s.is_empty() {
            m.set(i, ItemStack { count: s.count - 1, ..s });
        }
    }
}

fn tick_players(t: &EntityTick) {
    let w = t.world();
    for &id in t.ids() {
        let Some(mut st) = w.entity(id) else { continue };
        let Some(h) = w.component(id, &HUNGER) else { continue };
        h.exhaustion += st.moved * 0.01 + if st.vel[1] > 3.0 { 0.01 } else { 0.0 };
        if h.exhaustion > 4.0 {
            h.exhaustion -= 4.0;
            if h.saturation > 0.0 {
                h.saturation = (h.saturation - 1.0).max(0.0);
            } else {
                h.food = (h.food - 1.0).max(0.0);
            }
        }
        h.timer += 1;
        if h.timer >= 80 {
            h.timer = 0;
            if h.food >= 18.0 && st.health < st.max_health {
                st.health = (st.health + 1.0).min(st.max_health);
                h.exhaustion += 3.0;
                w.set_entity(id, &st);
            } else if h.food <= 0.0 && st.health > 1.0 {
                w.damage(id, 1.0, 0, damage::STARVE);
            }
        }
        let eye = [st.pos[0].floor() as i32, (st.pos[1] + 1.62).floor() as i32, st.pos[2].floor() as i32];
        let underwater = w.flags(eye) & abi::block_flags::LIQUID != 0;
        let inv = w.inventory(id);
        let armor: u32 = (36..40).filter_map(|i| item_prop::<Armor>(inv.get(i).item, ARMOR)).map(|a| a.points).sum();
        let Some(h) = w.component(id, &HUNGER) else { continue };
        h.armor = armor as f32;
        if underwater {
            h.air -= 1.0;
            if h.air <= -20.0 {
                h.air = 0.0;
                w.damage(id, 2.0, 0, damage::DROWN);
            }
        } else if h.air < MAX_AIR {
            h.air = (h.air + 5.0).min(MAX_AIR);
        }
    }
}

/// Eating: instant, consumes one item.
pub fn eat(u: &mut ItemUse) -> bool {
    let stack = u.stack();
    let Some(food) = item_prop::<Food>(stack.item, FOOD) else { return false };
    let w = u.world();
    let Some(h) = w.component(u.player(), &HUNGER) else { return false };
    if h.food >= 20.0 {
        return false;
    }
    h.food = (h.food + food.hunger as f32).min(20.0);
    h.saturation = (h.saturation + food.saturation).min(h.food);
    u.set_stack(ItemStack { count: stack.count - 1, ..stack });
    true
}
