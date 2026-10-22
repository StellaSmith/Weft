//! The base game. It is an ordinary mod: everything here goes through `voxide_sdk`, nothing in the
//! engine knows about stone, water or zombies.

mod blocks;
mod items;
mod worldgen;
mod noise;

use voxide_sdk::*;

pub static OVERWORLD: Dimension = Dimension::new();
pub static FLATLANDS: Dimension = Dimension::new();

declare_mod!(id: "base", name: "Voxide", version: "0.1.0", deps: [], init: init);

fn init(r: &mut Registry) {
    blocks::register(r);
    items::register(r);
    worldgen::register_biomes(r);
    r.dimension("overworld", &OVERWORLD)
        .generator(worldgen::overworld)
        .spawn(worldgen::spawn)
        .sky_above(16)
        .day_length(24000);
    // Second dimension on purpose: nothing in the engine may assume there is only one world.
    r.dimension("flatlands", &FLATLANDS)
        .generator(worldgen::flat)
        .spawn(|_| [0, 3, 0])
        .height(-2, 8)
        .sky_above(1)
        .fixed_time(6000)
        .gravity(20.0)
        .sky(0xb0c8ff, 0xe0e8ff);
}
