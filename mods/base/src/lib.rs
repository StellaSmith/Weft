//! The base game. It is an ordinary mod: everything here goes through `weft_sdk`, nothing in the
//! engine knows about stone, water or zombies.

mod blocks;
mod items;
mod machines;
mod mobs;
mod player;
mod recipes;
mod sounds;
mod worldgen;

use weft_sdk::*;

/// Vanilla's creative tabs.
pub mod tabs {
    use weft_sdk::CreativeTab;
    pub static BUILDING: CreativeTab = CreativeTab::named("building");
    pub static COLORED: CreativeTab = CreativeTab::named("colored");
    pub static NATURAL: CreativeTab = CreativeTab::named("natural");
    pub static FUNCTIONAL: CreativeTab = CreativeTab::named("functional");
    pub static TOOLS: CreativeTab = CreativeTab::named("tools");
    pub static COMBAT: CreativeTab = CreativeTab::named("combat");
    pub static FOOD: CreativeTab = CreativeTab::named("food");
    pub static INGREDIENTS: CreativeTab = CreativeTab::named("ingredients");
}

pub static OVERWORLD: Dimension = Dimension::named("overworld");
pub static FLATLANDS: Dimension = Dimension::named("flatlands");

declare_mod!(id: "base", name: "Weft", version: "0.1.0", deps: [], init: init);

fn init(r: &mut Registry) {
    // Vanilla's creative tabs; blocks and items join them with `.tab(..)`.
    for (tab, icon) in [
        (&tabs::BUILDING, &Item::named("bricks")),
        (&tabs::COLORED, &Item::named("red_stained_glass")),
        (&tabs::NATURAL, &Item::named("grass")),
        (&tabs::FUNCTIONAL, &Item::named("crafting_table")),
        (&tabs::TOOLS, &Item::named("iron_pickaxe")),
        (&tabs::COMBAT, &Item::named("iron_sword")),
        (&tabs::FOOD, &Item::named("apple")),
        (&tabs::INGREDIENTS, &items::IRON_INGOT),
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
    r.dimension(&OVERWORLD)
        .spawn(worldgen::spawn)
        .player_spawn()
        .sky_above(16)
        .day_length(24000);
    // Second dimension on purpose: nothing in the engine may assume there is only one world.
    r.dimension(&FLATLANDS)
        .generator(worldgen::flat)
        .spawn(|_| [0, 3, 0])
        .height(-2, 8)
        .sky_above(1)
        .fixed_time(6000)
        .gravity(20.0)
        .sky(0xb0c8ff, 0xe0e8ff);
}
