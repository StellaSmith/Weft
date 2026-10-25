//! The base game. It is an ordinary mod: everything here goes through `voxide_sdk`, nothing in the
//! engine knows about stone, water or zombies.

mod blocks;
mod items;
mod machines;
mod mobs;
mod player;
mod recipes;
mod sounds;
mod worldgen;

use voxide_sdk::*;

pub static OVERWORLD: Dimension = Dimension::new();
pub static FLATLANDS: Dimension = Dimension::new();

declare_mod!(id: "base", name: "Voxide", version: "0.1.0", deps: [], init: init);

fn init(r: &mut Registry) {
    // Vanilla's creative tabs; blocks and items join them with `.tab(..)`.
    for (tab, icon) in [
        ("building", "bricks"),
        ("colored", "red_stained_glass"),
        ("natural", "grass"),
        ("functional", "crafting_table"),
        ("tools", "iron_pickaxe"),
        ("combat", "iron_sword"),
        ("food", "apple"),
        ("ingredients", "iron_ingot"),
    ] {
        r.creative_tab(tab, icon);
    }
    blocks::register(r);
    items::register(r);
    player::register(r);
    machines::register(r);
    recipes::register(r);
    sounds::register(r);
    mobs::register(r);
    worldgen::register(r);
    // Its terrain is described in `worldgen`.
    r.dimension("overworld").handle(&OVERWORLD)
        .spawn(worldgen::spawn)
        .player_spawn()
        .sky_above(16)
        .day_length(24000);
    // Second dimension on purpose: nothing in the engine may assume there is only one world.
    r.dimension("flatlands").handle(&FLATLANDS)
        .generator(worldgen::flat)
        .spawn(|_| [0, 3, 0])
        .height(-2, 8)
        .sky_above(1)
        .fixed_time(6000)
        .gravity(20.0)
        .sky(0xb0c8ff, 0xe0e8ff);
}
