# Resource packs guide

A resource pack changes how the game looks without touching any code, and it can do that for the base game or for any single mod.\
It is just a folder of files, laid out like a mod's assets.

## Note
A pack replaces files by their path, so what goes where is the same as in a mod's `assets` folder, with one extra level in front for the mod's id. Players turn packs on in **Settings > Resource Packs**, which also has an **Open Folder** button.\
This guide is about making a pack. How a mod lays out its own files is in the [cheat sheet](modding.md#names-and-paths).

## Making a pack
A pack is a folder in `resourcepacks/` next to the game, and the folder's name is its id.

```
resourcepacks/
    my_pack/
        pack.cfg
        base/
            textures/block/stone.png
            lang/en_us.lang
        examplemod/
            textures/item/example_item.png
```
Every mod gets its own folder inside the pack, named by the mod's id and laid out exactly like that mod's `assets`. A file at the same path as the mod's own replaces it and everything else stays as the mod made it. A folder for a mod that isn't loaded is simply ignored, so one pack can cover as many mods as you like.

`pack.cfg` is optional and only names the pack in the list, with two `key = value` lines (`name` and `description`). Without it the folder's name is shown.\
While you work, **F3+T** reloads the files, so there's no need to restart.

## What can be replaced
Anything a mod keeps in `assets`, at the same path:

| files | what they change |
|---|---|
| `textures/block/`, `item/`, `entity/` | blocks, items and mob skins. `_e` glow masks and `.anim` files go along, see the [textures guide](textures.md) |
| `textures/gui/` | windows, buttons and bars, with `.gui` files for how they stretch ([UI guide](ui.md#gui-textures)) |
| `textures/entity/armor/`, `textures/particle/` | worn armor and particles |
| `lang/en_us.lang` | all text, [tooltips](tooltips.md) included, and the named colors (`color.button = {#ffffff}`, [UI guide](ui.md#looks)) |
| `models/`, `animations/` | block and item shapes (`models/block/`, `models/item/`, Java models) and mob models and keyframes ([models guide](models.md)); a block's new shape is only drawn, its collision stays |
| `shaders/` (`fx`, `gui`, `entity`, `particle`, `post`) | the shader snippets ([shaders guide](shaders.md)) |
| `sounds/` | sounds and music, `.ogg` or `.wav` |
| `<texture>.ctm` | connected textures ([CTM guide](ctm.md)) |

Two things catch people out. First, files are replaced whole. Translations are the exception: a pack's `base/lang/en_us.lang` goes on top of base's, so it needs only the lines it changes (colors included). Second, the extra files that belong to a texture (`.anim`, `_e`, `_overlay`, `.ctm`) are taken from the same pack or mod as the picture itself. If a pack swaps `glass.png`, it should bring its own `glass.ctm` and tiles too, otherwise the mod's connected tiles end up on the pack's picture.

A pack can't add blocks, items or mobs, since those are code and so need a mod. What it can add is looks for things that already exist: a `.ctm` for a texture that had none (in the folder of the mod that owns it, like `my_pack/base/textures/block/glass.ctm`, which a mod can't do for someone else's texture), or `_e`, `.anim` and `_overlay` files for existing textures.

## Order of packs
Several packs can be on at once, and they stack. The list reads **Selected (top wins)**: where two packs have the same file, the one higher up is used. A pack only needs the files it changes, so a small pack of textures can sit on top of a big one without copying anything.

Packs are folders only, zip files aren't read.

## Shader packs
A shader pack is a different thing: it replaces the engine's own shaders (blocks, mobs, particles, the interface) rather than assets. It lives in `shaderpacks/`, is picked in **Settings > Shader Packs** and only one can be on at a time. Everything about it is in the [shaders guide](shaders.md#shader-packs).
