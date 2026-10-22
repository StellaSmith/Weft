use voxide_sdk::abi::block_flags as bf;
use voxide_sdk::*;

use crate::worldgen;

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

const HORIZONTAL: [[i32; 3]; 4] = [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]];

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

/// Log axis from the clicked face: 0 = Y, 1 = X, 2 = Z.
fn axis_placement(p: &Place) -> u32 {
    match p.face() {
        0 | 1 => 1,
        4 | 5 => 2,
        _ => 0,
    }
}

fn log(r: &mut Registry, name: &str, handle: &'static Block, wood: &str) {
    let side = format!("log_{wood}");
    let top = format!("log_{wood}_top");
    r.block(name, handle)
        .hardness(2.0)
        .tool("axe", 0)
        .tag("base:logs")
        .prop("base:flammable", 5u32)
        .prop("base:fuel", 300u32)
        .states(3, |i, s| match i {
            0 => s.textures(&top, &side, &top),
            1 => s.faces([&top, &top, &side, &side, &side, &side]),
            _ => s.faces([&side, &side, &side, &side, &top, &top]),
        })
        .placement(axis_placement);
}

fn leaves(r: &mut Registry, name: &str, handle: &'static Block, tex: &str) {
    r.block(name, handle)
        .texture(tex)
        .hardness(0.2)
        .cutout()
        .opacity(1)
        .tint_foliage()
        .tag("base:leaves")
        .drops("sapling", 1)
        .tool("shears", 0);
}

/// Liquid states: level 0..7 (0 = source) plus 8 for "falling".
fn liquid_tick(w: World, p: [i32; 3], me: &'static Block, step: u32, infinite: bool) {
    let state = w.get(p);
    if !me.is(state) {
        return;
    }
    let idx = me.index(state);
    let (level, _falling) = (idx & 7, idx & 8 != 0);

    if me.id() == LAVA.id() || me.id() == WATER.id() {
        let other = if me.id() == LAVA.id() { &WATER } else { &LAVA };
        for d in [[0, 1, 0], [0, -1, 0], [1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]] {
            if other.is(w.get(add(p, d))) {
                if me.id() == LAVA.id() {
                    w.set(p, if level == 0 { OBSIDIAN.id() } else { COBBLESTONE.id() });
                    return;
                }
            }
        }
    }

    if level != 0 || idx & 8 != 0 {
        let above = w.get(add(p, [0, 1, 0]));
        let expected = if me.is(above) {
            Some(8)
        } else {
            let mut best: Option<u32> = None;
            let mut sources = 0;
            for d in HORIZONTAL {
                let n = w.get(add(p, d));
                if me.is(n) {
                    let ni = me.index(n);
                    if ni == 0 {
                        sources += 1;
                    }
                    let nl = if ni & 8 != 0 { 0 } else { ni & 7 };
                    let cand = nl + step;
                    best = Some(best.map_or(cand, |b: u32| b.min(cand)));
                }
            }
            let below = w.get(add(p, [0, -1, 0]));
            if infinite && sources >= 2 && (state_flags(below) & bf::SOLID != 0 || me.is(below) && me.index(below) == 0) {
                Some(0)
            } else {
                best.filter(|&b| b <= 7)
            }
        };
        match expected {
            None => {
                w.set(p, 0);
                return;
            }
            Some(e) if e != idx => {
                w.set(p, me.state(e));
                return;
            }
            _ => {}
        }
    }

    let below = add(p, [0, -1, 0]);
    let bs = w.get(below);
    let can_fall = state_flags(bs) & bf::REPLACEABLE != 0 && !me.is(bs);
    if can_fall {
        w.set(below, me.state(8));
        if level != 0 {
            return;
        }
    }
    let next = if idx & 8 != 0 { step } else { level + step };
    if next > 7 {
        return;
    }
    if !can_fall || level == 0 {
        // Flowing liquid sitting on its own kind does not spread sideways.
        if me.is(bs) && level != 0 && idx & 8 == 0 {
            return;
        }
        for d in HORIZONTAL {
            let n = add(p, d);
            let ns = w.get(n);
            if me.is(ns) {
                let ni = me.index(ns);
                if ni & 8 == 0 && (ni & 7) > next {
                    w.set(n, me.state(next));
                }
            } else if state_flags(ns) & bf::REPLACEABLE != 0 && state_flags(ns) & bf::LIQUID == 0 {
                w.set(n, me.state(next));
            }
        }
    }
}

fn liquid(r: &mut Registry, name: &str, handle: &'static Block, still: &str, flow_delay: u32, step: u32, infinite: bool, light: u8) {
    let b = r
        .block(name, handle)
        .texture(still)
        .states(16, |i, s| s.liquid(if i >= 8 { 0 } else { i as u8 }))
        .light(light)
        .no_item()
        .drops_nothing()
        .on_neighbor(move |e| e.world().schedule(e.pos(), flow_delay))
        .on_placed(move |e| e.world().schedule(e.pos(), flow_delay))
        .on_scheduled(move |e| liquid_tick(e.world(), e.pos(), handle, step, infinite));
    if name == "water" {
        b.tint_water().opacity(2);
    } else {
        b.drag(0.3);
    }
}

fn plant(r: &mut Registry, name: &str, handle: &'static Block, tex: &str) {
    r.block(name, handle).texture(tex).plant().on_neighbor(|e| {
        let w = e.world();
        let p = e.pos();
        let below = w.get([p[0], p[1] - 1, p[2]]);
        if !(GRASS.is(below) || DIRT.is(below) || SNOWY_GRASS.is(below) || FARMLAND.is(below)) {
            w.set(p, 0);
        }
    });
}

pub fn register(r: &mut Registry) {
    // harvest level N > 0 needs a matching tool of tier N-1 (wood 0, stone 1, iron 2, diamond 3).
    r.block("stone", &STONE).hardness(1.5).tool("pickaxe", 1).drops("cobblestone", 1).tag("base:stone");
    r.block("cobblestone", &COBBLESTONE).hardness(2.0).tool("pickaxe", 1).tag("base:stone");
    r.block("mossy_cobblestone", &MOSSY_COBBLESTONE).hardness(2.0).tool("pickaxe", 1);
    r.block("dirt", &DIRT).hardness(0.5).tool("shovel", 0);
    r.block("grass", &GRASS)
        .textures("grass_top", "grass_side", "dirt")
        .tint_grass_top()
        .hardness(0.6)
        .tool("shovel", 0)
        .drops("dirt", 1)
        .random_ticks(grass_tick);
    r.block("snowy_grass", &SNOWY_GRASS).textures("snow", "grass_snow_side", "dirt").hardness(0.6).tool("shovel", 0).drops("dirt", 1);
    r.block("sand", &SAND).hardness(0.5).tool("shovel", 0);
    r.block("gravel", &GRAVEL).hardness(0.6).tool("shovel", 0);
    r.block("clay", &CLAY).hardness(0.6).tool("shovel", 0);
    r.block("sandstone", &SANDSTONE).textures("sandstone_top", "sandstone_side", "sandstone_bottom").hardness(0.8).tool("pickaxe", 1);
    r.block("snow", &SNOW).hardness(0.2).tool("shovel", 0);
    r.block("ice", &ICE).hardness(0.5).translucent().opacity(2).friction(0.98).tool("pickaxe", 0).drops_nothing();
    r.block("obsidian", &OBSIDIAN).hardness(50.0).tool("pickaxe", 4);
    r.block("coal_ore", &COAL_ORE).hardness(3.0).tool("pickaxe", 1).drops("coal", 1).tag("base:ores");
    r.block("iron_ore", &IRON_ORE).hardness(3.0).tool("pickaxe", 2).tag("base:ores");
    r.block("gold_ore", &GOLD_ORE).hardness(3.0).tool("pickaxe", 3).tag("base:ores");
    r.block("diamond_ore", &DIAMOND_ORE).hardness(3.0).tool("pickaxe", 3).drops("diamond", 1).tag("base:ores");

    log(r, "oak_log", &OAK_LOG, "oak");
    log(r, "birch_log", &BIRCH_LOG, "birch");
    log(r, "spruce_log", &SPRUCE_LOG, "spruce");
    for (name, h, tex) in [("oak_planks", &OAK_PLANKS, "planks_oak"), ("birch_planks", &BIRCH_PLANKS, "planks_birch"), ("spruce_planks", &SPRUCE_PLANKS, "planks_spruce")] {
        r.block(name, h).texture(tex).hardness(2.0).tool("axe", 0).tag("base:planks").prop("base:flammable", 5u32).prop("base:fuel", 300u32);
    }
    leaves(r, "oak_leaves", &OAK_LEAVES, "leaves_oak");
    leaves(r, "birch_leaves", &BIRCH_LEAVES, "leaves_birch");
    leaves(r, "spruce_leaves", &SPRUCE_LEAVES, "leaves_spruce");

    liquid(r, "water", &WATER, "water_still", 5, 1, true, 0);
    liquid(r, "lava", &LAVA, "lava_still", 30, 2, false, 15);

    r.block("tall_grass", &TALL_GRASS).texture("tallgrass").plant().tint_grass().drops("wheat_seeds", 1).no_item();
    plant(r, "dandelion", &DANDELION, "flower_dandelion");
    plant(r, "rose", &ROSE, "flower_rose");
    r.block("sapling", &SAPLING).texture("sapling_oak").plant().random_ticks(|e| grow_sapling(e.world(), e.pos()));
    r.block("sugar_cane", &SUGAR_CANE).texture("reeds").plant().random_ticks(|e| grow_column(e, &SUGAR_CANE));
    r.block("cactus", &CACTUS)
        .textures("cactus_top", "cactus_side", "cactus_top")
        .model_box_faces([1.0, 0.0, 1.0], [15.0, 16.0, 15.0], ["cactus_side", "cactus_side", "cactus_top", "cactus_top", "cactus_side", "cactus_side"])
        .hardness(0.4)
        .random_ticks(|e| grow_column(e, &CACTUS));
    r.block("wheat", &WHEAT)
        .states(8, |i, s| s.texture(&format!("wheat_stage_{i}")))
        .plant()
        .no_item()
        .drops("wheat", 1)
        .random_ticks(|e| {
            let w = e.world();
            let i = WHEAT.index(e.state());
            if i < 7 && w.random_below(3) == 0 {
                w.set(e.pos(), WHEAT.state(i + 1));
            }
        });
    r.block("farmland", &FARMLAND)
        .textures("farmland", "dirt", "dirt")
        .model_box_faces([0.0, 0.0, 0.0], [16.0, 15.0, 16.0], ["dirt", "dirt", "dirt", "farmland", "dirt", "dirt"])
        .flags(0, 0)
        .hardness(0.6)
        .tool("shovel", 0)
        .drops("dirt", 1);

    r.block("glass", &GLASS).cutout().hardness(0.3).drops_nothing();
    for (name, col) in [("red", [0u8, 12, 12]), ("green", [12, 0, 12]), ("blue", [12, 12, 0])] {
        r.block(&format!("{name}_stained_glass"), leak()).texture(&format!("glass_{name}")).translucent().filter(col[0], col[1], col[2]).hardness(0.3);
    }
    for (name, rgb) in [("red", [15u8, 3, 2]), ("green", [2, 15, 4]), ("blue", [3, 6, 15])] {
        r.block(&format!("{name}_lamp"), leak()).texture(&format!("lamp_{name}")).light_rgb(rgb[0], rgb[1], rgb[2]).hardness(0.3);
    }
    r.block("crafting_table", &CRAFTING_TABLE)
        .faces(["crafting_table_side", "crafting_table_side", "planks_oak", "crafting_table_top", "crafting_table_front", "crafting_table_front"])
        .hardness(2.5)
        .tool("axe", 0)
        .prop("base:fuel", 300u32)
        .on_use(crate::machines::open_crafting);
    r.block("furnace", &FURNACE)
        .states(8, |i, s| {
            let facing = i % 4;
            let lit = i >= 4;
            let front = if lit { "furnace_front_lit" } else { "furnace_front" };
            let mut f = ["furnace_side"; 6];
            f[2] = "furnace_top";
            f[3] = "furnace_top";
            f[[4, 1, 5, 0][facing as usize]] = front;
            let s = s.faces(f);
            if lit { s.light_rgb(13, 11, 8) } else { s }
        })
        .hardness(3.5)
        .tool("pickaxe", 1)
        .placement(|p| (p.horizontal() + 2) % 4)
        .on_use(crate::machines::open_on_use(&crate::machines::FURNACE_MENU));
    r.block("chest", &CHEST)
        .states(4, |i, s| {
            let mut f = ["chest_side"; 6];
            f[2] = "chest_top";
            f[3] = "chest_top";
            f[[4, 1, 5, 0][i as usize]] = "chest_front";
            s.model_box_faces([1.0, 0.0, 1.0], [15.0, 14.0, 15.0], f)
        })
        .hardness(2.5)
        .tool("axe", 0)
        .placement(|p| (p.horizontal() + 2) % 4)
        .on_use(crate::machines::open_on_use(&crate::machines::CHEST_MENU));
    r.block("torch", &TORCH)
        .model_box([7.0, 0.0, 7.0], [9.0, 10.0, 9.0], "torch")
        .not_solid()
        .light_rgb(15, 13, 10)
        .hardness(0.0)
        .on_neighbor(|e| {
            let w = e.world();
            let p = e.pos();
            if state_flags(w.get([p[0], p[1] - 1, p[2]])) & bf::SOLID == 0 {
                w.set(p, 0);
            }
        });
    r.block("ladder", &LADDER).model_box([0.0, 0.0, 15.0], [16.0, 16.0, 16.0], "ladder").climbable().not_solid().hardness(0.4);
    r.block("glowstone", &GLOWSTONE).light(15).hardness(0.3);
    r.block("bricks", &BRICKS).hardness(2.0).tool("pickaxe", 1);
    r.block("stone_bricks", &STONE_BRICKS).texture("stonebrick").hardness(1.5).tool("pickaxe", 1);
    r.block("iron_block", &IRON_BLOCK).hardness(5.0).tool("pickaxe", 2);
    r.block("gold_block", &GOLD_BLOCK).hardness(3.0).tool("pickaxe", 3);
    r.block("diamond_block", &DIAMOND_BLOCK).hardness(5.0).tool("pickaxe", 3);
    r.block("coal_block", &COAL_BLOCK).hardness(5.0).tool("pickaxe", 1).prop("base:fuel", 16000u32);
    r.block("wool", &WOOL).texture("wool_white").hardness(0.8).prop("base:flammable", 30u32);
}

/// Handles for blocks nothing refers to directly.
fn leak() -> &'static Block {
    Box::leak(Box::new(Block::new()))
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
    if DIRT.is(w.get(t)) && state_flags(w.get([t[0], t[1] + 1, t[2]])) & bf::OPAQUE == 0 {
        w.set(t, GRASS.id());
    }
}

fn grow_column(e: &mut BlockEvent, me: &'static Block) {
    let w = e.world();
    let p = e.pos();
    let above = [p[0], p[1] + 1, p[2]];
    if w.get(above) != 0 || w.random_below(4) != 0 {
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

pub fn grow_sapling(w: World, p: [i32; 3]) {
    if w.random_below(7) == 0 {
        worldgen::grow_tree(w, p);
    }
}
