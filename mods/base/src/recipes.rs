//! Crafting and smelting recipes. Ingredients are item names (this mod's namespace by default) or
//! `#tag` so other mods' planks and ingots work too.

use voxide_sdk::*;

const C: &str = "crafting";
const S: &str = "smelting";

pub fn register(r: &mut Registry) {
    for wood in ["oak", "birch", "spruce"] {
        r.shapeless(C, &format!("{wood}_planks"), 4, &[&format!("{wood}_log")], 0);
    }
    r.shaped(C, "stick", 4, &["#", "#"], &[('#', "#base:planks")]);
    r.shaped(C, "crafting_table", 1, &["##", "##"], &[('#', "#base:planks")]);
    r.shaped(C, "furnace", 1, &["###", "# #", "###"], &[('#', "cobblestone")]);
    r.shaped(C, "chest", 1, &["###", "# #", "###"], &[('#', "#base:planks")]);
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

    for (block, item) in [("iron_block", "iron_ingot"), ("gold_block", "gold_ingot"), ("diamond_block", "diamond"), ("coal_block", "coal")] {
        r.shaped(C, block, 1, &["###", "###", "###"], &[('#', item)]);
        r.shapeless(C, item, 9, &[block], 0);
    }

    for (mat, input) in [("wood", "#base:planks"), ("stone", "cobblestone"), ("iron", "iron_ingot"), ("gold", "gold_ingot"), ("diamond", "diamond")] {
        let keys = [('X', input), ('#', "stick")];
        r.shaped(C, &format!("{mat}_pickaxe"), 1, &["XXX", " # ", " # "], &keys);
        r.shaped(C, &format!("{mat}_axe"), 1, &["XX", "X#", " #"], &keys);
        r.shaped(C, &format!("{mat}_shovel"), 1, &["X", "#", "#"], &keys);
        r.shaped(C, &format!("{mat}_hoe"), 1, &["XX", " #", " #"], &keys);
        r.shaped(C, &format!("{mat}_sword"), 1, &["X", "X", "#"], &keys);
    }
    for (mat, input) in [("leather", "leather"), ("iron", "iron_ingot"), ("gold", "gold_ingot"), ("diamond", "diamond")] {
        let keys = [('X', input)];
        r.shaped(C, &format!("{mat}_helmet"), 1, &["XXX", "X X"], &keys);
        r.shaped(C, &format!("{mat}_chestplate"), 1, &["X X", "XXX", "XXX"], &keys);
        r.shaped(C, &format!("{mat}_leggings"), 1, &["XXX", "X X", "X X"], &keys);
        r.shaped(C, &format!("{mat}_boots"), 1, &["X X", "X X"], &keys);
    }

    for (input, output, ticks) in [
        ("sand", "glass", 200),
        ("cobblestone", "stone", 200),
        ("iron_ore", "iron_ingot", 200),
        ("gold_ore", "gold_ingot", 200),
        ("porkchop", "cooked_porkchop", 200),
        ("beef", "cooked_beef", 200),
        ("#base:logs", "coal", 200),
        ("clay", "bricks", 200),
    ] {
        r.shapeless(S, output, 1, &[input], ticks);
    }
}
