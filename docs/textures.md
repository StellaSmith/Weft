# Textures guide

Everything a texture can do beyond being a picture.\
Made with files next to the texture, works from both mods and resource packs and requires no code.
Connected and varied textures have their own guide: [CTM](ctm.md).

## Note
The extra files are found by the texture's name:\
for `furnace_front_lit.png` they are`furnace_front_lit.anim`, `furnace_front_lit_e.png` etc. in the same folder.\
They come from the same mod as the texture itself, so a pack that replaces the picture should have its own `_e` and `.anim` too.\
Examples to copy from are in `mods/base/assets/textures/block/`:\
`water_still.anim`,`furnace_front_lit_e.png`, `grass_side_overlay.png`.

## Quick links
[animation](#animation) | [emission](#emission) | [overlays](#overlays) | [debugging](#debugging)

## Animation
A texture is animated when its picture is a vertical strip of square frames: a 16x16 texture with
8 frames is a 16x128 image, the frames go from top to bottom. Without a `.anim` file the frames play in order,
one tick each (20 texture frames per second).

`<texture>.anim` is used to set those properties. It is `key = value` lines.

### frametime
How many ticks one frame is shown. Default is `1`.

example: `frametime = 2`

---

### interpolate
Blends smoothly towards the next frame instead of switching. Default `false`.

values: `true`, `false`

---

### frames
The order of the frames, counting from 0. `N*M` shows frame N for M ticks, instead of `frametime`.
Frames that don't exist are skipped. Without this line animation shows all frames in default order.

example: `frames = 0 1 2 3*5 2 1`

### Example
Eight frames, slowly blended, the fourth one stays longer:
```
frametime = 4
interpolate = true
frames = 0 1 2 3*10 4 5 6 7
```

A texture that is not square or not a whole number of frames tall shows as the "missing texture"
pattern, and the log says `not square or a vertical strip`.

## Emission
Parts of a texture can glow, they keep their color and don't get dark at night or in caves.
It's only how the texture looks, it doesn't light up its surroundings (for that use the block's own light
settings).

Put the glowing parts in a second image next to the texture, with the same size and the same number of frames:

### `<texture>_e.png`
Our own format. The red channel multiplied by alpha is how much a pixel glows: black or transparent
is no glow, full red is full glow.\
That's how the lit furnace works: `furnace_front_lit.png` is the front, `furnace_front_lit_e.png`
has the fire pixels white and the rest black, so the fire shines in the dark and the stone doesn't.

### `<texture>_s.png`
The specular map of LabPBR packs, so shader packs' textures work as they are. Only emission is read from it:
uses alpha channel, `0` - `254` is glow strength, `255` means no emission at all
(it's the empty value of the format).

If both files exist, `_e` overrides `_s`.\
Glow works for blocks and for entity skins (`_e` next to the skin png).

## Overlays
If `<texture>.png` has a `<texture>_overlay.png` next to it, the overlay is drawn over the face
and takes the block's tint instead of the face itself.\
There is exactly one overlay per texture and it is always tinted, (there are no `_overlay1` or conditions).\
That's how grass sides are done: the dirt side stays brown, the grass fringe gets the biome's color.\
Different looks for different block states (a lit furnace) are different textures, not overlays.

The overlay has no `_e` of its own, the glow of a face comes from the base texture's `_e`.

## Debugging
A broken picture never stops the game: a texture that is missing or has a wrong size shows the missing
texture pattern, and the log says which file caused it.\
`F3+T` reloads the textures, so a `.anim` or an `_e` file can be changed while the game is running.
