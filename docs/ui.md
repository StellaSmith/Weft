# UI guide

Screens, HUD parts and the menus of inventories and machines, plus the theme system that decides how all of it looks.\
Base's inventory, furnace, chest and health bars are built from the same small set of parts you get, so anything you see in the game you can make.

## Note
Examples to copy from:
- `mods/base/src/player.rs`: the HUD bars, the inventory and the crafting table
- `mods/base/src/machines.rs`: the furnace and the chest, with bars that follow data
- `mods/testmod/src/lib.rs`: a screen of its own (`about`), machine screens on a themed panel, an F3 panel
- `mods/base/assets/ui/theme.theme` and `mods/testmod/assets/ui/theme.theme`

## Quick links
[the parts](#the-parts) | [layout](#layout) | [live values](#live-values) | [screens](#screens) | [HUD](#hud) | [menus](#menus) | [themes](#themes) | [GUI textures](#gui-textures) | [shaders and animations](#shaders-and-animations)

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
| `panel(class, s, ..)` | a column with a themed background, `"panel"` is the usual window |
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
- for text: `class("title")` (the theme's look, also `dim`, `button`, `chat`), `scale(2.0)`, `no_wrap()` and `mono()`, which puts every glyph in a digit-wide cell so numbers don't jump (`{mono}..{/mono}` does it for part of a text)
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
The `class` ("hearts", "arrow", "flame") is a theme class, which is where the textures come from.

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
r.client_command("about", "", |c| { if let Some(cl) = c.client() { cl.open_screen(&ABOUT); } });
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
| `DEBUG_LEFT`, `DEBUG_RIGHT` | a panel of the F3 overlay, titled by the element's name as a translation key |

F3 panels from mods take the key numbers after the engine's five, so the first one is F3+6.

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

## Themes
How a screen looks comes from themes: the `ui/theme.theme` of each mod, merged together. A pack's file replaces the one of the mod it's made for. Lines are `class.property = value`, and comments are whole lines starting with `#`:
```
panel.texture = base:gui/panel
panel.text = #404040
button.hover = base:gui/button_hover
```
A part asks for its class, so `ui.panel("panel", ..)` looks up `panel.texture`, `panel.text` and so on, and a property the class doesn't have comes from `default.<property>`. Colors are `#rrggbb`, or `#rrggbbaa` when they need transparency. Base sets these, and your own classes can use the same properties (`testmod_panel` is one):

| class | properties |
|---|---|
| `panel` | `texture`, `text`, `dim_text`, `shadow` (`none` for no text shadow) |
| `well` | `texture`, `text`, `dim_text` (a sunken area: lists, the inventory figure's frame) |
| `button` | `texture`, `hover`, `pressed`, `disabled`, `text`, `hover_text`, `disabled_text`, `shadow` |
| `slider` | `track`, `knob`, `knob_hover` |
| `input` | `texture`, `focused`, `text`, `selection` |
| `slot` | `texture`, `highlight` (over the hovered slot), or `fill`, `light` and `dark` without a texture |
| `hotbar` | the same for the HUD's hotbar |
| `scrollbar` | `track`, `handle`, `handle_hover` |
| `tooltip` | `texture`, `frame` (drawn over it), `text` |
| `background` | `texture`, `color` (behind menus) |
| `hearts`, `food`, `armor` | `full`, `half`, `empty` |
| `arrow`, `flame` | `background`, `fill_texture` |
| `air` | `fill` |
| `chat`, `debug` | `text`, `shadow` |

A bar of your own needs two textures and a line each, `energy.background = mymod:gui/energy_bg` and `energy.fill_texture = mymod:gui/energy_fill`, and then `ui.progress_bind("energy", ..)` draws it. A resource pack restyles everything the same way, with its own theme file.

## GUI textures
A texture used as a background has to be stretched over parts of any size, and a `.gui` file next to it says how (`panel.gui` for `panel.png`):
```
scaling = nine_slice
border = 4
```
`scaling` is `nine_slice` (the corners keep their size, the edges and middle stretch), `tile` or `stretch`. `border` is one number for all sides or `left top right bottom`, in GUI pixels. Two more keys exist for special cases: `size = w h` for an image with more pixels than the GUI pixels it stands for, and `outset = n` for textures that reach outside their box, like shadows. An image without one is simply stretched.

## Shaders and animations
A theme class can have `shader = <name>`, and the panel is then filled by `shaders/gui/<name>.wgsl` instead of a texture:
```
testmod_panel.shader = testmod:circuit
```
The file defines `fn fill(p: GuiPixel) -> vec4<f32>`, where `p` has the position across the panel, its size in pixels and the time. The circuit board of the testmod machines is one, see the [shaders guide](shaders.md) for the details.

A class can also have `animation = <name>`, pointing at `animations/<name>.animation.json`. An animation called `open` plays once when the screen opens and `idle` loops afterwards. Both move one bone named `node`, with `position` (GUI pixels, y up), `rotation` (z only, clockwise degrees) and `scale`, around the center of the part. Testmod's panel grows from 85% and rises 10 pixels this way, and the file format is the one from the [models guide](models.md#animations). Item icons inside an animated panel don't follow it yet.
