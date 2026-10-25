# Voxide modding cheat sheet

## Contents
1. [Introduction](#introduction)
2. [Creating a mod in two minutes](#creating-a-mod-in-two-minutes)
3. [Names and paths](#names-and-paths)
   - [Names](#names), [Tags](#tags), [Handles](#handles), [Textures](#textures), [Translations](#translations)
4. [Components and tags as inter-mod communication](#components-and-tags-as-inter-mod-communication)
5. [Content builders](#content-builders)
   - [Blocks](#blocks), [Items](#items), [Entities](#entities), [Machines](#machines), [Liquids](#liquids), [World](#world), [Recipes](#recipes), [Loot](#loot), [Replacing something of another mod](#replacing-something-of-another-mod)
6. [Hooking into the game](#hooking-into-the-game)
   - [Built-in events](#built-in-events), [Systems and energy](#systems-and-energy)
7. [Commands](#commands)
8. [World API](#world-api)
9. [Client side](#client-side)
10. [Debugging](#debugging)
11. [Things that make your and other modders' life easier](#things-that-make-your-and-other-modders-life-easier)
12. [Guides for bigger systems](#guides-for-other-systems)

## Introduction
This is a guide describing how the base game itself is written.\
The game is a mod called `base`, which has no private hooks or priorities from the engine, so if base can do something,
your mod can too.\
When in doubt, check `mods/base` and `mods/testmod` and copy from there.\
testmod exists to prove you don't have to touch the engine.

## Creating a mod in two minutes

Fastest way to begin is to copy `examples/examplemod`.\
It is a ready mod with one item, a setting and an event.\
Every kind of registration is written out there as a commented example with a pointer to where the base game uses it.\
Rename three things listed at the top of its `lib.rs` and you can begin creating your mod.\
The bare minimum looks like this:

```toml
# Cargo.toml
[package]
name = "mymod"            # the library name must be your mod id
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
# you want to replace this path with the one to the SDK after copying this folder somewhere else
voxide_sdk = { path = "../../crates/sdk" }

# only while the mod folder lies inside another cargo project (like this repository), see examples/examplemod/Cargo.toml
[workspace]
```

```rust
// lib.rs
use voxide_sdk::*;

declare_mod!(id: "mymod", name: "My Mod", version: "0.1.0", deps: ["base"], init: init);

pub static RUBY: Item = Item::new();
pub static RUBY_ORE: Block = Block::new();

fn init(r: &mut Registry) {
    r.block("ruby_ore").handle(&RUBY_ORE).hardness(3.0).mineable("pickaxe").needs_tool(tool_tier::IRON).drops("ruby", 1);
    r.item("ruby").handle(&RUBY).tag("gems/ruby").tab("base:ingredients");
    r.ore("base:overworld", "ruby_ore").y(-64, 16).veins(2).size(5).replace("#stone_ore_replaceables");
}
```

Build with `cargo build --release`, then make a folder in `run/mods/` and copy the library from `target/release/` into it:

```
run/mods/mymod/
    mymod.dll          (libmymod.so / libmymod.dylib elsewhere)
    assets/
        textures/block/ruby_ore.png
        textures/item/ruby.png
        lang/en_us.lang
```

`voxide --pack-mod run/mods/mymod` makes a `mymod.zip` of the same thing, which is what you can send to anyone.\
The folder has to hold the library that's what gets packed.

A word on versioning: the mod interface is checked by a hash of its source. If your mod was built
using another engine version, it just refuses to load and tells you to rebuild.

## Names and paths
### Names
Everything has a name `namespace:path`. In your own mod you write just `ruby`, it becomes `mymod:ruby`.\
For other mods' things you have to write the full name: `base:furnace`.

### Tags
`#tag` in a recipe, an ore replace list or a loot condition means "anything with this tag".\
`#ingots/*` means anything with a tag starting with `ingots/`.

### Handles
Handles like `static RUBY: Item = Item::new()` are bound with `.handle(&RUBY)` and then give you the numeric id.\
Other mods' content: `Block::named("base:stone")`. Hot code works only with ids.

### Textures
`.texture("ruby")` for a block means `mymod:block/ruby`, for an item it means `mymod:item/ruby`.\
A full path stays as written. Resource packs can replace any of them.\
Textures can also be [animated, glow and have overlays](textures.md), or [be connected](ctm.md)).

### Translations
Format: `type.modname.name = value`, e.g. `item.examplemod.example_item = Example Item`\
Any item can have a tooltip using `type.modname.name.tooltip = Some tooltip`, more in the [tooltips guide](tooltips.md).\
If the player's language doesn't have a key (or a whole file), English is shown, so `en_us.lang` is the one you always need.

Text can carry colors and effects:
- `{#ff8080}This text is red{/}`
- `{fx:core:wave}wavy{/fx}` (new fx can be added from your mod too)
- `{on_key:!core:sneak}This text will appear when the bound key is NOT pressed{/on_key}`
- `{on_key:core:sneak}This text will appear only when the bound key is pressed{/on_key}`
- `{if:creative}This text will appear if the player is in creative mode{/if}`

## Components and tags as inter-mod communication

This is the most important part that you should remember to make life easier for you and other modders working with your mod.\
Properties are named values on content. Things like fuel, food, durability, etc etc, are components or tags.\
This is done so any mod reads and writes them without depending on whoever added a certain block.

```rust
// Declared the same way in every mod that uses it
static FUEL: Component<u32> = Component::new("fuel");
static HOT: Tag = Tag::new("hot");

r.item("coal_dust").set(&FUEL, 800).tag("hot");     // on your own content
r.set("base:stick", &FUEL, 100);                    // on someone else's
r.set_tagged("planks/*", &FUEL, 300);               // on everything with a tag
r.tag("hot", &["base:lava", "base:magma"]);

// Reading
if let Some(ticks) = FUEL.on_item(stack.item) { /* burn it */ }
if HOT.on_block(state) { /* harm player */ }
```

Common names like `fuel`, `food`, `sand`, `conductor`, `ores/iron` have no prefix on purpose since they
are very common.\
Your stuff goes under your namespace if you want other mods to not conflict with them: `mymod:heat`.

Your own data structures:

```rust
value! {
    #[derive(Default)]
    struct Machine { burn: u32, burn_max: u32 }
}
static MACHINE: Component<Machine> = Component::new("mymod:machine");
```

Per-entity components like a state of a machine, hunger or anything else that changes in time go through
`r.entity_component(&COMP, default, flags, &["core:player"])` or the `.attach_with(..)` of a builder.\
Flags from `component_flags` are `SAVE`, `SYNC_OWNER`, `SYNC_WATCHERS`, `SYNC_MENU`.\
You can change them through `w.component_mut(id, &COMP)` and the engine will figure out what to send.

## Content builders

All of them are `r.<thing>("name")` followed by settings. The registration happens when the builder
is dropped.

### Blocks
`r.block("name")` and then:
- `hardness`
- `mineable("pickaxe")`
- `needs_tool(tier)`
- `drops(item, n)` or `drops_nothing()`
- `light(15)` or `light_rgb(r, g, b)`
- `cutout()`, `translucent()`
- `model_box(min, max, texture)`, `collision(..)`
- `needs_below("soil")`
- `grows(seconds, min_light)` + `on_grow(..)` for plants
- `random_ticks(..)`
- `on_use`, `on_placed`, `on_broken`, `on_neighbor`, `on_scheduled`
- `facing(Facings::HORIZONTAL, Toward::Player, |dir, state| ..)` for rotated blocks
- `states(n, |i, s| ..)` for anything else with states

### Items
`r.item("name")` and then:
- `texture`
- `stack(N)`
- `durability`
- `tool("pickaxe", tier, speed)`
- `attack`
- `places(block)`
- `worn(..)` for armor
- `on_use`

### Entities
`r.entity("name")` and then:
- `size(w, h)`, `health`, `speed`
- `hostile()` or `passive()` or `category(..)`
- `texture`
- `model(biped(..))` or `model_file("thing")` (Blockbench geo.json)
- `animations(..)`
- `goals(..)` for AI (see `voxide_sdk::ai`)
- `on_tick`, `on_hurt`, `on_death`, `on_interact`

Moving platforms: `solid()`, `kinematic()`, `carry_height(..)`, and
`blocks(&[("base:oak_planks", from, to)])` makes a flying structure players can build on.

### Machines
`r.machine(name, &MENU)` is block + block entity + inventory + menu + screen in one go.\
Look at testmod's generator for an example.

### Liquids
`r.liquid("name")` and then:
- `delay`, `drop_off`, `slope_find`
- `infinite`
- `temperature`
- `bucket(..)`

Reactions: `r.liquid_reaction("#hot", "#water").source("obsidian")`.

### World
- `r.biome(..)`
- `r.dimension(..)` (`generator`, `spawn`, `column`, `height`, `gravity`, `fixed_time`, `sky`)
- `r.ore(dim, block)`
- `r.structure(dim, name)` with layers
- `r.spawn(entity).biome("animals").light(7, 15)`

The `terrain` module has presets like hills, canyons, dunes, caves, lava lakes if you don't want to write noise by hand.\
Mars in testmod is built from presets only.

### Recipes
- `r.shaped(&KIND, out, count, &["iii", " s ", " s "], &[('i', "#ingots/iron"), ('s', "base:stick")])`
- `r.shapeless(..)`

Recipe kinds are registered too (`r.recipe_kind`), a furnace is just a kind with stations.

### Loot
`r.loot("name").pool(Pool::new().rolls(1, 2).item("ruby"))`\
and other mods' tables without replacing them: `r.loot_modify("base:zombie").add_pool(..)`.

### Replacing something of another mod
Register the same name with `.replace()`.\
Their handle, item and recipes now point at yours. testmod does it with the torch.

## Hooking into the game
There are four mechanisms, try them in this order:

```rust
// Events which are batched, delivered once per tick and nobody can cancel them.
static EATEN: Event<Eaten> = Event::new("eaten");
r.on(&EATEN, |b| for e in b.iter() { /* do something */ });
w.emit(&MY_EVENT, value);

// Questions, anyone may say no, any deny wins.
r.intercept(question::BREAK_BLOCK, |q| if is_protected(q.pos()) { Verdict::Deny } else { Verdict::Abstain });

// Value chains for damage, attack damage, or your own. Stages order them, `w.explain(..)` and
// `--log modifiers=debug` show what changed what.
r.modify(chain::DAMAGE, stage::MULTIPLY, |m| m.value *= 0.5);

// Attributes with timed modifiers like speed, max health, your own that are saved with the entity.
r.attribute("mymod:mana", 100.0, 0.0, 1000.0);
```

### Built-in events
`BLOCK_BROKEN`, `BLOCK_PLACED`, `ITEM_PICKED_UP`, `ENTITY_HURT`, `ENTITY_DIED`, `PLAYER_JOINED`\
base adds `eaten` and `smelted`.

### Systems and energy
Systems run over every entity with a component all at once, which is what you want for
thousands of machines: `r.system(&MACHINE, |batch| for (id, m) in batch.iter_mut() { .. })`.

Energy is a built-in engine contract, you can give a block entity the `core:energy` component, tag
cables with `conductor`, and every mod's generators and machines will work together.\
Networks balance once per tick.

## Commands
`r.command("heal", "[amount:int] [who:targets]", |cmd| ..)`\
The usage string is the whole spec: the engine parses it, checks types, completes it with Tab and shows it in `/help`.\
Reading values by name: `cmd.int("amount")`, `cmd.targets("who")`.\
`r.client_command` is the same for commands that run on the client that typed them.

## World API

Inside any callback you get a `World` (`w`):
- blocks (`get`, `set`, `schedule`)
- entities (`spawn`, `entity`, `set_entity`, `damage`, `near`, `navigate`, `walk`)
- inventories (`inventory(id).get/set/insert`)
- components, energy, menus
- particles (`w.particles(&SPARK, pos).count(20)`)
- sounds (`w.play_sound(&S, pos, volume)`)
- rules (`w.rule("mymod:hard_mode")`)
- gizmos for debugging (`w.gizmo_line/box/text`)

## Client side

- UI is built in code: `r.screen("about", |ui| ui.panel(..))`
- HUD elements are built with `r.hud_element(name, hud_anchor::HOTBAR_LEFT, 0, |ui| ..)`
- F3 panels are at `hud_anchor::DEBUG_LEFT`
- Looks come from themes and 9-slice sprites in `assets/ui`, so a resource pack can reskin screens
- Bind values instead of copying them: `ui.progress_bind("energy", style, |c| ..)`, `ui.text_bind(..)`

Also:
- key bindings (`r.key("dash", "R")`)
- packets both ways (`r.packet` on the server, `r.client_packet` on the client, `w.send` / `client.send`)
- frame hooks (`r.client_frame`)
- particles and screen effects from shader snippets in `assets/shaders`
- sounds with `r.sound(..).category(..)` (every category gets its own volume slider), music
- settings that show up in Mod Settings by themselves (`r.config("sparks").bool(true).handle(&SPARKS)`)

## Debugging

`--log mod:mymod=debug` (or `/log` in chat) turns on your `debug!` / `trace!` lines.\
`info!(target: "pipes", ..)` gives you `mod:mymod::pipes` to filter on.

F3 shows your mod's time per tick. `/gizmo` draws debug shapes, your own groups included (`r.gizmo_group("pipes")`).

A panic in your callback disables that callback and names your mod in the log instead of taking the game down.\
Native crashes write a report into `run/crash-reports/` with the mod that was running.

F3+T reloads textures, lang, shaders and UI without restarting.

## Things that make your and other modders' life easier
Don't hardcode other mods' blocks, use a tag or a component.\
Tag `#sand` allows other mods to work with your mod, whereas `base:sand` does not.

Don't do per-object work across the mod boundary in hot paths if a batch exists (systems, events, entity ticks get all entities of a kind at once).

If you had to work around the engine to do something reasonable, that is an engine flaw.\
Tell me about this instead of building a hack, so I can adjust the engine for those cases.

You can contact me on Discord: `@vortex_so`

## Guides for other systems
- [Textures](textures.md): animation, glow, tinted overlays
- [CTM](ctm.md): connected and varied block textures (glass, logs, random stone)
- [Tooltips](tooltips.md): item tooltips, rich text and effects, adding them from other mods and packs
- [Worldgen](worldgen.md): ores, structures, biomes, spawns, new dimensions and terrain presets
- [Models and animations](models.md): mob models in code or Blockbench, keyframes, the first-person hand, armor
- [UI](ui.md): screens, HUD, menus, themes, GUI textures
- [Machines](machines.md): blocks with inventories, recipes, energy
- [Shaders](shaders.md): text effects, panels, mobs, particles, screen effects, shader packs
- [Resource packs](resourcepacks.md): what a pack can replace and how it is laid out
