# CTM guide

CTM is connected and varied block textures like glass that merges with its neighbors, logs that look
like one trunk, stone that doesn't repeat itself, etc.\
Everything is almost identical to OptiFine/MCPatcher CTM, so packs made for
those port with little more than renaming files.

## Note
CTM is created without code and works from both mods and resource packs, a pack's file overrides the mod's one.\
It works on cube faces only, so no CTM on items, entities or block models (`model_box` or a model file).\
Examples to copy from are in `mods/base/assets/textures/block/`: `glass.ctm`, `log_oak.ctm`, `stone.ctm`.

Check [textures guide](textures.md) for more texturing options like overlays, glow and animation.

## Quick links
[method](#method) | [tiles](#tiles) | [connect](#connect) | [faces](#faces) | [inner_seams](#inner_seams) | [blocks](#blocks) | [heights](#heights) | [biomes](#biomes) | [weights](#weights) | [symmetry](#symmetry) | [width and height](#width-and-height)

## Where the files go
`<texture>.ctm` goes next to the texture it changes, the file's name is the texture's name.\
For example for `textures/block/glass.png` it's `textures/block/glass.ctm`:

### Base mod example:
```
assets/textures/block/
    glass.png          <- the plain texture, the one blocks use
    glass.ctm          <- the rules
    glass/0.png        <- the tiles the rules point to
    glass/1.png
    ...
```

The rules apply to every cube face that shows this texture on any block.\
For example if a block uses glass texture as one of the faces, it will get the same CTM as glass block for that face.

A mod can only add `.ctm` files for its own textures.\
A resource pack can add them for any mod's texture, since packs keeps files in a folder per namespace:\
`glass.ctm` for the base game's glass is `<pack>/base/textures/block/glass.ctm`, and its tiles go into `<pack>/base/textures/block/glass/`.

The file uses `key = value` lines.\
Comments are whole lines starting with `#`. A `#` after a value is a part of that value, so don't do it.

A file can hold several rules, each new one starts with a line `[any text]` as a separator, the text inside doesn't matter.\
The first rule whose conditions pass for the face wins, so put specific ones first.

## Structure of .ctm files

### method
What the tiles mean and how many are needed. Default is `ctm`.

values: `ctm`, `compact`, `horizontal`, `vertical`, `h+v`, `v+h`, `top`, `random`, `repeat`, `fixed`\
(also accepted: `glass`, `bookshelf`, `sandstone`, `pattern`, `static`, `horizontal+vertical`, `vertical+horizontal`, `ctm_compact`)

example: `method = compact`

---

### tiles
Which textures the method uses, in order.

A name is relative to the folder of the `.ctm` file (a full `namespace:path` also works).\
`a-b` at the end of a name expands to numbers: `glass/0-4` is `glass/0 glass/1 glass/2 glass/3 glass/4`.\
`<default>` is the plain texture itself.

examples:\
`tiles = glass/0-4`\
`tiles = <default> stone/1-3`\
`tiles = log_oak/0 log_oak/1 log_oak/2 log_oak/3`

If there are fewer tiles than the method needs, the rule is skipped and the log says
`needs N tiles, has M`.

---

### connect
What counts as "a neighbor" for the connecting methods.

values:\
`tile`: the neighbor shows this texture on that face, whatever block it is (default)\
`block`: the neighbor is the same block\
`state`: the neighbor is the same block state

example: `connect = block`

---

### faces
Which faces of a block the rule works on.

values: `all` (default), `top`, `bottom`, `sides`, `north`, `south`, `east`, `west`, any of them separated by spaces

example: `faces = sides`

---

### inner_seams
If the neighbor's face is covered by another opaque block, that neighbor doesn't connect.\
Default `false`, glass uses `true`.

values: `true`, `false`

---

### blocks
Only for these blocks, separated by spaces. Written like blocks are named anywhere else: a full name, a state (`base:furnace[lit=true]`) or a tag (`#logs`, `#ores/*`).
A name nothing has is reported in the log with the file.

example: `blocks = base:stone base:cobblestone #ores/*`

---

### heights
Only inside these world heights (Y of the block).

example: `heights = 0-40 100-`\
(`100-` means from 100 up, `0-40` up to 40)

---

### biomes
Only in these biomes, by name or by tag. A `!` in front turns the list around.

examples:\
`biomes = base:desert base:beach`\
`biomes = #cold`\
`biomes = !base:desert`

---

### weights
For `random`: how often each tile appears, in the order of `tiles`. Missing ones get the average.

example: `weights = 4 2 2 2`

---

### symmetry
For `random`: do faces of one block get the same tile?

values: `none` (default), `opposite` (opposite faces share it), `all` (all six faces share it)

---

### width and height
For `repeat`: the size of the pattern in blocks, tiles go left to right, top to bottom.

example: `width = 2` and `height = 2` with four tiles

## Method

### ctm
Every neighbor of the 8 around the face decides which of 47 tiles is shown.
Requires exactly 47 tiles, the most versatile and the most size-heavy method.

Tile numbers (the same as OptiFine's template), so tile `0` is the file `0.png` and so on.
Each tile shows the face with the edges where it does **not** connect to a neighbor:

![the numbered 47 tiles](img/ctm_full_layout.png)

Tile 0 is the face alone, with no neighbors. Draw each file as the picture shows: a dark
border on the sides that have no neighbor, a dark dot in a corner where both sides connect but
the diagonal one doesn't.

### compact
Same result as `ctm` from only 5 tiles, the other 42 are built when the game loads.
Each face is split into 4 quadrants, and every quadrant takes one of the 5 tiles
depending on its two edges and the diagonal:

| tile | when the quadrant is... |
|---|---|
| 0 | alone (all borders) |
| 1 | connected on all sides (no borders) |
| 2 | vertical lines only |
| 3 | connected left and right only |
| 4 | an inner corner: both sides connect but the diagonal doesn't |

Here is how 4 quadrants of the input become a stitched result, on a 3x3 pattern with a cutout:

![compact tiles, split, rearranged and stitched](img/ctm_compact.png)

One limit: a quadrant is half the tile, so an edge or a corner can't be wider than half of it
(8x8 pixels on a 16x16 texture). Textures with thicker borders need full CTM.
Check `mods/base/assets/textures/block/glass/` for a real example.

### horizontal, vertical
4 tiles. Connects only left-right (or up-down), like bookshelves or logs.

`horizontal`, seen from the front:\
![horizontal tiles](img/ctm_horizontal.png)

`vertical`, seen from the front:\
![vertical tiles, top to bottom](img/ctm_vertical.png)

### h+v, v+h

`h+v`, the extra tiles go down the left column:

![h+v tiles](img/ctm_h_v.png)

`v+h`:

![v+h tiles](img/ctm_v_h.png)

### top
1 tile. Side faces use it when the same block is above them (sandstone-like), top and bottom faces never do.

### random
Picks a tile from the position, so the same block always looks the same.\
Any number of tiles, see `weights` and `symmetry` for more values.

### repeat
A pattern of `width` x `height` tiles that continues across blocks, tiles go row by row.\
Needs `width * height` tiles.

### fixed
always 1 tile. Handy together with `blocks`, `heights` or `biomes` to swap the texture there.

## Examples

### Glass
`glass.ctm`:
```
method = compact
tiles = glass/0-4
inner_seams = true
```
with `glass/0.png` ... `glass/4.png` next to it.

### Logs that look like one trunk
`log_oak.ctm`:
```
method = vertical
tiles = log_oak/0-3
faces = sides
```

### Stone that doesn't repeat
`stone.ctm`:
```
method = random
tiles = <default> stone/1-3
weights = 4 2 2 2
```

### Several rules in one file
Only in deserts, a different one elsewhere:
```
method = fixed
tiles = sand_smooth
biomes = base:desert

[the rest of the world]
method = random
tiles = <default> sand_1 sand_2
```

## Debugging
The log says `connected textures: N rules, M tiles, K generated` when any rule is loaded.\
`unknown method` or `needs N tiles` lines name the file and the rule that was skipped, and a skipped
rule just shows the plain texture.\
A tile file that doesn't exist shows up as the "missing texture" pattern.
