//! A starting point for your own mod. Copy this folder, rename it, and replace the
//! examples with your stuff.
//!
//! Rename checklist:
//! - `Cargo.toml`: `name` (it's also the library's file name, so it must equal your mod id).
//! - `declare_mod!`: `id`, `name`, `version`, `deps`.
//! - `assets/lang/*.lang`: the keys start with your mod id.
//!
//! Build: `cargo build --release`. Then make `run/mods/<your id>/` with the library from
//! `target/release/` and the `assets` folder next to it.
//! To share it, run `voxide --pack-mod run/mods/<your id>`,
//! it packs that folder (library included) into a zip.
//!
//! This example adds one item, a setting, and a gift for joined players.
//! Everything else is commented examples. The full cheat sheet is at `docs/modding.md`.
//! `mods/base` and `mods/testmod` use exactly this API, so for more examples check those.

use voxide_sdk::*;

declare_mod!(id: "examplemod", name: "Example Mod", version: "0.1.0", deps: ["base"], init: init);

// Handles bound at registration with `.handle(&...)`, after that `EXAMPLE_ITEM.id()` is the
// numeric id. Keep them `static` since hot code always works with ids.
pub static EXAMPLE_ITEM: Item = Item::new();
pub static GIFT_ON_JOIN: Config<bool> = Config::new();

/// Called once at startup, in parallel with the other mods: register everything here.
/// Nothing exists in the world yet, you only describe what your mod adds.
fn init(r: &mut Registry) {
    config(r);
    blocks(r);
    items(r);
    recipes(r);
    worldgen(r);
    entities(r);
    events(r);
    commands(r);
    client(r);
}

/// Settings show up in Settings > Mod Settings by themselves and are saved to
/// `config/examplemod.txt`. Name and tooltip come from `config.examplemod.<key>` in the lang file.
fn config(r: &mut Registry) {
    r.config("gift_on_join").bool(true).handle(&GIFT_ON_JOIN);
    // More examples:
    //      r.config("power").int(10, 0, 100).handle(&POWER);
    //      r.config("style").choice(0, &["warm", "cold"]).handle(&STYLE);
    // `.synced()` means server value overrides clients' values, for anything gameplay depends on
}

/// Your blocks, texture is `block/<name>.png`, translation is `block.examplemod.<name>`.
/// More at `mods/base/src/blocks.rs` (support rules, plants, falling sand, torches, axis, etc.)
/// And `mods/testmod/src/lib.rs` (machines, facing, replacing another mod's block etc.)
fn blocks(_r: &mut Registry) {
    // r.block("ruby_ore")
    //     .hardness(3.0)
    //     .mineable("pickaxe")
    //     .needs_tool(tool_tier::IRON)      // drops nothing without at least an iron pickaxe
    //     .drops("ruby", 1)
    //     .tab("base:natural");
    //
    // Light and glass-like looks plus a custom box:
    // r.block("lamp").light_rgb(15, 10, 4).hardness(0.3);
    // r.block("pane").model_box([0.0, 0.0, 7.0], [16.0, 16.0, 9.0], "pane").cutout();
    //
    // Behavior (all optional, all get a `World`):
    // r.block("button").on_use(|u| { u.world(); u.pos(); u.player(); true });
}

/// Your items, texture is `item/<name>.png`, translation is `item.examplemod.<name>`
/// Gray tooltip with `item.examplemod.<name>.tooltip`, for more tooltip options check `mods/base/assets/lang/en_us.lang`
fn items(r: &mut Registry) {
    r.item("example_item").handle(&EXAMPLE_ITEM).tab("base:ingredients").tag("gems/example");
    // Tools, weapons, food, fuel are all components or tags, so other mods can use them:
    // r.item("ruby_pickaxe").tool("pickaxe", tool_tier::DIAMOND, 8.0).durability(1200).attack(4.0).tag("tools/pickaxe");
    // r.item("ruby_apple").set(&FOOD, ...)     // see `mods/base/src/items.rs` for food and armor
    // r.set("examplemod:example_item", &FUEL, 1600);   // FUEL: Component<u32> = Component::new("fuel")
}

/// Recipes reference items by name or `#tag`, so a recipe made with `#ingots/iron` tag takes any mod's
/// item with the same tag. `base:crafting` and `base:smelting` belong to the base game.
fn recipes(_r: &mut Registry) {
    // static CRAFTING: RecipeKind = RecipeKind::named("base:crafting");
    // static SMELTING: RecipeKind = RecipeKind::named("base:smelting");
    // r.shaped(&CRAFTING, "ruby_pickaxe", 1, &["rrr", " s ", " s "], &[('r', "#gems/ruby"), ('s', "base:stick")]);
    // r.shapeless(&CRAFTING, "ruby", 9, &["ruby_block"], 0);
    // r.shapeless(&SMELTING, "ruby", 1, &["ruby_ore"], 200);    // 200 ticks in a furnace
}

/// Ores, biomes, structures and dimensions. Ores go into another mod's dimension by name.
/// Mars as a whole planet from terrain presets is at `mods/testmod/src/lib.rs`
/// More at `mods/base/src/worldgen.rs`
fn worldgen(_r: &mut Registry) {
    // r.ore("base:overworld", "ruby_ore").y(-64, 16).veins(2).size(5).replace("#stone_ore_replaceables");
    // r.structure("base:overworld", "shrine").layer(&["ccc", "c c", "ccc"]).key('c', "base:cobblestone").spacing(256).chance(0.3);
}

/// Mobs and other entities
/// Models in code (`biped`, `quadruped`) or Blockbench files in `assets/models`
/// More at `mods/base/src/mobs.rs` (AI goals, spawn rules, etc.).
fn entities(_r: &mut Registry) {
    // r.entity("ghost").size(0.6, 1.8).health(10.0).speed(3.0).hostile().texture("ghost", 64, 32).model(biped(false));
    // r.spawn("ghost").light(0, 4).weight(1);
}

/// Reacting to the game, events arrive batched once per tick
/// for "may this happen" use `r.intercept`
/// to change a number use `r.modify`
/// For more info check "Hooking into the game" in `docs/modding.md`
fn events(r: &mut Registry) {
    // Players joining get an example item if the setting allows it.
    r.on(&PLAYER_JOINED, |batch| {
        if !GIFT_ON_JOIN.get() {
            return;
        }
        let w = batch.world();
        for joined in batch.iter() {
            w.inventory(joined.player).insert(ItemStack::new(EXAMPLE_ITEM.id(), 1));
            info!("gave player {} an example item", joined.player);
        }
    });
    // r.on(&BLOCK_BROKEN, |b| for e in b.iter() { /* e.player, e.pos, e.state */ });
    // r.intercept(question::BREAK_BLOCK, |q| if q.pos()[1] < -60 { Verdict::Deny } else { Verdict::Abstain });
    // r.modify(chain::DAMAGE, stage::MULTIPLY, |m| m.value *= 0.9);
}

/// Chat commands
/// The usage string is parsed, checked, completed with Tab and shown in `/help` by the engine
/// you only read the values by name.
fn commands(_r: &mut Registry) {
    // r.command("example", "[times:int]", |cmd| {
    //     let n = cmd.int("times").unwrap_or(1);
    //     cmd.reply(&format!("hello {n} times"));
    // });
}

/// Client side: screens, HUD, key bindings, packets.
/// More at:
/// `mods/base/src/player.rs` (HUD),
/// `mods/testmod/src/lib.rs` (screens, machine panels),
/// `mods/testmod/src/looks.rs` (particles, shaders, sounds).
fn client(_r: &mut Registry) {
    // r.key("dash", "R");
    // r.screen("about", |ui| ui.panel("background", S::col().pad(8.0), |ui| ui.text("examplemod.about")));
    // r.hud_element("mana", hud_anchor::HOTBAR_RIGHT, 2, |ui| ui.text_bind(|c, out| { /* ... */ }));
}
