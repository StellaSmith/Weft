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
static FURNACE_MENU: Menu = Menu::new();
static SMELTING: Component<Smelting> = Component::new("base:smelting");

r.machine("furnace", &FURNACE_MENU)
    .block(|b| b.hardness(3.5).mineable("pickaxe").needs_tool(0).tab("functional"))
    .slots(&[0, 0, slot_flags::OUTPUT])        // input, fuel, output
    .component(&SMELTING, Smelting { burn: 0, burn_max: 0, cook: 0, cook_max: 0 }, component_flags::SAVE | component_flags::SYNC_MENU)
    .on_tick(tick_furnaces)
    .screen(|ui, player| { /* see below */ });
```
`block` is the same builder as `r.block`, so everything from the blocks part of the [cheat sheet](modding.md) applies to it. `slots` has one entry per slot, `0` for a normal one and `slot_flags::OUTPUT` for one that only gives items out, and `accepts(1, "fuel")` limits a slot to items with that tag or component, any mod's. `entity_handle(&E)` gives you an `Entity` static if your code needs to find the block entity.

The machine's state is a component, a `value!` struct of plain numbers like any other. Slots hold items, so anything else a machine keeps (heat, a liquid level) goes into a component like this one:
```rust
value! {
    pub struct Smelting { pub burn: u32, pub burn_max: u32, pub cook: u32, pub cook_max: u32 }
}
```
The flags say who gets it. `SAVE` writes it with the world, `SYNC_MENU` sends it to players who have this machine's menu open (progress bars need that), `SYNC_OWNER` sends it to the player who owns the entity (HUD values) and `SYNC_WATCHERS` to everyone who can see it. Since components are named, another mod that declares `base:smelting` with the same layout can read the furnace's state, see "Components and tags" in the [cheat sheet](modding.md#components-and-tags-as-inter-mod-communication).

## The tick
`on_tick` receives every loaded machine of the kind and walks over them:
```rust
fn tick_generators(t: &EntityTick) {
    let w = t.world();
    for &id in t.ids() {
        let inv = w.inventory(id);
        let Some(m) = w.component_mut(id, &MACHINE) else { continue };
        if m.burn == 0 {
            let fuel = inv.get(0);
            // `fuel` is the common component, so any mod's fuel works
            if let Some(ticks) = FUEL.on_item(fuel.item).filter(|_| !fuel.is_empty()) {
                m.burn = ticks;
                inv.set(0, use_one(fuel));
            }
        }
        if m.burn > 0 {
            m.burn -= 1;
        }
    }
}
```
`w.inventory(id)` is the machine's slots (`get`, `set`, and `insert`, which returns what didn't fit), and `w.component_mut(id, &C)` its data, changed in place and saved and synced by itself. For stacks there are `use_one` (a lava bucket leaves its bucket), `shrink`, `grow`, `stacks_with` and `max_stack`, and the rest of the world is in `w` too (`get`, `set`, `play_sound`, `emit`).

To show that a machine is working, give the block states and flip between them from the tick. The furnace has facing times lit states (`facing_variants(.., 2, |dir, lit, s| ..)`) and does
```rust
w.set(pos, FURNACE.state(Facings::HORIZONTAL.index(dir, lit as u32)));
```
`tick_furnaces` in `mods/base/src/machines.rs` is the whole thing in one place, with the recipe, the lit state and an event.

## The screen
`screen(|ui, player| ..)` builds the window. Slots are numbered in the order of `slots(..)`, so the furnace has `0`, `1` and `2`, and `player` is where the player's own slots start, right after the machine's:
```rust
.screen(|ui, player| {
    ui.panel("panel", S::col().pad(7.0).gap(4.0).stretch(), |ui| {
        ui.text("container.furnace");
        ui.row(S::row().gap(8.0), |ui| {
            ui.slot(0);
            ui.progress_bind("arrow", S::default().size(24.0, 16.0), |c| c.menu_component(&SMELTING).map_or(0.0, |s| ratio(s.cook, s.cook_max)));
            ui.slot(2);
        });
        ui.player_inventory(player, true);
    });
})
```
The arrow reads the machine's component through `menu_component`, which is why it needs `SYNC_MENU`. The screen belongs to the menu and not to a single block, so every machine of one kind shows the same window. Parts, layout and themes are in the [UI guide](ui.md). Using the block already opens the menu, and `world.open_menu(player, &MENU, owner)` does the same from code.

## Recipes
Anything where an input turns into an output over time is a recipe kind, and a machine can use the base game's or one of its own:
```rust
static SMELTING_RECIPES: RecipeKind = RecipeKind::named("base:smelting");

r.recipe_station(&SMELTING_RECIPES, "electric_furnace");     // the recipe viewer lists this machine for those recipes
// in the tick:
if let Some((output, time)) = recipe_process(&SMELTING_RECIPES, input.item) { .. }
```
`recipe_process` gives the output and the time in ticks for an input. Recipes are added the usual way, `r.shapeless(&SMELTING, "ruby", 1, &["ruby_ore"], 200)`, so every machine that uses the kind smelts it, whichever mod it comes from. A kind of your own (a crusher) is registered with `r.recipe_kind(&KIND)`, and recipes that name a kind nobody registered fail at load.

## Energy
Generators, cables and machines from different mods work together through one contract in the engine. An entity with the `core:energy` component takes part, and a block tagged `conductor` is a cable. The block entities next to a cable, or a chain of them, form a network, and once per tick the engine shares what the generators offer among the machines that want it.

```rust
r.machine("electric_furnace", &FURNACE_MENU)
    .slots(&[0, slot_flags::OUTPUT])
    .attach_with(&ENERGY, Energy { stored: 0, capacity: 40_000, max_in: 400, max_out: 0 })
    .on_tick(tick_furnaces);
r.block("cable").model_box([5.0, 5.0, 5.0], [11.0, 11.0, 11.0], "cable").tag("conductor");
```
`Energy` is `stored` and `capacity`, plus `max_in` and `max_out`, the most it takes from or gives to the network per tick. A generator has `max_in: 0`, a machine `max_out: 0`, and a battery has both, without feeding itself. In the tick, generators add to `stored` and machines spend it:
```rust
let Some(e) = w.energy_mut(id) else { continue };
e.stored = (e.stored + 40).min(e.capacity);      // a generator, per tick
// a machine: if e.stored >= COST { e.stored -= COST; work }
```
`w.energy(id)` reads it, and `energy_insert` and `energy_extract` move energy with the limits respected (`simulate = true` only reports how much would move). Networks are rebuilt when a cable or a machine next to one is placed or broken, not every tick.

There are no units, conversion or losses along the way. Testmod uses 40 per tick and capacities in the tens of thousands, so if your mod should play along with others, stay in that range.

## Many machines
`on_tick` is called once per tick with all machines of the kind, which is fine even for thousands. When the work only touches a component and no inventory, a system is cheaper, since it gets every value of that component in one slice and makes no host call per entity:
```rust
r.system(&HEAT, |batch| for (_, heat) in batch.iter_mut() { *heat = heat.saturating_sub(1); });   // HEAT: Component<u32>
```
Systems are covered under "Hooking into the game" in the [cheat sheet](modding.md#hooking-into-the-game).

Machines have no built-in way to pass items to each other, so a pipe or a hopper is something you write: do it in a tick with `inventory(id).insert(..)`.
