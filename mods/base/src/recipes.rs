//! Crafting and smelting recipes. Ingredients are item names (this mod's namespace by default) or
//! `#tag` so other mods' planks and ingots work too.

use voxide_sdk::*;

const C: &str = "crafting";
const S: &str = "smelting";

pub fn register(r: &mut Registry) {
    r.recipe_station(C, "crafting_table");
    r.recipe_station("smelting", "furnace");
    for wood in ["oak", "birch", "spruce"] {
        r.shapeless(C, &format!("{wood}_planks"), 4, &[&format!("{wood}_log")], 0);
    }
    r.shaped(C, "stick", 4, &["#", "#"], &[('#', "#planks")]);
    r.shaped(C, "crafting_table", 1, &["##", "##"], &[('#', "#planks")]);
    r.shaped(C, "furnace", 1, &["###", "# #", "###"], &[('#', "cobblestone")]);
    r.shaped(C, "chest", 1, &["###", "# #", "###"], &[('#', "#planks")]);
    r.shaped(C, "torch", 4, &["c", "s"], &[('c', "coal"), ('s', "stick")]);
    r.shaped(C, "ladder", 3, &["s s", "sss", "s s"], &[('s', "stick")]);
    r.shaped(C, "bread", 1, &["www"], &[('w', "wheat")]);
    r.shaped(C, "stone_bricks", 4, &["##", "##"], &[('#', "stone")]);
    r.shaped(C, "wool", 1, &["ss", "ss"], &[('s', "string")]);
    r.shaped(C, "bucket", 1, &["i i", " i "], &[('i', "iron_ingot")]);
    r.shaped(C, "glowstone", 1, &["ttt", "tgt", "ttt"], &[('t', "torch"), ('g', "glass")]);
    r.shapeless(C, "red_lamp", 1, &["glowstone", "rose"], 0);
    r.shapeless(C, "green_lamp", 1, &["glowstone", "cactus"], 0);
    r.shapeless(C, "blue_lamp", 1, &["glowstone", "ice"], 0);
    r.shapeless(C, "red_stained_glass", 1, &["glass", "rose"], 0);
    r.shapeless(C, "green_stained_glass", 1, &["glass", "cactus"], 0);
    r.shapeless(C, "blue_stained_glass", 1, &["glass", "ice"], 0);

    // Every mod's `ore_blocks/<m>` packs its `ingots/<m>` or `gems/<m>`, and its ores smelt into them.
    for kind in ["#ingots/*", "#gems/*"] {
        r.shaped(C, "#ore_blocks/*", 1, &["###", "###", "###"], &[('#', kind)]);
        r.shapeless(C, kind, 9, &["#ore_blocks/*"], 0);
        r.shapeless(S, kind, 1, &["#ores/*"], 200);
    }
    r.shapeless(S, "#cooked_meat/*", 1, &["#raw_meat/*"], 200);
    r.shapeless(S, "#ingots/*", 1, &["#raw_ores/*"], 200);

    // A `pickaxes/<m>` or `armor_chest/<m>` item is made from `materials/<m>`.
    for (tool, pattern) in [
        ("pickaxes", &["XXX", " # ", " # "][..]),
        ("axes", &["XX", "X#", " #"]),
        ("shovels", &["X", "#", "#"]),
        ("hoes", &["XX", " #", " #"]),
        ("swords", &["X", "X", "#"]),
    ] {
        r.shaped(C, &format!("#{tool}/*"), 1, pattern, &[('X', "#materials/*"), ('#', "stick")]);
    }
    for (slot, pattern) in crate::items::ARMOR_SLOTS.iter().zip([&["XXX", "X X"][..], &["X X", "XXX", "XXX"], &["XXX", "X X", "X X"], &["X X", "X X"]]) {
        r.shaped(C, &format!("#{slot}/*"), 1, pattern, &[('X', "#materials/*")]);
    }

    for (input, output, ticks) in [
        ("sand", "glass", 200),
        ("cobblestone", "stone", 200),
        ("#logs", "coal", 200),
        ("clay", "bricks", 200),
    ] {
        r.shapeless(S, output, 1, &[input], ticks);
    }
}
