//! Proves the engine is moddable without touching the core. Everything here uses only
//! `voxide_sdk`:
//! - a generator, a cable and an electric furnace talking through the engine's energy contract,
//! - a block with its own screen (energy bar bound to synced menu data),
//! - reads the base game's data (fuel values, smelting recipes) by name, no code dependency: it
//!   declares the common `fuel` component itself and the engine checks both agree on its type,
//! - replaces a base game block (torches burn blue),
//! - adds its own dimensions, one of them a planet built only from terrain presets,
//! - adds an ore to the base game's overworld,
//! - adds a liquid (oil) whose bucket is the base game's bucket filled, without bucket code.

use std::sync::LazyLock;

use voxide_sdk::abi::slot_flags as sf;
use voxide_sdk::terrain::*;
use voxide_sdk::*;

declare_mod!(id: "testmod", name: "Test Mod", version: "0.1.0", deps: ["base"], init: init);

static GENERATOR: Block = Block::new();
static CABLE: Block = Block::new();
static ELECTRIC_FURNACE: Block = Block::new();
static SOUL_TORCH: Block = Block::new();
static GENERATOR_ENTITY: Entity = Entity::new();
static FURNACE_ENTITY: Entity = Entity::new();
static GENERATOR_MENU: Menu = Menu::new();
static FURNACE_MENU: Menu = Menu::new();
static VOID: Dimension = Dimension::new();
static MARS: Dimension = Dimension::new();
static DUST: Biome = Biome::new();
static REGOLITH: Block = Block::new();
static BASALT: Block = Block::new();
static TIN_ORE: Block = Block::new();
static TIN_INGOT: Item = Item::new();

static MARS_TERRAIN: LazyLock<Terrain> = LazyLock::new(|| {
    Terrain::new(&BASALT, 60.0)
        .feature(preset::hills(8.0, 220.0))
        .feature(preset::plateaus(24.0, 320.0, 6.0).mask(Mask::noise(900.0, 0.1, 0.4)))
        .feature(preset::canyons(26.0, 9.0).mask(Mask::noise(1200.0, -0.2, 0.1)))
        .feature(preset::craters(32.0, 16.0, 150.0))
        .feature(preset::dunes(3.0, 22.0).mask(Mask::noise(500.0, 0.15, 0.45)))
        .biome(BiomeRule::new(&DUST).surface(&REGOLITH, &REGOLITH, 3))
});

value! {
    #[derive(Default)]
    struct Machine { burn: u32, burn_max: u32, progress: u32, progress_max: u32 }
}

static MACHINE: Component<Machine> = Component::new("testmod:machine");
/// The common fuel value; the base game sets it, this mod only reads it.
static FUEL: Component<u32> = Component::new("fuel");

const PER_TICK: i64 = 40;
const FURNACE_USE: i64 = 20;

fn init(r: &mut Registry) {
    r.block("generator").handle(&GENERATOR).textures("machine_top", "generator", "machine_top").hardness(3.0).mineable("pickaxe").needs_tool(0).on_use(open(&GENERATOR_MENU));
    r.block("electric_furnace").handle(&ELECTRIC_FURNACE)
        .textures("machine_top", "electric_furnace", "machine_top")
        .hardness(3.0)
        .mineable("pickaxe").needs_tool(0)
        .on_use(open(&FURNACE_MENU));
    r.block("cable").handle(&CABLE).model_box([5.0, 5.0, 5.0], [11.0, 11.0, 11.0], "cable").tag("conductor").hardness(0.5);

    // Same name as the base game's torch: the base game's handle, item and recipes now give this.
    r.block("base:torch").handle(&SOUL_TORCH)
        .replace()
        .model_box([7.0, 0.0, 7.0], [9.0, 10.0, 9.0], "soul_torch")
        .not_solid()
        .light_rgb(3, 9, 15)
        .hardness(0.0)
        .needs(2, "");

    r.entity_component(&MACHINE, Machine::default(), component_flags::SAVE, &["testmod:generator", "testmod:electric_furnace"]);
    r.entity("generator").handle(&GENERATOR_ENTITY).flags(entity_flags::NO_GRAVITY | entity_flags::INVULNERABLE).on_tick(tick_generators);
    r.entity("electric_furnace").handle(&FURNACE_ENTITY).flags(entity_flags::NO_GRAVITY | entity_flags::INVULNERABLE).on_tick(tick_furnaces);
    r.attach("generator", "energy");
    r.attach("electric_furnace", "energy");
    r.block_entity("generator", "generator", 1);
    r.block_entity("electric_furnace", "electric_furnace", 2);

    r.menu("generator").handle(&GENERATOR_MENU)
        .data(4)
        .setup(|m| {
            m.slots(m.owner(), 0, 1, 0);
            m.slots(PLAYER, 9, 27, 0);
            m.slots(PLAYER, 0, 9, 0);
        })
        .on_tick(|m| sync(m))
        .screen(|ui| {
            machine_screen(ui, "testmod.generator", 1, |ui| {
                ui.column(S::col().gap(2.0).center(), |ui| {
                    ui.slot(0);
                    ui.progress_dir("flame", S::default().size(14.0, 14.0), 2, 3, abi::ui_dir::UP);
                });
            })
        });
    r.menu("electric_furnace").handle(&FURNACE_MENU)
        .data(4)
        .setup(|m| {
            m.slots(m.owner(), 0, 1, 0);
            m.slots(m.owner(), 1, 1, sf::OUTPUT);
            m.slots(PLAYER, 9, 27, 0);
            m.slots(PLAYER, 0, 9, 0);
        })
        .on_tick(|m| sync(m))
        .screen(|ui| {
            machine_screen(ui, "testmod.electric_furnace", 2, |ui| {
                ui.slot(0);
                ui.progress("arrow", S::default().size(24.0, 16.0), 2, 3);
                ui.slot(1);
            })
        });

    r.shaped("base:crafting", "generator", 1, &["iii", "ifi", "iii"], &[('i', "#ingots/iron"), ('f', "base:furnace")]);
    r.shaped("base:crafting", "electric_furnace", 1, &["iii", "iFi", "ici"], &[('i', "#ingots/iron"), ('F', "base:furnace"), ('c', "cable")]);
    r.shaped("base:crafting", "cable", 8, &["iii"], &[('i', "#ingots/iron")]);

    r.block("regolith").handle(&REGOLITH).hardness(0.5).mineable("shovel");
    // Only the common `sand` tag: cactus and sugar cane accept it without knowing it.
    r.block("red_sand").hardness(0.5).mineable("shovel").tag("sand");
    r.block("basalt").handle(&BASALT).hardness(1.5).mineable("pickaxe").tag("stone_ore_replaceables");
    // Only tags: the base game's `#ores/*` smelting recipe and tools pick these up.
    r.block("tin_ore").handle(&TIN_ORE).hardness(3.0).mineable("pickaxe").needs_tool(0).tag("ores/tin");
    r.item("tin_ingot").handle(&TIN_INGOT).tag("ingots/tin");
    r.biome("dust").handle(&DUST).colors(0xb5764a, 0x9a6040, 0x8a5a40).climate(1.5, 0.0);
    r.ore("base:overworld", "tin_ore").y(-64, 48).veins(4).size(7).replace("#stone_ore_replaceables");
    r.ore("mars", "tin_ore").y(-64, 90).veins(6).size(8).replace("#stone_ore_replaceables");
    r.dimension("mars").handle(&MARS)
        .generator(|g| {
            MARS_TERRAIN.generate(g);
        })
        .spawn(|seed| [8, MARS_TERRAIN.column(seed, 8, 8).height + 1, 8])
        .height(-4, 12)
        .sky_above(11)
        .gravity(12.0)
        .fixed_time(5000)
        .sky(0xd8a080, 0xe0b090);

    // A liquid of its own, flowing by the engine's rules; its bucket fills from the base game's.
    r.liquid("oil").texture("oil_still").opacity(4).drag(0.4).delay(15).drop_off(2).slope_find(3).infinite(0).tag("oil").bucket("oil_bucket", "base:bucket");

    r.command("tagbench", "[millions:int]", tag_bench);

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

fn open(menu: &'static Menu) -> impl Fn(&mut Use) -> bool + Send + Sync + 'static {
    move |u| {
        let w = u.world();
        let owner = w.block_entity(u.pos());
        if owner == 0 {
            return false;
        }
        w.open_menu(u.player(), menu, owner);
        true
    }
}

/// `first`: menu index of the first player slot (after the machine's own slots).
fn machine_screen(ui: &mut Ui, title: &'static str, first: u32, slots: impl FnOnce(&mut Ui)) {
    ui.panel("panel", S::col().pad(7.0).gap(4.0), |ui| {
        ui.text(title);
        ui.row(S::row().gap(8.0), |ui| {
            ui.column(S::col().gap(2.0), |ui| {
                ui.text_bind(|c, out| out.set("testmod.energy", &[Arg::Int(c.menu_data(0)), Arg::Int(c.menu_data(1))]));
                ui.progress("energy", S::default().size(80.0, 8.0), 0, 1);
            });
            ui.spacer();
            ui.row(S::row().gap(6.0), slots);
        });
        ui.text("container.inventory");
        ui.column(S::col().gap(4.0), |ui| {
            ui.slots(first, 27, 9);
            ui.slots(first + 27, 9, 9);
        });
    });
}

/// Menu data: 0 stored energy, 1 capacity, 2 progress, 3 progress max.
fn sync(m: &MenuCtx) {
    let w = m.world();
    let id = m.owner();
    if let Some(e) = w.energy(id) {
        let e = *e;
        m.set_data(0, e.stored);
        m.set_data(1, e.capacity);
    }
    if let Some(s) = w.component(id, &MACHINE) {
        let (a, b) = if s.burn_max > 0 { (s.burn, s.burn_max) } else { (s.progress, s.progress_max) };
        m.set_data(2, a as i64);
        m.set_data(3, b as i64);
    }
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
        let Some(e) = w.energy_mut(id) else { continue };
        if e.capacity == 0 {
            *e = Energy { stored: 0, capacity: 20_000, max_in: 0, max_out: 200 };
        }
        let room = e.capacity - e.stored;
        let Some(m) = w.component_mut(id, &MACHINE) else { continue };
        if m.burn == 0 && room >= PER_TICK {
            let inv = w.inventory(id);
            let fuel = inv.get(0);
            // Any mod's fuel works: the value is the common `fuel` component.
            if let Some(ticks) = FUEL.on_item(fuel.item).filter(|_| !fuel.is_empty()) {
                m.burn = ticks;
                m.burn_max = ticks;
                inv.set(0, fuel.shrink(1));
            }
        }
        let Some(m) = w.component_mut(id, &MACHINE) else { continue };
        if m.burn > 0 {
            m.burn -= 1;
            if let Some(e) = w.energy_mut(id) {
                e.stored = (e.stored + PER_TICK).min(e.capacity);
            }
        }
    }
}

fn tick_furnaces(t: &EntityTick) {
    energy_gizmos(t);
    let w = t.world();
    for &id in t.ids() {
        let Some(e) = w.energy_mut(id) else { continue };
        if e.capacity == 0 {
            *e = Energy { stored: 0, capacity: 40_000, max_in: 400, max_out: 0 };
        }
        let stored = e.stored;
        let inv = w.inventory(id);
        let (input, output) = (inv.get(0), inv.get(1));
        let recipe = (!input.is_empty()).then(|| recipe_process("base:smelting", input.item)).flatten();
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

/// A floating island of stone and grass in an empty void.
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
