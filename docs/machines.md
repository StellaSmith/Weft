# Machines guide

Blocks with an inventory, a screen and a purpose: furnaces, chests, generators, batteries.\
This guide builds a machine step by step, then adds energy so machines and cables from different mods work together.

## Note
Examples to copy from:
- `mods/base/src/machines.rs`: the furnace (fuel, a recipe, a lit state) and the chest
- `mods/testmod/src/lib.rs`: a generator, an electric furnace and a battery on the energy contract, plus a `cable` block

## Quick links
[how a machine works](#how-a-machine-works) | [creating a machine](#creating-a-machine) | [the tick](#the-tick) | [the screen](#the-screen) | [recipes](#recipes) | [energy](#energy) | [many machines](#many-machines)

## How a machine works
`r.machine` creates three things that belong together: the **block** players see and place, a **block entity**, and a **menu** with its screen. The block entity is an invisible entity of the same name that sits on the block. It holds the inventory and the machine's data (what is burning, how far along it is) and it is what gets ticked, once per tick for all machines of the kind. It appears and disappears with the block, and data saved with `SAVE` stays in the world.

## Creating a machine
```rust
static FURNACE: Block = Block::named("furnace");
static FURNACE_MENU: Menu = Menu::named("furnace");
static SMELTING: Component<Smelting> = Component::named("base:smelting");

r.entity_component(&SMELTING, Smelting { burn: 0, burn_max: 0, cook: 0, cook_max: 0 }, component_flags::SAVE | component_flags::SYNC_MENU);
r.machine(&FURNACE, &FURNACE_MENU)
    .block(|b| b.hardness(3.5).mineable("pickaxe").needs_tool(0).tab(&FUNCTIONAL))
    .group("input", Slots::single())
    .group("fuel", Slots::single().accepts("fuel"))
    .group("output", Slots::single().output())
    .attach(&SMELTING)
    .screen(|ui| { /* see below */ });
r.system(&SMELTING, tick_furnaces);
```
`block` is the same builder as `r.block`, so everything from the blocks part of the [cheat sheet](modding.md) applies to it. The block entity is named like the block, so `Entity::named("furnace")` finds it if your code needs to.

Slots come in named groups. The machine's inventory holds all of its groups, the engine works out how big it is and which slot is which, and the tick and the screen ask for a group by its name. A group starts from a shape and adds rules:

| | |
|---|---|
| `Slots::single()`, `row(n)`, `column(n)`, `grid(cols, rows)`, `chest(rows)` | how many slots and how the screen lays them out |
| `output()` | items only come out (results) |
| `input()` | items only go in, nothing comes back out by hand (fuel a machine burns) |
| `accepts("fuel")` | only items with that tag or component, any mod's |
| `limit(16)` | at most this many items per slot |
| `large()`, `size(px)` | a bigger slot, 18 GUI pixels by default |
| `background("flame_slot")` | a texture from `gui/` shown while the slot is empty, like the armor silhouettes |
| `tooltip("mymod.fuel_slot")` | a translation shown over the empty slot |
| `priority(n)` | shift-clicks from the inventory fill groups with a higher priority first |

Without `input()` or `output()` a slot takes and gives like a chest's. Shift-clicking an item in the player's inventory puts it into the first group that takes it, groups with an `accepts` rule before plain ones, so ore goes into the input and coal into the fuel slot without you saying anything.

The machine's state is a component, a `value!` struct of plain numbers like any other. Slots hold items, so anything else a machine keeps (heat, a liquid level) goes into a component like this one:
```rust
value! {
    pub struct Smelting { pub burn: u32, pub burn_max: u32, pub cook: u32, pub cook_max: u32 }
}
```
The flags say who gets it. `SAVE` writes it with the world, `SYNC_MENU` sends it to players who have this machine's menu open (progress bars need that), `SYNC_OWNER` sends it to the player who owns the entity (HUD values) and `SYNC_WATCHERS` to everyone who can see it. Since components are named, another mod that declares `base:smelting` with the same layout can read the furnace's state, see "Components and tags" in the [cheat sheet](modding.md#components-and-tags-as-inter-mod-communication).

## The tick
A machine works through its state component: `r.system(&BURNER, f)` hands `f` every loaded machine with that component, their values side by side in one slice, changed in place (and saved and synced like any change). Measured on a thousand machines, that costs a few hundredths of what asking for each machine's component costs, so it is the one way machines tick:
```rust
fn tick_generators(b: &mut SystemBatch<Burner>) {
    let w = b.world();
    for (id, m) in b.iter_mut() {
        let slot = w.inventory(id).group("fuel");
        if m.burn == 0 {
            let fuel = slot.get(0);
            // `fuel` is the common component, so any mod's fuel works
            if let Some(ticks) = FUEL.on_item(fuel.item).filter(|_| !fuel.is_empty()) {
                m.burn = ticks;
                slot.set(0, use_one(fuel));
            }
        }
        if m.burn > 0 {
            m.burn -= 1;
        }
    }
}
```
`w.inventory(id).group("fuel")` is one group of the machine's slots, numbered from 0 (`get`, `set`, `len`); the whole inventory also has `insert`, which returns what didn't fit. Give each kind of machine a state component of its own (the generator's `Burner`, the furnace's `Smelting`), so its system reaches only its machines. For stacks there are `use_one` (a lava bucket leaves its bucket), `shrink`, `grow`, `stacks_with` and `max_stack`, and the rest of the world is in `w` too (`get`, `set`, `play_sound`, `emit`).

To show that a machine is working, give the block a property and flip it from the tick. The furnace has `.facing(..)` and `.prop("lit", ["false", "true"])`, and its tick does
```rust
w.set_prop(pos, "lit", if lit { "true" } else { "false" });
```
which keeps the way it faces.
`tick_furnaces` in `mods/base/src/machines.rs` is the whole thing in one place, with the recipe, the lit state and an event.

## The screen
`screen(|ui| ..)` builds the window. `ui.group("input")` draws a group as its grid, `ui.group_slot("input", 1)` a single slot of it when a layout places them apart, and `ui.player_inventory(true)` the player's inventory and hotbar, every machine menu has them:
```rust
.screen(|ui| {
    ui.panel("panel", S::col().pad(7.0).gap(4.0).stretch(), |ui| {
        ui.title();
        ui.row(S::row().gap(8.0), |ui| {
            ui.group("input");
            ui.recipe_arrow_bind(|c| c.menu_component(&SMELTING).map_or(0.0, |s| ratio(s.cook, s.cook_max)));
            ui.group("output");
        });
        ui.player_inventory(true);
    });
})
```
`ui.title()` is the block's name, so the window says "Furnace" without a translation of its own; a chest a player renamed (`/rename Loot` on the item, the anvil's job) shows that name instead, and keeps it when it's broken and picked up again. A menu opened from code has `menu.<ns>.<name>` as its title.
The arrow reads the machine's component through `menu_component`, which is why it needs `SYNC_MENU`. The screen belongs to the menu and not to a single block, so every machine of one kind shows the same window. Parts, layout and themes are in the [UI guide](ui.md). Using the block already opens the menu, and `world.open_menu(player, &MENU, owner)` does the same from code.

## Recipes
Anything where inputs turn into outputs over time is a recipe kind, and a machine can use the base game's or one of its own:
```rust
static SMELTING_RECIPES: RecipeKind = RecipeKind::named("base:smelting");

r.machine(&ELECTRIC_FURNACE, &FURNACE_MENU)
    .recipes(&SMELTING_RECIPES)          // its arrow opens them, the recipe viewer shows it next to them
    ..;
// in the tick:
if let Some((output, time)) = recipe_process(&SMELTING_RECIPES, input.item) { .. }
```
`recipe_process` gives the output and the time in ticks for one input. Recipes are added the usual way, `r.shapeless(&SMELTING, "ruby", 1, &["ruby_ore"], 200)`, so every machine that uses the kind smelts it, whichever mod it comes from. `recipes(..)` makes the machine's `ui.recipe_arrow_bind(..)` (or `ui.recipe_arrow()` without progress) open the kind in the recipe viewer, with "Recipes" over it, and shows the machine there as where those recipes are made.
An ordinary menu works the same way: `r.menu(&CRAFTING).recipes(&CRAFTING_RECIPES)`, and a block with `.opens(&CRAFTING)` opens it and becomes a station of those recipes (that's the whole crafting table). The grid's size comes from its group, `m.group("grid").cols()`.

A recipe can say more than one output and a time:
```rust
r.shapeless(&ALLOYING, "tin_ingot", 2, &["base:iron_ingot", "base:coal"], 100)
    .input_amounts(&[3, 1])                              // 3 iron and 1 coal
    .chance_output("base:diamond", 1, 0.05)       // and a diamond now and then
    .value(&ENERGY_PER_TICK, 32);                 // data the machine reads
```
Inputs and outputs can be fluids, `fluid:base:water` with the count in millibuckets, and `hidden()` keeps a recipe out of the viewer. A machine with several inputs finds its recipe with `recipe_find(&KIND, &stacks)`, which gives the `time()`, the `outputs()` with their chances, the `value(&COMPONENT)` and `consume(&mut stacks)` to take the inputs out.

A kind of your own is registered with `r.recipe_kind(&KIND)`, and recipes that name a kind nobody registered fail at load. The same call says how the recipe viewer shows it:
```rust
r.recipe_kind(&ALLOYING)
    .inputs(2, 1).outputs(2, 1)                   // always these grids, like a crafting table's 3 x 3
    .info_line("recipe.mymod.alloying.info", &["mymod:energy_per_tick", "time"]);
```
`info_line` is a line under every recipe, a translation like `Uses {0} FE/t for {1} s` whose numbers come from the recipe's values and its time. For a layout of your own, `layout(|ui, recipe| ..)` builds each recipe with the usual UI parts plus `ui.recipe_inputs()`, `ui.recipe_outputs()` and `ui.recipe_input(i)` for its slots (`infusing` in testmod is one, with a fluid).

## Energy
Generators, cables and machines from different mods work together through one contract in the engine. An entity with the `core:energy` component takes part, and a block tagged `conductor` is a cable. The block entities next to a cable, or a chain of them, form a network, and once per tick the engine shares what the generators offer among the machines that want it.

```rust
r.machine(&ELECTRIC_FURNACE, &FURNACE_MENU)
    .group("input", Slots::single())
    .group("output", Slots::single().output())
    .attach(&WORK)
    .attach_with(&ENERGY, Energy { stored: 0, capacity: 40_000, max_in: 400, max_out: 0 });
r.system(&WORK, tick_furnaces);
r.block(&CABLE).model_box([5.0, 5.0, 5.0], [11.0, 11.0, 11.0]).tag("conductor");
```
`Energy` is `stored` and `capacity`, plus `max_in` and `max_out`, the most it takes from or gives to the network per tick. A generator has `max_in: 0`, a machine `max_out: 0`, and a battery has both, without feeding itself. In their systems, generators add to `stored` and machines spend it:
```rust
let Some(e) = w.energy_mut(id) else { continue };
e.stored = (e.stored + 40).min(e.capacity);      // a generator, per tick
// a machine: if e.stored >= COST { e.stored -= COST; work }
```
`w.energy(id)` reads it, and `energy_insert` and `energy_extract` move energy with the limits respected (`simulate = true` only reports how much would move). Networks are rebuilt when a cable or a machine next to one is placed or broken, not every tick.

There are no units, conversion or losses along the way. Testmod uses 40 per tick and capacities in the tens of thousands, so if your mod should play along with others, stay in that range.

## Many machines
Systems already run once per tick and region with every machine at once, so thousands cost little. Work that only changes the state and touches no inventory is the cheapest of all:
```rust
r.system(&HEAT, |batch| for (_, heat) in batch.iter_mut() { *heat = heat.saturating_sub(1); });   // HEAT: Component<u32>
```

Machines have no built-in way to pass items to each other, so a pipe or a hopper is something you write: do it in a tick with `inventory(id).insert(..)`.
