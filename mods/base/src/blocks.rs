use voxide_sdk::abi::block_flags as bf;
use voxide_sdk::*;

use crate::items::{FLAMMABLE, FUEL};
use crate::worldgen;

pub static GRASS_SPREADS_TO: Tag = Tag::new("grass_spreads_to");

pub static STONE: Block = Block::new();
pub static COBBLESTONE: Block = Block::new();
pub static MOSSY_COBBLESTONE: Block = Block::new();
pub static DIRT: Block = Block::new();
pub static GRASS: Block = Block::new();
pub static SNOWY_GRASS: Block = Block::new();
pub static SAND: Block = Block::new();
pub static GRAVEL: Block = Block::new();
pub static CLAY: Block = Block::new();
pub static SANDSTONE: Block = Block::new();
pub static SNOW: Block = Block::new();
pub static ICE: Block = Block::new();
pub static OBSIDIAN: Block = Block::new();
pub static COAL_ORE: Block = Block::new();
pub static IRON_ORE: Block = Block::new();
pub static GOLD_ORE: Block = Block::new();
pub static DIAMOND_ORE: Block = Block::new();
pub static OAK_LOG: Block = Block::new();
pub static BIRCH_LOG: Block = Block::new();
pub static SPRUCE_LOG: Block = Block::new();
pub static OAK_PLANKS: Block = Block::new();
pub static BIRCH_PLANKS: Block = Block::new();
pub static SPRUCE_PLANKS: Block = Block::new();
pub static OAK_LEAVES: Block = Block::new();
pub static BIRCH_LEAVES: Block = Block::new();
pub static SPRUCE_LEAVES: Block = Block::new();
pub static WATER: Block = Block::new();
pub static LAVA: Block = Block::new();
pub static TALL_GRASS: Block = Block::new();
pub static DANDELION: Block = Block::new();
pub static ROSE: Block = Block::new();
pub static SAPLING: Block = Block::new();
pub static SUGAR_CANE: Block = Block::new();
pub static CACTUS: Block = Block::new();
pub static WHEAT: Block = Block::new();
pub static FARMLAND: Block = Block::new();
pub static GLASS: Block = Block::new();
pub static CRAFTING_TABLE: Block = Block::new();
pub static FURNACE: Block = Block::new();
pub static CHEST: Block = Block::new();
pub static TORCH: Block = Block::new();
pub static LADDER: Block = Block::new();
pub static GLOWSTONE: Block = Block::new();
pub static BRICKS: Block = Block::new();
pub static STONE_BRICKS: Block = Block::new();
pub static IRON_BLOCK: Block = Block::new();
pub static GOLD_BLOCK: Block = Block::new();
pub static DIAMOND_BLOCK: Block = Block::new();
pub static COAL_BLOCK: Block = Block::new();
pub static WOOL: Block = Block::new();

/// Log axis from the clicked face: 0 = Y, 1 = X, 2 = Z.
fn axis_placement(p: &Place) -> u32 {
    match p.face() {
        0 | 1 => 1,
        4 | 5 => 2,
        _ => 0,
    }
}

fn log(r: &mut Registry, name: &str, handle: &'static Block, wood: &str, tree: &str) {
    let side = format!("log_{wood}");
    let top = format!("log_{wood}_top");
    r.block(name).handle(handle)
        .set_text(&crate::worldgen::TREE, tree)
        .sounds("wood")
        .hardness(2.0)
        .mineable("axe")
        .tag("logs")
        .set(&FLAMMABLE, 5)
        .set(&FUEL, 300)
        .states(3, |i, s| match i {
            0 => s.textures(&top, &side, &top),
            1 => s.faces([&top, &top, &side, &side, &side, &side]),
            _ => s.faces([&side, &side, &side, &side, &top, &top]),
        })
        .placement(axis_placement)
        .tab("natural");
}

fn leaves(r: &mut Registry, name: &str, handle: &'static Block, tex: &str) {
    r.block(name).handle(handle)
        .sounds("grass")
        .texture(tex)
        .hardness(0.2)
        .cutout()
        .opacity(1)
        .tint_foliage()
        .tag("leaves")
        .mineable("shears")
        .tab("natural");
    // Vanilla: a sapling one time in 20, oak leaves an apple one time in 200.
    let table = r.loot(name).pool(Pool::new().when(Cond::Chance(0.05)).item("sapling"));
    if name == "oak_leaves" {
        table.pool(Pool::new().when(Cond::Chance(0.005)).item("apple"));
    }
}

fn plant(r: &mut Registry, name: &str, handle: &'static Block, tex: &str) {
    r.block(name).handle(handle).sounds("grass").texture(tex).plant().needs_below("plant_soil").tab("natural");
}

/// Sand and gravel: with nothing under them they fall as an entity and land as a block.
fn fall_tick(e: &mut BlockEvent) {
    let w = e.world();
    let p = e.pos();
    if state_flags(w.get([p[0], p[1] - 1, p[2]])) & bf::REPLACEABLE != 0 {
        w.fall(p);
    }
}

pub fn register(r: &mut Registry) {
    // harvest level N > 0 needs a matching tool of tier N-1 (wood 0, stone 1, iron 2, diamond 3).
    r.block("stone").handle(&STONE).sounds("stone").hardness(1.5).mineable("pickaxe").needs_tool(0).drops("cobblestone", 1).tag("stone").tag("stone_ore_replaceables").tab("building");
    r.block("cobblestone").handle(&COBBLESTONE).sounds("stone").hardness(2.0).mineable("pickaxe").needs_tool(0).tag("stone").tag("materials/stone").tab("building");
    r.block("mossy_cobblestone").handle(&MOSSY_COBBLESTONE).sounds("stone").hardness(2.0).mineable("pickaxe").needs_tool(0).tab("building");
    r.block("dirt").handle(&DIRT).sounds("gravel").hardness(0.5).mineable("shovel").tag("dirt").tab("natural");
    r.block("grass").handle(&GRASS).sounds("grass")
        .textures("grass_top", "grass_side", "dirt")
        .tint_grass_top()
        .hardness(0.6)
        .mineable("shovel")
        .drops("dirt", 1)
        .tag("dirt")
        .random_ticks(grass_tick).tab("natural");
    r.block("snowy_grass").handle(&SNOWY_GRASS).sounds("grass").textures("snow", "grass_snow_side", "dirt").hardness(0.6).mineable("shovel").drops("dirt", 1).tag("dirt").tab("natural");
    r.block("sand").handle(&SAND).sounds("sand").hardness(0.5).mineable("shovel").tag("sand").on_neighbor(|e| e.world().schedule(e.pos(), 2)).on_scheduled(fall_tick).tab("natural");
    r.block("gravel").handle(&GRAVEL).sounds("gravel").hardness(0.6).mineable("shovel").on_neighbor(|e| e.world().schedule(e.pos(), 2)).on_scheduled(fall_tick).tab("natural");
    r.block("clay").handle(&CLAY).sounds("gravel").hardness(0.6).mineable("shovel").tab("natural");
    r.block("sandstone").handle(&SANDSTONE).sounds("stone").textures("sandstone_top", "sandstone_side", "sandstone_bottom").hardness(0.8).mineable("pickaxe").needs_tool(0).tab("building");
    r.block("snow").handle(&SNOW).sounds("sand").hardness(0.2).mineable("shovel").tab("natural");
    r.block("ice").handle(&ICE).sounds("glass").hardness(0.5).translucent().opacity(2).friction(0.98).mineable("pickaxe").drops_nothing().tab("natural");
    r.block("obsidian").handle(&OBSIDIAN).sounds("stone").hardness(50.0).mineable("pickaxe").needs_tool(tool_tier::DIAMOND).tab("natural");
    r.block("coal_ore").handle(&COAL_ORE).sounds("stone").hardness(3.0).mineable("pickaxe").needs_tool(0).drops("coal", 1).tag("ores").tag("ores/coal").tab("natural");
    r.block("iron_ore").handle(&IRON_ORE).sounds("stone").hardness(3.0).mineable("pickaxe").needs_tool(tool_tier::STONE).drops("raw_iron", 1).tag("ores").tag("ores/iron").tab("natural");
    r.block("gold_ore").handle(&GOLD_ORE).sounds("stone").hardness(3.0).mineable("pickaxe").needs_tool(tool_tier::IRON).drops("raw_gold", 1).tag("ores").tag("ores/gold").tab("natural");
    r.block("diamond_ore").handle(&DIAMOND_ORE).sounds("stone").hardness(3.0).mineable("pickaxe").needs_tool(tool_tier::IRON).drops("diamond", 1).tag("ores").tag("ores/diamond").tab("natural");

    log(r, "oak_log", &OAK_LOG, "oak", "round base:oak_leaves");
    log(r, "birch_log", &BIRCH_LOG, "birch", "round base:birch_leaves");
    log(r, "spruce_log", &SPRUCE_LOG, "spruce", "cone base:spruce_leaves");
    for (name, h, tex) in [("oak_planks", &OAK_PLANKS, "planks_oak"), ("birch_planks", &BIRCH_PLANKS, "planks_birch"), ("spruce_planks", &SPRUCE_PLANKS, "planks_spruce")] {
        r.block(name).handle(h).sounds("wood").texture(tex).hardness(2.0).mineable("axe").tag("planks").tag("materials/wood").set(&FLAMMABLE, 5).set(&FUEL, 300).tab("building");
    }
    leaves(r, "oak_leaves", &OAK_LEAVES, "leaves_oak");
    leaves(r, "birch_leaves", &BIRCH_LEAVES, "leaves_birch");
    leaves(r, "spruce_leaves", &SPRUCE_LEAVES, "leaves_spruce");

    // Vanilla numbers: water steps every 5 ticks and reaches 7 blocks, lava every 30 and 3.
    r.liquid("water").handle(&WATER).texture("water_still").tint_water().tag("water").bucket("water_bucket", "bucket", "tools");
    r.liquid("lava").handle(&LAVA).texture("lava_still").light(15).drag(0.3).delay(30).drop_off(2).slope_find(2).infinite(0).temperature(1300).tag("lava").tag("hot").bucket("lava_bucket", "bucket", "tools");
    r.liquid_reaction("#hot", "#water").source("obsidian").flowing("cobblestone").falls_into("stone");

    r.block("tall_grass").handle(&TALL_GRASS).sounds("grass").texture("tallgrass").plant().needs_below("plant_soil").tint_grass().no_item();
    r.loot("tall_grass").pool(Pool::new().when(Cond::Chance(0.125)).item("wheat_seeds"));
    plant(r, "dandelion", &DANDELION, "flower_dandelion");
    plant(r, "rose", &ROSE, "flower_rose");
    r.block("sapling").handle(&SAPLING).sounds("grass").texture("sapling_oak").plant().needs_below("plant_soil").grows(470.0, 9).on_grow(|e| worldgen::grow_tree(e.world(), e.pos())).tab("natural");
    r.block("sugar_cane").handle(&SUGAR_CANE).sounds("grass").texture("reeds").plant().needs_below("sugar_cane_soil").needs_near_support("water").grows(270.0, 0).on_grow(|e| grow_column(e, &SUGAR_CANE)).tab("natural");
    r.block("cactus").handle(&CACTUS).sounds("cloth")
        .textures("cactus_top", "cactus_side", "cactus_top")
        .model_box_faces([1.0, 0.0, 1.0], [15.0, 16.0, 15.0], ["cactus_side", "cactus_side", "cactus_top", "cactus_top", "cactus_side", "cactus_side"])
        .hardness(0.4)
        .needs_below("cactus_soil")
        .grows(270.0, 0).on_grow(|e| grow_column(e, &CACTUS)).tab("natural");
    r.block("wheat").handle(&WHEAT).sounds("grass")
        .states(8, |i, s| s.texture(&format!("wheat_stage_{i}")))
        .plant()
        .needs_below("crop_soil")
        .no_item()
        .drops("wheat", 1)
        .grows(205.0, 9);
    r.block("farmland").handle(&FARMLAND).sounds("gravel")
        .textures("farmland", "dirt", "dirt")
        .model_box_faces([0.0, 0.0, 0.0], [16.0, 15.0, 16.0], ["dirt", "dirt", "dirt", "farmland", "dirt", "dirt"])
        .flags(0, 0)
        .hardness(0.6)
        .mineable("shovel")
        .drops("dirt", 1)
        .tab("natural");

    r.block("glass").handle(&GLASS).sounds("glass").cutout().cull_same().hardness(0.3).drops_nothing().tab("building");
    for (name, col) in [("red", [0u8, 12, 12]), ("green", [12, 0, 12]), ("blue", [12, 12, 0])] {
        r.block(&format!("{name}_stained_glass")).sounds("glass").texture(&format!("glass_{name}")).translucent().filter(col[0], col[1], col[2]).hardness(0.3).tab("colored");
    }
    for (name, rgb) in [("red", [15u8, 3, 2]), ("green", [2, 15, 4]), ("blue", [3, 6, 15])] {
        r.block(&format!("{name}_lamp")).sounds("glass").texture(&format!("lamp_{name}")).light_rgb(rgb[0], rgb[1], rgb[2]).hardness(0.3).tab("colored");
    }
    r.block("crafting_table").handle(&CRAFTING_TABLE).sounds("wood")
        .faces(["crafting_table_side", "crafting_table_side", "planks_oak", "crafting_table_top", "crafting_table_front", "crafting_table_front"])
        .hardness(2.5)
        .mineable("axe")
        .set(&FUEL, 300)
        .on_use(crate::machines::open_crafting).tab("functional");
    r.block("torch").handle(&TORCH).sounds("wood")
        .facing(TORCH_FACINGS, Toward::Face, |d, s| torch_state(d, s, "torch"))
        .item_texture("torch")
        .not_solid()
        .light_rgb(15, 13, 10)
        .hardness(0.0)
        .tab("functional");
    // Facing away from the solid wall it hangs on.
    r.block("ladder").handle(&LADDER).sounds("wood")
        .facing(Facings::HORIZONTAL, Toward::Face, |d, s| {
            let (min, max) = match d {
                Dir::South => ([0.0, 0.0, 0.0], [16.0, 16.0, 1.0]),
                Dir::North => ([0.0, 0.0, 15.0], [16.0, 16.0, 16.0]),
                Dir::East => ([0.0, 0.0, 0.0], [1.0, 16.0, 16.0]),
                _ => ([15.0, 0.0, 0.0], [16.0, 16.0, 16.0]),
            };
            s.model_box(min, max, "ladder").needs(d.opposite().face(), "")
        })
        .climbable()
        .not_solid()
        .hardness(0.4)
        .tab("functional");
    r.block("glowstone").handle(&GLOWSTONE).sounds("glass").light(15).hardness(0.3).tab("natural");
    r.block("bricks").handle(&BRICKS).sounds("stone").hardness(2.0).mineable("pickaxe").needs_tool(0).tab("building");
    r.block("stone_bricks").handle(&STONE_BRICKS).sounds("stone").texture("stonebrick").hardness(1.5).mineable("pickaxe").needs_tool(0).tab("building");
    r.block("iron_block").handle(&IRON_BLOCK).sounds("stone").hardness(5.0).mineable("pickaxe").needs_tool(tool_tier::STONE).tag("ore_blocks/iron").tab("building");
    r.block("gold_block").handle(&GOLD_BLOCK).sounds("stone").hardness(3.0).mineable("pickaxe").needs_tool(tool_tier::IRON).tag("ore_blocks/gold").tab("building");
    r.block("diamond_block").handle(&DIAMOND_BLOCK).sounds("stone").hardness(5.0).mineable("pickaxe").needs_tool(tool_tier::IRON).tag("ore_blocks/diamond").tab("building");
    r.block("coal_block").handle(&COAL_BLOCK).sounds("stone").hardness(5.0).mineable("pickaxe").needs_tool(0).tag("ore_blocks/coal").set(&FUEL, 16000).tab("building");
    r.block("wool").handle(&WOOL).sounds("cloth").texture("wool_white").hardness(0.8).set(&FLAMMABLE, 30).tab("colored");

    // What may grow or stand where, from the common `dirt` and `sand` tags like modern vanilla:
    // a mod's sand tagged `sand` gets cactus and sugar cane, in the world and from players.
    r.tag_on(Target::Tagged("dirt"), "plant_soil");
    r.tag("plant_soil", &["farmland"]);
    r.tag_on(Target::Tagged("dirt"), "sugar_cane_soil");
    r.tag_on(Target::Tagged("sand"), "sugar_cane_soil");
    r.tag("sugar_cane_soil", &["sugar_cane"]);
    r.tag_on(Target::Tagged("sand"), "cactus_soil");
    r.tag("cactus_soil", &["cactus"]);
    r.tag("crop_soil", &["farmland"]);
    r.tag("grass_spreads_to", &["dirt"]);
    r.tag("animal_spawn", &["grass"]);
}

fn grass_tick(e: &mut BlockEvent) {
    let w = e.world();
    let p = e.pos();
    if state_flags(w.get([p[0], p[1] + 1, p[2]])) & bf::OPAQUE != 0 {
        w.set(p, DIRT.id());
        return;
    }
    let r = w.random();
    let t = [p[0] + (r % 3) as i32 - 1, p[1] + ((r >> 8) % 5) as i32 - 3, p[2] + ((r >> 16) % 3) as i32 - 1];
    if GRASS_SPREADS_TO.on_block(w.get(t)) && state_flags(w.get([t[0], t[1] + 1, t[2]])) & bf::OPAQUE == 0 {
        w.set(t, GRASS.id());
    }
}

/// Torches stand on the floor or hang on a wall, facing away from it.
pub const TORCH_FACINGS: Facings = Facings::only(&[Dir::Up, Dir::North, Dir::East, Dir::South, Dir::West]);

/// A torch facing `d`: on the floor (`Up`) or raised against the wall behind it.
// ponytail: wall torches stand straight; vanilla's lean needs rotated block boxes.
pub fn torch_state<'a>(d: Dir, s: State<'a>, texture: &str) -> State<'a> {
    let (min, max) = match d {
        Dir::South => ([7.0, 3.0, 0.0], [9.0, 13.0, 2.0]),
        Dir::West => ([14.0, 3.0, 7.0], [16.0, 13.0, 9.0]),
        Dir::North => ([7.0, 3.0, 14.0], [9.0, 13.0, 16.0]),
        Dir::East => ([0.0, 3.0, 7.0], [2.0, 13.0, 9.0]),
        _ => ([7.0, 0.0, 7.0], [9.0, 10.0, 9.0]),
    };
    s.model_box(min, max, texture).needs(d.opposite().face(), "")
}

/// Sugar cane and cacti grow a block on top, up to three high.
fn grow_column(e: &mut BlockEvent, me: &'static Block) {
    let w = e.world();
    let p = e.pos();
    let above = [p[0], p[1] + 1, p[2]];
    if w.get(above) != 0 {
        return;
    }
    let mut height = 1;
    while height < 3 && me.is(w.get([p[0], p[1] - height, p[2]])) {
        height += 1;
    }
    if height < 3 {
        w.set(above, me.id());
    }
}


