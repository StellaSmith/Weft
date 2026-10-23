use voxide_sdk::*;

pub static STICK: Item = Item::new();
pub static COAL: Item = Item::new();
pub static IRON_INGOT: Item = Item::new();
pub static GOLD_INGOT: Item = Item::new();
pub static DIAMOND: Item = Item::new();
pub static WHEAT_SEEDS: Item = Item::new();
pub static WHEAT: Item = Item::new();
pub static BUCKET: Item = Item::new();
pub static WATER_BUCKET: Item = Item::new();
pub static LAVA_BUCKET: Item = Item::new();

/// Read by the hunger system; other mods attach it to their own food.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Food {
    pub hunger: u32,
    pub saturation: f32,
}

/// Burn time in ticks, read by the furnace.
pub type Fuel = u32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Armor {
    /// 0 head, 1 chest, 2 legs, 3 feet.
    pub slot: u32,
    pub points: u32,
}

pub const FOOD: &str = "base:food";
pub const FUEL: &str = "base:fuel";
pub const ARMOR: &str = "base:armor";
/// Slot tags; `<slot>/<material>` tags pick the recipe (see recipes.rs).
pub const ARMOR_SLOTS: [&str; 4] = ["armor_head", "armor_chest", "armor_legs", "armor_feet"];

struct Material {
    name: &'static str,
    level: u32,
    speed: f32,
    durability: u32,
    damage: f32,
}

const MATERIALS: [Material; 5] = [
    Material { name: "wood", level: 0, speed: 2.0, durability: 59, damage: 0.0 },
    Material { name: "stone", level: 1, speed: 4.0, durability: 131, damage: 1.0 },
    Material { name: "iron", level: 2, speed: 6.0, durability: 250, damage: 2.0 },
    Material { name: "diamond", level: 3, speed: 8.0, durability: 1561, damage: 3.0 },
    Material { name: "gold", level: 0, speed: 12.0, durability: 32, damage: 0.0 },
];

pub fn register(r: &mut Registry) {
    r.item("stick", &STICK).prop(FUEL, 100 as Fuel).tab("ingredients");
    r.item("coal", &COAL).tag("gems/coal").prop(FUEL, 1600 as Fuel).tab("ingredients");
    r.item("iron_ingot", &IRON_INGOT).tag("ingots/iron").tag("materials/iron").tab("ingredients");
    r.item("gold_ingot", &GOLD_INGOT).tag("ingots/gold").tag("materials/gold").tab("ingredients");
    // Metal ores drop raw pieces (like modern versions) that smelt into ingots.
    r.item("raw_iron", leak()).tag("raw_ores/iron").tab("ingredients");
    r.item("raw_gold", leak()).tag("raw_ores/gold").tab("ingredients");
    r.item("diamond", &DIAMOND).tag("gems/diamond").tag("materials/diamond").tab("ingredients");
    r.item("wheat_seeds", &WHEAT_SEEDS).places("wheat").tab("natural");
    r.item("wheat", &WHEAT).tab("ingredients");
    r.item("bucket", &BUCKET).stack(16).on_use(fill_bucket).tab("tools");
    r.item("water_bucket", &WATER_BUCKET).stack(1).on_use(|u| pour_bucket(u, &crate::blocks::WATER)).tab("tools");
    r.item("lava_bucket", &LAVA_BUCKET).stack(1).prop(FUEL, 20000 as Fuel).on_use(|u| pour_bucket(u, &crate::blocks::LAVA)).tab("tools");

    for (name, hunger, saturation, tag) in [
        ("apple", 4, 2.4, ""),
        ("bread", 5, 6.0, ""),
        ("porkchop", 3, 1.8, "raw_meat/pork"),
        ("cooked_porkchop", 8, 12.8, "cooked_meat/pork"),
        ("beef", 3, 1.8, "raw_meat/beef"),
        ("cooked_beef", 8, 12.8, "cooked_meat/beef"),
        ("rotten_flesh", 4, 0.8, ""),
    ] {
        let i = r.item(name, leak()).prop(FOOD, Food { hunger, saturation }).on_use(crate::player::eat).tab("food");
        if !tag.is_empty() {
            i.tag(tag);
        }
    }
    r.item("leather", leak()).tag("materials/leather").tab("ingredients");
    for name in ["bone", "string", "sugar"] {
        r.item(name, leak()).tab("ingredients");
    }

    for m in &MATERIALS {
        let tool = |r: &mut Registry, kind: &str, class: &str, damage: f32| {
            r.item(&format!("{}_{kind}", m.name), leak())
                .tool(class, m.level, m.speed)
                .durability(m.durability)
                .attack(damage + m.damage)
                .tag(&format!("tools/{kind}"))
                .tag(&format!("{kind}s/{}", m.name))
                .tab(if kind == "sword" { "combat" } else { "tools" });
        };
        tool(r, "pickaxe", "pickaxe", 2.0);
        tool(r, "axe", "axe", 3.0);
        tool(r, "shovel", "shovel", 1.0);
        tool(r, "hoe", "hoe", 1.0);
        tool(r, "sword", "sword", 4.0);
    }

    for (mat, mult, durability) in [("leather", 0u32, 5u32), ("iron", 1, 15), ("gold", 1, 7), ("diamond", 2, 33)] {
        for (slot, piece, points, dur) in [(0, "helmet", 1, 11), (1, "chestplate", 3, 16), (2, "leggings", 2, 15), (3, "boots", 1, 13)] {
            let points = points + mult * (if slot == 1 { 3 } else { 1 });
            let tag = ARMOR_SLOTS[slot as usize];
            let (parts, inflate, layer) = [(worn_parts::HEAD, 1.0, 1), (worn_parts::BODY | worn_parts::ARMS, 1.0, 1), (worn_parts::BODY | worn_parts::LEGS, 0.5, 2), (worn_parts::LEGS, 1.0, 1)][slot as usize];
            r.item(&format!("{mat}_{piece}"), leak())
                .durability(durability * dur)
                .prop(ARMOR, Armor { slot, points })
                .tag(tag)
                .tag(&format!("{tag}/{mat}"))
                .worn(parts, inflate, &format!("armor/{mat}_{layer}"))
                .tab("combat");
        }
    }
}

/// Scoops up the liquid source looked at.
fn fill_bucket(u: &mut ItemUse) -> bool {
    use crate::blocks::{LAVA, WATER};
    let (w, player, stack) = (u.world(), u.player(), u.stack());
    let Some((pos, _, state)) = w.look_at(player, 5.0, true) else { return false };
    let full = match () {
        _ if WATER.is(state) && WATER.index(state) == 0 => &WATER_BUCKET,
        _ if LAVA.is(state) && LAVA.index(state) == 0 => &LAVA_BUCKET,
        _ => return false,
    };
    w.set(pos, 0);
    if w.creative(player) {
        return true;
    }
    let filled = ItemStack::new(full.id(), 1);
    if stack.count == 1 {
        u.set_stack(filled);
    } else {
        u.set_stack(ItemStack { count: stack.count - 1, ..stack });
        let rest = w.inventory(player).insert(filled);
        if !rest.is_empty() {
            if let Some(e) = w.entity(player) {
                w.spawn_item(e.pos, rest);
            }
        }
    }
    true
}

/// Pours a source block into the block looked at, or next to it on the face looked at.
fn pour_bucket(u: &mut ItemUse, liquid: &Block) -> bool {
    const DIRS: [[i32; 3]; 6] = [[-1, 0, 0], [1, 0, 0], [0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1]];
    let (w, player) = (u.world(), u.player());
    let Some((pos, face, state)) = w.look_at(player, 5.0, false) else { return false };
    let replaceable = |s: u32| state_flags(s) & abi::block_flags::REPLACEABLE != 0;
    let target = if replaceable(state) {
        pos
    } else {
        let d = DIRS[face as usize % 6];
        [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]]
    };
    if !replaceable(w.get(target)) {
        return false;
    }
    w.set(target, liquid.state(0));
    if !w.creative(player) {
        u.set_stack(ItemStack::new(BUCKET.id(), 1));
    }
    true
}

fn leak() -> &'static Item {
    Box::leak(Box::new(Item::new()))
}
