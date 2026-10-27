# UI guide

Screens, HUD parts and the menus of inventories and machines, and how all of it looks.\
Base's inventory, furnace, chest and health bars are built from the same small set of parts you get, so anything you see in the game you can make.

## Note
Examples to copy from:
- `mods/base/src/player.rs`: the HUD bars, the inventory and the crafting table
- `mods/base/src/machines.rs`: the furnace and the chest, with bars that follow data
- `mods/testmod/src/lib.rs`: a screen of its own (`about`), machine screens on a panel of its own class, an F3 panel
- `mods/base/assets/textures/gui/` and the `color.*` lines of `mods/base/assets/lang/en_us.lang`: the base game's look

## Quick links
[the parts](#the-parts) | [layout](#layout) | [live values](#live-values) | [screens](#screens) | [HUD](#hud) | [menus](#menus) | [looks](#looks) | [GUI textures](#gui-textures) | [shaders and animations](#shaders-and-animations)

## The parts
A screen is a tree of parts built by a function that receives a `Ui`. Parts that hold others take a closure for their children:

```rust
ui.panel("panel", S::col().pad(7.0).gap(4.0), |ui| {
    ui.text("examplemod.title");
    ui.row(S::row().gap(8.0), |ui| {
        ui.group("input");
        ui.button("examplemod.go", S::default(), |_| {});
    });
});
```

| part | what it is |
|---|---|
| `column(s, ..)`, `row(s, ..)` | children one under another, or side by side |
| `panel(class, s, ..)` | a column with the class's background, `"panel"` is the usual window |
| `text(key)` | a translation. `text_args(key, &[Arg::Int(5)])` fills `{0}` and the rest, `text_styled(s, key, args)` takes a style too (class, size, one line, `mono`) |
| `button(key, s, on_click)` | a button with a translated label (`button_args` with arguments); the closure runs on click and can drive the game through `c.client()` (open screens and worlds, quit, change settings), even in the main menu |
| `input(value, max, s, on_change)` | a one-line text field; the closure gets the text after every edit |
| `slider(s, value, on_change)` | a slider at `value` (0..1, read every frame); the closure gets where it is dragged |
| `item(id, count, s)` | an item's icon with its count |
| `image(texture, w, h)` | a GUI texture such as `base:gui/arrow_bg` |
| `spacer()` | an empty part that grows, to push others apart or center them |
| `group(name)`, `group_slot(name, i)` | a menu's slots, see [menus](#menus) |
| `player_model(w, h)` | the player with what they wear, the figure in the inventory |
| `player_inventory(title)` | the player's inventory and hotbar in a menu |
| `progress_bind`, `icons` | bars and icon rows driven by live values |

Text is a translation key rather than a raw string, which keeps everything translatable and lets it use the rich text of the [tooltips guide](tooltips.md), pictures included. Text that isn't words of yours (a number, a player's name) goes in as an argument, `Arg::Text(..)` shows exactly what it holds; `Arg::Float(v, 2)` has two decimals. Any part with a style can have a tooltip too: `S::col().tooltip("examplemod.what_this_does")`, or `tooltip_args(key, &[..])` with arguments.

The engine's own pause, death and settings screens are built from exactly these parts, so a screen that replaces one can look and work the same.

## Layout
Layout works like flexbox, and all sizes are in GUI pixels, so the GUI scale in the settings makes them bigger or smaller. `S` describes one part, starting from `S::col()` or `S::row()` (rows center their children vertically, columns start at the top left):
- `gap(n)` is the space between children and `pad(n)` the space around them inside
- `size(w, h)` and `w(n)` fix a size, otherwise a part is as big as its content
- `grow()` takes whatever space is left in the parent
- `center()` and `end()` position the children along the main axis
- `stretch()` makes children fill the other axis, so rows get as wide as their column and a `spacer()` can center things
- `w_pct(p)`, `h_pct(p)` size a part in percent of its parent; `min_w`, `min_h`, `max_w` limit it
- `self_align(abi::ui_align::END)` places one part differently from its siblings
- `scroll()` lets the children scroll with the wheel, with a bar when they don't fit
- for text: `class("title")` (its color, also `dim`, `button`, `chat`), `scale(2.0)`, `no_wrap()` and `mono()`, which puts every glyph in a digit-wide cell so numbers don't jump (`{mono}..{/mono}` does it for part of a text)
- for buttons: `disabled(true)` and `selected(true)` (the open tab)

## Live values
Text and bars can change while a screen is open. A binding's closure runs every frame, and the part is redrawn when the value changed.
- `text_bind(|view, out| out.set("examplemod.energy", &[Arg::Int(stored), Arg::Int(capacity)]))` for text from data
- `progress_bind(class, s, |view| 0.0..1.0)` for a bar, and `progress_bind_dir(.., dir, ..)` to fill another way, `abi::ui_dir::UP` for a flame
- `icons(class, count, |view| value)` for a row of icons that shows a number in half-icon steps, like hearts. `icons_rtl` fills from the right, like hunger. A negative value hides the row

`view` is a read-only look at the client, the same in HUD bindings, screens and frame hooks (`c.view()`): `view.me()` has your health and position, `view.inventory_group("core:equipment")` the stacks of a group of your inventory, `view.component(&C)` a synced component of yours, `view.creative()` the game mode, and `view.menu_component(&C)` a component of the machine whose menu is open (it needs the `SYNC_MENU` flag on that component, see the [machines guide](machines.md#creating-a-machine)).
Anything the client already has is better read than mirrored into a component: base's armor bar adds up the points of what you wear, the way the server's damage does. A component read without the flag that sends it there says so in the log.

```rust
static HEALTH: HudElement = HudElement::named("health");
r.hud_element(&HEALTH, hud_anchor::HOTBAR_LEFT, 0, |ui| {
    ui.icons("hearts", 10, |c| if c.creative() { -1.0 } else { c.me().map_or(20.0, |s| s.health.ceil()) })
});
```
The `class` ("heart", "arrow", "flame") names the textures, see [looks](#looks).

## Screens
`r.screen(&SCREEN, |ui| ..)` registers a screen of your own, and what it does besides showing parts is declared on it, the same properties the engine's screens have:
- `.pauses()`: single player stops while it is open
- `.keep_on_escape()`: Escape doesn't close it (the death screen)
- `.closes_on_respawn()`
- `.on_key(|view, key| ..)`: keys pressed while it is on top (`Enter`, `KeyE`), before Escape
- `.shows_chat()`: like the chat screen, the HUD leaves the messages to it and Enter sends the text of its input of class `chat`

```rust
static ABOUT: Screen = Screen::named("about");
r.screen(&ABOUT, |ui| {
    ui.panel("panel", S::col().pad(8.0).gap(4.0), |ui| {
        ui.text("examplemod.about.title");
    });
});
r.client_command("about", "", |c| { c.client().open_screen(&ABOUT); });
```
`open_screen(&ABOUT)` works from a client command, a key or a button, and the command line takes it too as `--screen examplemod:about`.

Registering an engine screen's handle replaces that screen, so `r.screen(&screens::PAUSE, ..).pauses()` is your own pause screen. The engine's are in `screens::*`: `TITLE`, `PAUSE`, `SETTINGS`, `LANGUAGE`, `CONTROLS`, `SOUND`, `MOD_SETTINGS`, `MODS`, `PROFILE`, `WORLDS`, `CREATE_WORLD`, `MULTIPLAYER`, `RESOURCE_PACKS`, `SHADER_PACKS`, `CHAT` and `DEATH`. The engine's own screens open each other through the same names, so a replaced settings screen is the one the main menu's button opens.

Most of the time a whole new screen is more than you need. A screen part adds to a place of a screen and leaves the rest alone, so several mods can add their buttons to the main menu side by side:
```rust
r.screen_part("core:title", "buttons", 0, |ui| {
    ui.button("examplemod.guide", S::default(), |c| {
        c.client().open_screen("examplemod:guide");
    });
});
```
The places are `buttons` and `side` (the space right of the menu) on `core:title`, and `buttons` on `core:pause` and `core:settings`; a lower order comes first. A mod that replaces one of these screens keeps the others' parts by building the place in its own: `ui.screen_parts("core:title", "buttons")`.

The main menu takes its logo from the translation `menu.logo` and shows `menu.news` beside it, so a pack or a mod can put its own picture there (`{img:..}`).

## HUD
`r.hud_element(name, anchor, order, |ui| ..)` puts a part on the in-game overlay. The anchor says where, and `order` 0 sits nearest the anchor's edge.

| anchor (`hud_anchor::..`) | where |
|---|---|
| `TOP_LEFT`, `TOP_RIGHT`, `BOTTOM_LEFT`, `BOTTOM_RIGHT` | the corners of the window |
| `HOTBAR`, `HOTBAR_LEFT`, `HOTBAR_RIGHT` | over the hotbar, on its left and right (health and hunger live there) |
| `ABOVE_HOTBAR` | centered above all of that (the held item's name) |
| `DEBUG_LEFT`, `DEBUG_RIGHT` | a panel of the F3 overlay, titled by the element's name as a translation key |

F3 panels from mods take the key numbers after the engine's five, so the first one is F3+6.

The engine's own parts are elements like these, `core:hotbar` and `core:held_name`, and a player moves any of them with lines in `config/hud.txt`: `core:held_name = top_left` or `base:food = top_right 2` (anchor, then order).

Labels a mod draws (`w.draw_text`, `client.draw_text`, `client.screen_text`) are text like everywhere else, icons and `{mono}` included, with a size: 1 is menu text, 2 twice that.

## Menus
A menu is a set of slots that the server runs, plus the screen that shows them. `world.open_menu(player, &MENU, owner)` opens one, and `r.player_menu(&INVENTORY)` picks the one the inventory key opens. Its slots come in named groups, described once; the server and the screen both build them from that, so nothing is numbered by hand. Here is the crafting table:

```rust
static CRAFTING: Menu = Menu::named("crafting_table");
r.menu(&CRAFTING)
    .group("grid", Slots::grid(3, 3))
    .group("result", Slots::single().output().large())
    .player_inventory()
    .on_change(|m| {
        // a slot changed: show what the grid makes
        let grid = m.group("grid");
        let cells: Vec<ItemStack> = (0..grid.len()).map(|i| grid.get(i)).collect();
        m.group("result").set(0, recipe_match(&CRAFTING_RECIPES, &cells, 3, 3));
    })
    .on_take(|m| /* the result was taken: use up the grid */ {})
    .screen(|ui| {
        ui.panel("panel", S::col().pad(7.0).gap(4.0), |ui| {
            ui.row(S::row().gap(8.0), |ui| {
                ui.group("grid");
                ui.group("result");
            });
            ui.player_inventory(true);
        });
    });
```
A group of a plain menu is the menu's own slots, given back to the player when the menu closes; `.equipment()` adds the player's equipment slots (drawn by `ui.equipment()`), and machines' groups are the machine's inventory. `player_inventory()` adds the player's inventory and hotbar, which `ui.player_inventory(title)` draws. The shapes and rules a group can have (`output`, `input`, `accepts`, `limit`, `large`, `background`, `tooltip`, `priority`) are listed in the [machines guide](machines.md#creating-a-machine). In `on_change` and `on_take`, `m.group(name)` reads and writes a group's slots by their place in it.

Machines are menus tied to a block, with their own data and tick. The [machines guide](machines.md) covers them, including the component flags that their screens read.

## Looks
How a part looks follows from its class by name, there is no theme file. Class `button` is the texture `gui/button.png`; its states are `gui/button_hover.png`, `_pressed`, `_disabled` (and `_focused` for inputs). A bare class is looked up in the resource packs, then in the mods from the last loaded, then in the engine, so base's `base/textures/gui/button.png` dresses every button and a mod's own class (`testmod_panel`) lives in its own folder. `mymod:thing` looks only in `mymod`. A class without a texture shows a magenta and black checkerboard, like a missing block texture, so a typo can't hide.

| class | textures |
|---|---|
| `panel`, `well`, `sidebar`, `news`, `tooltip`, `overlay` (behind menus), `background`, `suggest`, `debug_panel`, `death` | the class |
| `button` | `_hover`, `_pressed`, `_disabled` |
| `input` | `_focused`, plus `input_selection` and `caret` |
| `slider`, `slider_knob` | `slider_knob_hover` |
| `scrollbar`, `scrollbar_handle` | `scrollbar_handle_hover` |
| `slot`, `recipe_slot`, `hotbar` | `hotbar_selected` (around the selected one), `slot_highlight` (over the hovered one) |
| `crosshair` | the class |
| `heart`, `food`, `armor`, `air` | `_full`, `_half`, `_empty` |
| `arrow`, `flame` and bars of your own | the class behind, `_fill` filling it |

A bar of your own is two pictures and nothing else: `gui/energy.png` and `gui/energy_fill.png`, then `ui.progress_bind("energy", ..)` draws it.

Text colors are named colors in the language files: `color.<class>` for its text, and `color.<class>_hover`, `_disabled`, `_dim` and `_shadow` (`none` for no shadow) where they differ; whatever a class doesn't set comes from `color.default`. They hold `{#rrggbb}` like any text, so a pack changes one with a single line in its `lang/en_us.lang`:
```
color.button = {#d8dceb}
color.button_hover = {#ffffff}
```
The same names work inside any text: `{color:debug_title}Performance` takes the color `color.debug_title` holds. If you want your mod to be easy to restyle, name your colors this way instead of writing `{#hex}` in every line, so they are set once and changed once.

## GUI textures
A texture used as a background has to be stretched over parts of any size, and a `.gui` file next to it says how (`panel.gui` for `panel.png`):
```
scaling = nine_slice
border = 4
```
`scaling` is `nine_slice` (the corners keep their size, the edges and middle stretch), `tile` or `stretch`. `border` is one number for all sides or `left top right bottom`, in GUI pixels. Two more keys exist for special cases: `size = w h` for an image with more pixels than the GUI pixels it stands for, and `outset = n` for textures that reach outside their box, like shadows. An image without one is simply stretched.

## Shaders and animations
A class's `.gui` can name a shader, and the part is then filled by `shaders/gui/<name>.wgsl` instead of the picture (`testmod_panel.gui`):
```
border = 2
shader = testmod:circuit
animation = testmod:ui_panel
```
The file defines `fn fill(p: GuiPixel) -> vec4<f32>`, where `p` has the position across the panel, its size in pixels and the time. The circuit board of the testmod machines is one, see the [shaders guide](shaders.md) for the details.

The `animation` line points at `animations/<name>.animation.json`. An animation called `open` plays once when the screen opens and `idle` loops afterwards. Both move one bone named `node`, with `position` (GUI pixels, y up), `rotation` (z only, clockwise degrees) and `scale`, around the center of the part. Testmod's panel grows from 85% and rises 10 pixels this way, and the file format is the one from the [models guide](models.md#animations). Item icons inside an animated panel don't follow it yet.

The picture itself can be animated like a block texture: frames stacked down in one file and an `.anim` next to it, `frametime = 3` (ticks per frame) and `frame_height = 8` when the frames aren't square, as testmod's `energy_fill` does.
