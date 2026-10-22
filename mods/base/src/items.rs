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
    r.item("stick", &STICK).prop(FUEL, 100 as Fuel);
    r.item("coal", &COAL).prop(FUEL, 1600 as Fuel);
    r.item("iron_ingot", &IRON_INGOT).tag("ingots/iron");
    r.item("gold_ingot", &GOLD_INGOT).tag("ingots/gold");
    r.item("diamond", &DIAMOND).tag("gems/diamond");
    r.item("wheat_seeds", &WHEAT_SEEDS).places("wheat");
    r.item("wheat", &WHEAT);
    r.item("bucket", &BUCKET).stack(16);
    r.item("water_bucket", &WATER_BUCKET).stack(1);
    r.item("lava_bucket", &LAVA_BUCKET).stack(1).prop(FUEL, 20000 as Fuel);

    for (name, hunger, saturation) in [
        ("apple", 4, 2.4),
        ("bread", 5, 6.0),
        ("porkchop", 3, 1.8),
        ("cooked_porkchop", 8, 12.8),
        ("beef", 3, 1.8),
        ("cooked_beef", 8, 12.8),
        ("rotten_flesh", 4, 0.8),
    ] {
        r.item(name, leak()).prop(FOOD, Food { hunger, saturation }).on_use(crate::player::eat);
    }
    for name in ["leather", "bone", "string", "sugar"] {
        r.item(name, leak());
    }

    for m in &MATERIALS {
        let tool = |r: &mut Registry, kind: &str, class: &str, damage: f32| {
            r.item(&format!("{}_{kind}", m.name), leak())
                .tool(class, m.level, m.speed)
                .durability(m.durability)
                .attack(damage + m.damage)
                .tag(&format!("tools/{kind}"));
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
            let tag = ["armor/head", "armor/chest", "armor/legs", "armor/feet"][slot as usize];
            r.item(&format!("{mat}_{piece}"), leak()).durability(durability * dur).prop(ARMOR, Armor { slot, points }).tag(tag);
        }
    }
}

fn leak() -> &'static Item {
    Box::leak(Box::new(Item::new()))
}
