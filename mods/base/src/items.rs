use voxide_sdk::*;

pub static STICK: Item = Item::new();
pub static COAL: Item = Item::new();
pub static IRON_INGOT: Item = Item::new();
pub static GOLD_INGOT: Item = Item::new();
pub static DIAMOND: Item = Item::new();
pub static WHEAT_SEEDS: Item = Item::new();
pub static WHEAT: Item = Item::new();
pub static BUCKET: Item = Item::new();

value! {
    /// Read by the hunger system; other mods set it on their own food.
    pub struct Food { pub hunger: u32, pub saturation: f32 }
}

value! {
    /// `slot` 0 head, 1 chest, 2 legs, 3 feet.
    pub struct Armor { pub slot: u32, pub points: u32 }
}

// The common vocabulary the base game reads; any mod may set these on its own content.
pub static FOOD: Component<Food> = Component::new("food");
/// Burn time in ticks.
pub static FUEL: Component<u32> = Component::new("fuel");
pub static ARMOR: Component<Armor> = Component::new("armor");
/// How readily fire takes it.
pub static FLAMMABLE: Component<u32> = Component::new("flammable");
/// Item left behind when this one is used up as fuel or ingredient (the bucket of a lava bucket).
/// Slot tags; `<slot>/<material>` tags pick the recipe (see recipes.rs).
pub const ARMOR_SLOTS: [&str; 4] = abi::comp::ARMOR;

struct Material {
    name: &'static str,
    level: u32,
    speed: f32,
    durability: u32,
    damage: f32,
}

const MATERIALS: [Material; 5] = [
    Material { name: "wood", level: tool_tier::WOOD, speed: 2.0, durability: 59, damage: 0.0 },
    Material { name: "stone", level: tool_tier::STONE, speed: 4.0, durability: 131, damage: 1.0 },
    Material { name: "iron", level: tool_tier::IRON, speed: 6.0, durability: 250, damage: 2.0 },
    Material { name: "diamond", level: tool_tier::DIAMOND, speed: 8.0, durability: 1561, damage: 3.0 },
    Material { name: "gold", level: tool_tier::WOOD, speed: 12.0, durability: 32, damage: 0.0 },
];

pub fn register(r: &mut Registry) {
    r.item("stick").handle(&STICK).set(&FUEL, 100).tab("ingredients");
    r.item("coal").handle(&COAL).tag("gems/coal").set(&FUEL, 1600).tab("ingredients");
    r.item("iron_ingot").handle(&IRON_INGOT).tag("ingots/iron").tag("materials/iron").tab("ingredients");
    r.item("gold_ingot").handle(&GOLD_INGOT).tag("ingots/gold").tag("materials/gold").tab("ingredients");
    // Metal ores drop raw pieces (like modern versions) that smelt into ingots.
    r.item("raw_iron").tag("raw_ores/iron").tab("ingredients");
    r.item("raw_gold").tag("raw_ores/gold").tab("ingredients");
    r.item("diamond").handle(&DIAMOND).tag("gems/diamond").tag("materials/diamond").tab("ingredients");
    r.item("wheat_seeds").handle(&WHEAT_SEEDS).places("wheat").tab("natural");
    r.item("wheat").handle(&WHEAT).tab("ingredients");
    // Filled buckets come with their liquids (blocks.rs); the engine fills and pours them.
    r.item("bucket").handle(&BUCKET).stack(16).tab("tools");
    r.set("lava_bucket", &FUEL, 20000);

    for (name, hunger, saturation, tag) in [
        ("apple", 4, 2.4, ""),
        ("bread", 5, 6.0, ""),
        ("porkchop", 3, 1.8, "raw_meat/pork"),
        ("cooked_porkchop", 8, 12.8, "cooked_meat/pork"),
        ("beef", 3, 1.8, "raw_meat/beef"),
        ("cooked_beef", 8, 12.8, "cooked_meat/beef"),
        ("rotten_flesh", 4, 0.8, ""),
    ] {
        let i = r.item(name).set(&FOOD, Food { hunger, saturation }).on_use(crate::player::eat).tab("food");
        if !tag.is_empty() {
            i.tag(tag);
        }
    }
    r.item("leather").tag("materials/leather").tab("ingredients");
    for name in ["bone", "string", "sugar"] {
        r.item(name).tab("ingredients");
    }

    for m in &MATERIALS {
        let tool = |r: &mut Registry, kind: &str, class: &str, damage: f32| {
            r.item(&format!("{}_{kind}", m.name))
                .tool(class, m.level, m.speed)
                .durability(m.durability)
                .attack(damage + m.damage)
                .tag(&format!("tools/{kind}"))
                .tag(&format!("{kind}s/{}", m.name))
                // Vanilla: weapons wear by one per hit, tools (made for blocks) by two.
                .set(&WEAR_PER_HIT, if kind == "sword" { 1 } else { 2 })
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
            r.item(&format!("{mat}_{piece}"))
                .durability(durability * dur)
                .set(&ARMOR, Armor { slot, points })
                .tag(tag)
                .tag(&format!("{tag}/{mat}"))
                .worn(parts, inflate, &format!("armor/{mat}_{layer}"))
                .tab("combat");
        }
    }
}
