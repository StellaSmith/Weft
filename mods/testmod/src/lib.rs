//! Proves the engine is moddable without touching the core. Everything here uses only
//! `voxide_sdk`:
//! - a generator, a cable and an electric furnace talking through the engine's energy contract,
//! - a block with its own screen (energy bar bound to synced menu data),
//! - reads the base game's data (fuel values, smelting recipes) by name, no code dependency: it
//!   declares the common `fuel` component itself and the engine checks both agree on its type,
//! - replaces a base game block (torches burn blue),
//! - adds its own dimensions, one of them a planet built only from terrain presets,
//! - adds an ore to the base game's overworld,
//! - adds a liquid (oil) whose bucket is the base game's bucket filled, without bucket code,
//! - makes cooked meat heal: its own component on the base game's items plus the base game's
//!   `eaten` event, and lets zombies drop tin through a loot modifier,
//! - shows off particles, shaders, text effects and glow masks (`looks`).

mod bodies;
mod looks;

use voxide_sdk::abi::slot_flags as sf;
use voxide_sdk::terrain::*;
use voxide_sdk::*;

declare_mod!(id: "testmod", name: "Test Mod", version: "0.1.0", deps: ["base"], init: init);

static GENERATOR: Block = Block::new();
static HUM: Sound = Sound::new();
static CABLE: Block = Block::new();
static ELECTRIC_FURNACE: Block = Block::new();
static SOUL_TORCH: Block = Block::new();
static GENERATOR_MENU: Menu = Menu::new();
static FURNACE_MENU: Menu = Menu::new();
static BATTERY_MENU: Menu = Menu::new();
static SMELTING_RECIPES: RecipeKind = RecipeKind::named("base:smelting");
static VOID: Dimension = Dimension::new();
static MARS: Dimension = Dimension::new();
static DUST: Biome = Biome::new();
static SCORCHED: Biome = Biome::new();
static REGOLITH: Block = Block::new();
static BASALT: Block = Block::new();
static TIN_ORE: Block = Block::new();
static TIN_INGOT: Item = Item::new();

value! {
    #[derive(Default)]
    struct Machine { burn: u32, burn_max: u32, progress: u32, progress_max: u32 }
}

static MACHINE: Component<Machine> = Component::new("testmod:machine");
/// The common fuel value; the base game sets it, this mod only reads it.
static FUEL: Component<u32> = Component::new("fuel");
static FOOD_HEAL: Component<f32> = Component::new("testmod:food_heal");

value! {
    /// The base game's `eaten` event, declared here the same way (no code dependency).
    struct Eaten { player: u64, stack: ItemStack, hunger: u32, saturation: f32 }
}
static EATEN: Event<Eaten> = Event::new("eaten");

const PER_TICK: i64 = 40;
const FURNACE_USE: i64 = 20;

fn init(r: &mut Registry) {
    looks::register(r);
    bodies::register(r);
    debug_panel(r);
    about_screen(r);
    morph(r);
    r.block("cable").handle(&CABLE).model_box([5.0, 5.0, 5.0], [11.0, 11.0, 11.0], "cable").tag("conductor").hardness(0.5);

    // Same name as the base game's torch: the base game's handle, item and recipes give this.
    // It keeps the base torch's states (floor and walls) and placement, only its texture and
    // light are its own.
    r.block("base:torch").handle(&SOUL_TORCH)
        .replace()
        .retexture("base:block/torch", "soul_torch")
        .light_rgb(3, 9, 15)
        .hardness(0.0);

    // Machines: block, entity with the machine and energy components, menu, all in one builder.
    // The components sync to open menus; screens bind to them.
    let flags = component_flags::SAVE | component_flags::SYNC_MENU;
    // Machines' sounds have their own category (and volume setting `testmod:volume_machines`).
    r.sound_category("machines", true);
    r.sound("machine.hum").handle(&HUM).files(&["base:dig/stone1"]).category("machines").pitch(0.45, 0.5);
    r.machine("generator", &GENERATOR_MENU)
        .block(|b| b.handle(&GENERATOR).textures("machine_top", "generator", "machine_top").hardness(3.0).mineable("pickaxe").needs_tool(0))
        .slots(&[0])
        // Only fuel goes in (the base game's `fuel` component).
        .accepts(0, "fuel")
        .component(&MACHINE, Machine::default(), flags)
        .attach_with(&ENERGY, Energy { stored: 0, capacity: 20_000, max_in: 0, max_out: 200 })
        .on_tick(tick_generators)
        .screen(|ui, player| {
            machine_screen(ui, "testmod.generator", player, |ui| {
                ui.column(S::col().gap(2.0).center(), |ui| {
                    ui.slot(0);
                    ui.progress_bind_dir("flame", S::default().size(14.0, 14.0), abi::ui_dir::UP, |c| c.menu_component(&MACHINE).map_or(0.0, |m| ratio(m.burn, m.burn_max)));
                });
            })
        });
    r.machine("electric_furnace", &FURNACE_MENU)
        .block(|b| b.handle(&ELECTRIC_FURNACE).textures("machine_top", "electric_furnace", "machine_top").hardness(3.0).mineable("pickaxe").needs_tool(0))
        .slots(&[0, sf::OUTPUT])
        .component(&MACHINE, Machine::default(), flags)
        .attach_with(&ENERGY, Energy { stored: 0, capacity: 40_000, max_in: 400, max_out: 0 })
        .on_tick(tick_furnaces)
        .screen(|ui, player| {
            machine_screen(ui, "testmod.electric_furnace", player, |ui| {
                ui.slot(0);
                ui.progress_bind("arrow", S::default().size(24.0, 16.0), |c| c.menu_component(&MACHINE).map_or(0.0, |m| ratio(m.progress, m.progress_max)));
                ui.slot(1);
            })
        });

    // No tick of its own: only the grid fills and drains it, so its bar shows that components
    // reach open menus whoever changed them.
    r.machine("battery", &BATTERY_MENU)
        .block(|b| b.textures("machine_top", "battery", "machine_top").hardness(3.0).mineable("pickaxe").needs_tool(0))
        .slots(&[])
        .attach_with(&ENERGY, Energy { stored: 0, capacity: 100_000, max_in: 400, max_out: 400 })
        .screen(|ui, player| machine_screen(ui, "testmod.battery", player, |_| {}));

    // The base game's kinds, by name: no code dependency on it.
    static CRAFTING: RecipeKind = RecipeKind::named("base:crafting");
    r.recipe_station(&SMELTING_RECIPES, "electric_furnace");
    r.shaped(&CRAFTING, "generator", 1, &["iii", "ifi", "iii"], &[('i', "#ingots/iron"), ('f', "base:furnace")]);
    r.shaped(&CRAFTING, "electric_furnace", 1, &["iii", "iFi", "ici"], &[('i', "#ingots/iron"), ('F', "base:furnace"), ('c', "cable")]);
    r.shaped(&CRAFTING, "cable", 8, &["iii"], &[('i', "#ingots/iron")]);

    r.block("regolith").handle(&REGOLITH).hardness(0.5).mineable("shovel");
    // Only the common `sand` tag: cactus and sugar cane accept it without knowing it.
    r.block("red_sand").hardness(0.5).mineable("shovel").tag("sand");
    r.block("basalt").handle(&BASALT).hardness(1.5).mineable("pickaxe").tag("stone_ore_replaceables");
    // Only tags: the base game's `#ores/*` smelting recipe and tools pick these up.
    r.block("tin_ore").handle(&TIN_ORE).hardness(3.0).mineable("pickaxe").needs_tool(0).tag("ores/tin");
    r.item("tin_ingot").handle(&TIN_INGOT).tag("ingots/tin");
    // A tier between vanilla's iron (200) and diamond (300): mines everything iron does plus
    // reinforced stone, which iron cannot.
    r.item("steel_pickaxe").tool("pickaxe", 250, 7.0).durability(800).attack(4.0).set(&WEAR_PER_HIT, 2).tag("tools/pickaxe").tab("base:tools");
    // May face up, north or east only: three states, not six.
    r.block("vent")
        .facing(Facings::only(&[Dir::Up, Dir::North, Dir::East]), Toward::Player, |d, s| {
            let mut f = ["vent_side"; 6];
            f[d.face() as usize] = "vent_front";
            s.faces(f)
        })
        .hardness(2.0)
        .mineable("pickaxe")
        .tab("base:building");
    r.block("reinforced_stone").hardness(4.0).mineable("pickaxe").needs_tool(250).tab("base:building");
    r.biome("dust").handle(&DUST).colors(0xb5764a, 0x9a6040, 0x8a5a40).climate(1.5, 0.0);
    r.biome("scorched").handle(&SCORCHED).colors(0xbf8a50, 0xae7a3a, 0x3f76e4).climate(2.0, 0.0).tag("dry").tag("hot");
    // Base's overworld, edited from outside: a biome in the hottest, driest deserts, a rock needle
    // from a feature of this mod's own, and boulders on the new biome.
    r.terrain("base:overworld")
        .biome_before("base:desert", BiomeRule::new("scorched").temperature(0.4, f64::INFINITY).humidity(f64::NEG_INFINITY, -0.1).surface("regolith", "regolith", 2))
        .feature(Feature::new(|_, x, z| if (x - NEEDLE[0]).abs().max((z - NEEDLE[1]).abs()) < 3.0 { 90.0 } else { 0.0 }).named("needle"))
        .decorate(boulders);
    r.ore("base:overworld", "tin_ore").y(-64, 48).veins(4).size(7).replace("#stone_ore_replaceables");
    r.ore("mars", "tin_ore").y(-64, 90).veins(6).size(8).replace("#stone_ore_replaceables");
    r.terrain("mars")
        .stone("basalt")
        .base_height(60.0)
        .feature(preset::hills(8.0, 220.0))
        .feature(preset::plateaus(24.0, 320.0, 6.0).mask(Mask::noise(900.0, 0.1, 0.4)))
        .feature(preset::canyons(26.0, 9.0).mask(Mask::noise(1200.0, -0.2, 0.1)))
        .feature(preset::craters(32.0, 16.0, 150.0))
        .feature(preset::dunes(3.0, 22.0).mask(Mask::noise(500.0, 0.15, 0.45)))
        .biome(BiomeRule::new("dust").surface("regolith", "regolith", 3));
    r.dimension("mars").handle(&MARS)
        .spawn(|seed| [8, MARS.column(seed, 8, 8).height + 1, 8])
        .height(-4, 12)
        .sky_above(11)
        .gravity(12.0)
        .fixed_time(5000)
        .sky(0xd8a080, 0xe0b090);

    // A liquid of its own, flowing by the engine's rules; its bucket fills from the base game's.
    r.liquid("oil").texture("oil_still").opacity(4).drag(0.4).delay(15).drop_off(2).slope_find(3).infinite(0).tag("oil").bucket("oil_bucket", "base:bucket", "base:tools");

    // Healing food without touching the base game's eating code.
    r.set_tagged("cooked_meat/*", &FOOD_HEAL, 2.0);
    r.on(&EATEN, |b| {
        let w = b.world();
        for e in b.iter() {
            if let (Some(heal), Some(mut st)) = (FOOD_HEAL.on_stack(&e.stack), w.entity(e.player)) {
                st.health = (st.health + heal).min(st.max_health);
                w.set_entity(e.player, &st);
            }
        }
    });
    r.loot_modify("base:zombie").add_pool(Pool::new().when(Cond::Chance(0.05)).item("tin_ingot"));

    r.command("tagbench", "[millions:int]", tag_bench);
    // The engine's /gizmo suggests this mod's group too.
    r.gizmo_group("energy");

    r.dimension("void").handle(&VOID)
        .generator(island)
        .spawn(|_| [0, 64, 0])
        .height(2, 6)
        .sky_above(5)
        .gravity(14.0)
        .fixed_time(18000)
        .ambient(5)
        .sky(0x2a1040, 0x40206a);
}

/// `/morph base:pig 0.9 0.9`: the player looks like another kind and gets a box of that size
/// (eyes at 85%), still the same player; `/morph` alone turns back.
fn morph(r: &mut Registry) {
    r.command("morph", "[kind:entity] [width:float] [height:float]", |cmd| {
        let (w, me) = (cmd.world(), cmd.player());
        match cmd.text("kind").map(str::to_string) {
            Some(kind) => {
                let (width, height) = (cmd.float("width").unwrap_or(0.9) as f32, cmd.float("height").unwrap_or(0.9) as f32);
                w.set_look(me, Some(&kind));
                w.set_size(me, Some([width, height, height * 0.85]));
                cmd.reply(&format!("now a {kind}, {width} x {height}"));
            }
            None => {
                w.set_look(me, None);
                w.set_size(me, None);
                cmd.reply("yourself again");
            }
        }
    });
}

/// A screen of the mod's own, opened by name (`/about` here, `--screen testmod:about`).
fn about_screen(r: &mut Registry) {
    r.screen("about", |ui| {
        ui.panel("testmod_panel", S::col().pad(8.0).gap(4.0), |ui| {
            ui.text("testmod.about.title");
            ui.text("testmod.about.text");
        });
    });
    r.client_command("about", "", |c| {
        if let Some(cl) = c.client() {
            cl.open_screen("testmod:about");
        }
    });
}

/// An F3 panel of the mod's own (F3+6 toggles it).
fn debug_panel(r: &mut Registry) {
    r.hud_element("debug", hud_anchor::DEBUG_LEFT, 0, |ui| {
        ui.text_bind(|c, out| {
            let Some(me) = c.me() else { return };
            let speed = (me.vel[0] * me.vel[0] + me.vel[2] * me.vel[2]).sqrt() * 20.0;
            out.set("testmod.debug.speed", &[Arg::Float(speed), Arg::Float(me.fall as f64)]);
        });
    });
}

/// `player`: menu index of the first player slot (after the machine's own slots).
fn machine_screen(ui: &mut Ui, title: &'static str, player: u32, slots: impl FnOnce(&mut Ui)) {
    // Drawn by the mod's GUI shader (`testmod_panel.shader` in its theme).
    ui.panel("testmod_panel", S::col().pad(7.0).gap(4.0), |ui| {
        ui.text(title);
        ui.row(S::row().gap(8.0), |ui| {
            ui.column(S::col().gap(2.0), |ui| {
                ui.text_bind(|c, out| {
                    let e = c.menu_component(&ENERGY).unwrap_or_default();
                    out.set("testmod.energy", &[Arg::Int(e.stored), Arg::Int(e.capacity)]);
                });
                ui.progress_bind("energy", S::default().size(80.0, 8.0), |c| c.menu_component(&ENERGY).map_or(0.0, |e| if e.capacity > 0 { e.stored as f32 / e.capacity as f32 } else { 0.0 }));
            });
            ui.spacer();
            ui.row(S::row().gap(6.0), slots);
        });
        ui.player_inventory(player, true);
    });
}

fn ratio(a: u32, b: u32) -> f32 {
    if b == 0 { 0.0 } else { a as f32 / b as f32 }
}

/// `/gizmo testmod:energy`: stored energy above every machine, the mod-side use of gizmos.
fn energy_gizmos(t: &EntityTick) {
    let w = t.world();
    if w.time() % 10 != 0 || !w.gizmo_on("energy") {
        return;
    }
    for &id in t.ids() {
        if let (Some(st), Some(e)) = (w.entity(id), w.energy(id)) {
            let text = format!("{} / {} EU", e.stored, e.capacity);
            w.gizmo_text("energy", [st.pos[0], st.pos[1] + 1.3, st.pos[2]], &text, 0x60FF60FF, 11);
        }
    }
}

fn tick_generators(t: &EntityTick) {
    energy_gizmos(t);
    let w = t.world();
    for &id in t.ids() {
        let Some(e) = w.energy(id) else { continue };
        let room = e.capacity - e.stored;
        let Some(m) = w.component_mut(id, &MACHINE) else { continue };
        if m.burn == 0 && room >= PER_TICK {
            let inv = w.inventory(id);
            let fuel = inv.get(0);
            // Any mod's fuel works: the value is the common `fuel` component.
            if let Some(ticks) = FUEL.on_item(fuel.item).filter(|_| !fuel.is_empty()) {
                m.burn = ticks;
                m.burn_max = ticks;
                // A lava bucket leaves its bucket, like in the furnace.
                inv.set(0, use_one(fuel));
            }
        }
        let Some(m) = w.component_mut(id, &MACHINE) else { continue };
        if m.burn > 0 {
            m.burn -= 1;
            if let Some(e) = w.energy_mut(id) {
                e.stored = (e.stored + PER_TICK).min(e.capacity);
            }
            // A low hum every two seconds, under the test mod's own volume setting.
            if w.time() % 40 == 0 {
                if let Some(st) = w.entity(id) {
                    w.play_sound(&HUM, st.pos, 0.4);
                }
            }
        }
    }
}

fn tick_furnaces(t: &EntityTick) {
    energy_gizmos(t);
    let w = t.world();
    for &id in t.ids() {
        let Some(e) = w.energy(id) else { continue };
        let stored = e.stored;
        let inv = w.inventory(id);
        let (input, output) = (inv.get(0), inv.get(1));
        let recipe = (!input.is_empty()).then(|| recipe_process(&SMELTING_RECIPES, input.item)).flatten();
        let fits = recipe.is_some_and(|(out, _)| output.is_empty() || (output.stacks_with(&out) && output.count + out.count <= max_stack(out.item)));
        let Some(m) = w.component_mut(id, &MACHINE) else { continue };
        if !fits || stored < FURNACE_USE {
            m.progress = 0;
            continue;
        }
        let (out, time) = recipe.unwrap();
        // Twice as fast as the base furnace, paid for in energy.
        m.progress_max = (time / 2).max(1);
        m.progress += 1;
        let done = m.progress >= m.progress_max;
        if done {
            m.progress = 0;
        }
        if let Some(e) = w.energy_mut(id) {
            e.stored -= FURNACE_USE;
        }
        if done {
            inv.set(0, input.shrink(1));
            inv.set(1, if output.is_empty() { out } else { output.grow(out.count) });
        }
    }
}

/// Another mod's blocks, found by name the first time they are used.
static STONE: Block = Block::named("base:stone");
static GRASS: Block = Block::named("base:grass");
static DIRT: Block = Block::named("base:dirt");
static CHEST: Block = Block::named("base:chest");
/// Where the island's chest stands: a block entity's block placed by a generator.
pub const ISLAND_CHEST: [i32; 3] = [4, 64, 0];

/// A floating island of stone and grass in an empty void, with a chest on it.
/// Where the needle stands in base's overworld.
pub const NEEDLE: [f64; 2] = [1000.0, 1000.0];

/// Basalt boulders on the scorched biome of base's overworld.
fn boulders(g: &mut Gen) {
    let Some(&cols) = g.columns() else { return };
    let [ox, oy, oz] = g.origin();
    let (scorched, basalt) = (SCORCHED.id(), BASALT.id());
    for (i, c) in cols.iter().enumerate() {
        let y = c.height + 1 - oy;
        if c.biome != scorched || !(1..16).contains(&y) {
            continue;
        }
        let (x, y, z) = (i % 16, y as usize, i / 16);
        let h = noise::hash(g.seed() ^ 0xB0, (ox + x as i32) as i64, 0, (oz + z as i32) as i64);
        if h % 40 == 0 && g.get(x, y, z) == 0 && g.get(x, y - 1, z) != 0 {
            g.set(x, y, z, basalt);
        }
    }
}

fn island(g: &mut Gen) {
    let [ox, oy, oz] = g.origin();
    let (stone, grass, dirt) = (STONE.id(), GRASS.id(), DIRT.id());
    for y in 0..16usize {
        for z in 0..16usize {
            for x in 0..16usize {
                let (wx, wy, wz) = ((ox + x as i32) as f32, oy + y as i32, (oz + z as i32) as f32);
                let r = (wx * wx + wz * wz).sqrt();
                let top = 63;
                let depth = (24.0 - r) * 0.8;
                if r < 24.0 && wy <= top && (top - wy) as f32 <= depth {
                    let s = if wy == top { grass } else if top - wy < 3 { dirt } else { stone };
                    g.set(x, y, z, s);
                }
            }
        }
    }
    let [cx, cy, cz] = ISLAND_CHEST;
    let (lx, ly, lz) = (cx - ox, cy - oy, cz - oz);
    if (0..16).contains(&lx) && (0..16).contains(&ly) && (0..16).contains(&lz) {
        g.set(lx as usize, ly as usize, lz as usize, CHEST.id());
    }
}

/// `/tagbench`: what a tag check costs a mod compared with reading a field.
fn tag_bench(cmd: &mut Command) {
    static PLANT_SOIL: Tag = Tag::new("plant_soil");
    let n = cmd.int("millions").unwrap_or(10).clamp(1, 1000) as u32 * 1_000_000;
    let states: Vec<u32> = (0..256).map(|i| i * 7 % 120).collect();
    let flags: Vec<u32> = (0..256).map(|i| i * 31).collect();
    let time = |f: &dyn Fn(u32) -> u32| {
        let t = std::time::Instant::now();
        let mut acc = 0u32;
        for i in 0..n {
            acc = acc.wrapping_add(f(states[(i & 255) as usize]));
        }
        std::hint::black_box(acc);
        t.elapsed().as_secs_f64() * 1e9 / n as f64
    };
    let field = time(&|s| flags[s as usize & 255]);
    let tag = time(&|s| PLANT_SOIL.on_block(s) as u32);
    let host_call = time(&|s| state_flags(s));
    let by_name = time(&|s| block_has_tag(s, "plant_soil") as u32);
    cmd.reply(&format!("ns per lookup: field {field:.2}, tag {tag:.2}, host call {host_call:.2}, tag by name {by_name:.2}"));
}
