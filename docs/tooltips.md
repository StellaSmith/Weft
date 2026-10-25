# Tooltips guide

Tooltips are the boxes that follow the cursor over items, creative tabs and settings.\
They are written in the language files, so there is no code for the usual case, and any mod or
resource pack can add or change them, and even for things in other mods.

## Note
The text is a normal translation, so everything from the rich text list below works in it.\
Examples to copy from: `mods/base/assets/lang/en_us.lang` (`block.base.furnace.tooltip`)\
and `mods/testmod/assets/lang/en_us.lang` (`item.testmod.rune.tooltip`, with effects and conditions).

## Quick links
[rich text](#rich-text) | [effects](#effects) | [conditions](#conditions) | [item tooltips](#item-tooltips) | [other tooltips](#other-tooltips) | [adding from outside](#adding-from-outside) | [look](#look) | [limits](#limits)

## Rich text
Tags in the translation color the text, show keys and icons, hide parts or animate them.\
They work in any translation, not only in tooltips:

| tag | what it does |
|---|---|
| `{#rrggbb}text{/}` | colors the text, `{/}` goes back to the default |
| `{item:base:coal}` | draws the item's icon inline |
| `{key:core:sneak}` | the key the player has bound to that action |
| `{lang:some.key}` | inserts another translation, so a shared phrase is written once |
| `{on_key:core:sneak}text{/on_key}` | shown only while the key is held; `!` in front inverts it; raw keys work too, but it's recommended to not use them |
| `{if:creative}text{/if}` | shown while a condition holds, `!` inverts it: `{if:!creative}text{/if}` |
| `{fx:core:wave}text{/fx}` | animates the text with an effect |
| `{0}` ... `{9}` | arguments given by the code in order (durability uses them) |
| `{{` | a literal `{` |

Hidden parts (`on_key`, `if`) are basically instant, the text is just laid out again when they change.

### Effects
`{fx:...}` animates the letters of the text. The engine has `core:rainbow`, `core:wave`, `core:pulse`
and `core:shake`.

A new one is a file `shaders/fx/<name>.wgsl` in any mod or pack, used as `{fx:mymod:<name>}`.\
The file has one function that gets a letter and returns it moved and/or tinted. What a letter carries (its offset, color, position in the text and so on) is listed in the [shaders guide](shaders.md#text-effects), so here is just a small example, letters that jump sideways now and then (the real one is `mods/testmod/assets/shaders/fx/glitch.wgsl`):
```wgsl
fn effect(g: Glyph) -> Glyph {
    var o = g;
    let t = floor(g.time * 8.0);
    let h = fract(sin(g.index * 91.7 + t * 13.1) * 43758.5453);
    if h > 0.8 {
        o.offset.x = (h - 0.9) * 20.0 * g.scale;
    }
    return o;
}
```

### Conditions
`{if:name}` shows text while a condition holds.\
The engine's are `creative`, `survival`, `spectator`, `debug` and `dev`. Your own are registered in the mod
and read the client, so they see the local player only:
```rust
r.text_condition("charged", |v| v.player_component(&CHARGE).is_some_and(|c| *c > 0));
```
then `{if:mymod:charged}Charged!{/if}` in any translation.

## Item tooltips
Hovering an item or block shows its name, and `<name key>.tooltip` under it if the language has one.\
The name line is white, the tooltip lines are gray by default (a color you set wins).\
The same tooltip shows in the inventory, the creative list and the recipe viewer. A block's is
`block.<modname>.<name>.tooltip`.

```
item.examplemod.ruby = Ruby
item.examplemod.ruby.tooltip = A red gem that smelts {#ffe070}8{/} items.
```

A new line is `\n`, or an indented line which continues the previous value on a new line:
```
block.examplemod.lamp.tooltip = Glows warmly.
    Does not burn.
```

The box is at most 320 GUI pixels wide, longer lines wrap.\
Damaged tools get a `Durability: 12 / 250` line by themselves (its translation is `tooltip.durability`).

Long details usually hide behind the sneak key, so the tooltip stays short until the player wants to read that.\
`{lang:tooltip.more}` is the ready phrase "Hold <key> for details":
```
block.base.furnace.tooltip = Smelts ores and cooks food.
    {on_key:!core:sneak}{lang:tooltip.more}{/on_key}{on_key:core:sneak}Burns {item:base:coal} coal, {item:base:oak_planks} planks or other {#ffe070}fuel{/} in the bottom slot.{/on_key}
```

## Other tooltips
- **Settings:** `config.<mod>.<key>.tooltip` is shown over the setting's row in Mod Settings. Settings that the
  server overrides or that need a restart get a note added by the engine.
- **Creative tabs:** show their name from `tab.<mod>.<name>`, or the mod's display name when there is no key.
- **Your own screens:** not yet. The engine can show a tooltip over any element of a mod's screen, but
  the SDK's `Ui` builder (`ui.button`, `ui.text`...) has no way to set it, so a mod can't do it today.

## Adding from outside
Translations are one shared table: every `lang/<code>.lang` of every mod and pack is read into it, so the
key of an item is the same whoever wrote the file.\
A mod can add a tooltip to an item of another mod just by having the key in its own lang file.

### Adding a line
The nice way, keeps the original: put your mod's id at the end of the tooltip key.\
After the item's own tooltip the game adds every `<tooltip key>.<mod id>` it finds, one line group per mod,
in the mods' load order:
```
# mods/mymod/assets/lang/en_us.lang
item.base.coal.tooltip.mymod = Also burns in my {#ffe070}boiler{/}.
```
That way coal shows its own text and your line under it.\
It works for any item or block and doesn't need the item to have a tooltip of its own.
Several mods can do it to the same item, they don't interfere with each other.

### Replacing
Without the id the key is the original's, and your text takes its place:
```
item.base.coal.tooltip = Burns, but only in my {#ffe070}boiler{/}.
```
- if the same key is in several mods, the one whose mod id sorts last alphabetically wins; for your own items there is no
  conflict, for someone else's it's a replacement: the old text is gone, so `{lang:}` can't pull it
  back in (a key that points to itself shows nothing), the new text has to be written in full
- a resource pack changes a tooltip the same way, with the key in the `lang` file of the mod that owns it:
  `<pack>/<mod>/lang/en_us.lang`. That replaces the whole file of the mod in that language, so a pack
  copies the original and edits the lines it wants; added lines (`<tooltip key>.<mod id>`) work in packs too
- other languages work with their own file (`ru_ru.lang`), a missing key falls back to English

## Look
The box is a theme part (`mods/base/assets/ui/theme.theme`):
```
tooltip.texture = base:gui/tooltip
tooltip.frame = base:gui/tooltip_frame
```
`texture` is the background and `frame` the border drawn over it, both are 9-slice GUI textures
(`.gui` sidecars set the slicing), `tooltip.text` is the default text color. A resource pack can replace the
files or the theme. Without a texture the box is a dark purple vanilla-like rectangle.

## Limits
- the text is the same for every stack of an item, so no tooltips like an enchantment or a charge on one
  stack; only conditions (`{if:}`, `{on_key:}`) vary it
- no tooltips changed by code
- text and inline item icons only, the box does not scroll: keep it to a few lines and put the rest behind sneak
