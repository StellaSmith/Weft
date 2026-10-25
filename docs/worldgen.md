# Worldgen guide

This covers everything that shapes the world: ores, structures, spawns, biomes, and whole dimensions with their own terrain.\
Base builds its overworld with the functions described here.

## Note
Examples to copy from:
- `mods/testmod/src/lib.rs`: Mars, a planet made only from terrain presets, and `void`, an island written by hand
- `mods/testmod/src/looks.rs`: an obelisk structure standing in another mod's desert
- `mods/base/src/worldgen.rs`: the overworld, with biomes, caves, trees and flowers

## Quick links
[how generation works](#how-generation-works) | [ores](#ores) | [structures](#structures) | [spawns](#spawns) | [biomes](#biomes) | [dimensions](#dimensions) | [terrain presets](#terrain-presets) | [writing a generator](#writing-a-generator) | [what a mod can't change](#what-a-mod-cant-change)

## How generation works
The world is made of 16x16x16 cubes, and each one is generated on its own, whenever somebody needs it. That's why a generator has to be a **pure function**: the same seed and position must always give the same blocks, on every machine and in any order. Don't use random numbers, clock, something remembered between cubes for worldgen.

Ores, structures and spawns are applied by the engine after your dimension's generator has run. They don't need to know about each other, which is also what lets a mod add them to a dimension that belongs to someone else.

Cube coordinates are block coordinates divided by 16, so cube `y = 4` holds the blocks from 64 to 79.

## Ores
`r.ore(dimension, block)` adds veins of a block to any dimension, yours or another mod's. For a dimension of another mod you need the full name (`base:overworld`), your own is just `mars`.

```rust
r.ore("base:overworld", "tin_ore").y(-64, 48).veins(4).size(7).replace("#stone_ore_replaceables");
```
- `y(min, max)`: the heights veins can start at
- `veins(n)`: how many veins are started per cube
- `size(n)`: blocks in a vein
- `replace(..)`: what the ore may replace, block names or `#tags` separated by spaces
- `biome("cave")`: only in biomes with this tag, underground ones included

## Structures
A structure is a fixed shape of blocks the engine places on the surface now and then. You draw it in layers from the bottom up: each layer is a list of rows along z, and each character in a row is one block along x. A key says which block a character means, a space leaves whatever is already there, and `air` clears it.

```rust
r.structure("base:overworld", "obelisk")
    .layer(&["sss", "sss", "sss"])
    .layer(&["   ", " g ", "   "])
    .key('s', "base:sandstone").key('g', "glow_crystal")
    .spacing(128).chance(0.5).biome("dry").sink(1);
```
That's a sandstone obelisk with a glowing crystal inside, in every dry biome (so deserts) of base's overworld.

Where and how often it appears is set by four calls. `spacing(n)` gives one chance per `n` x `n` blocks and `chance(c)` says how likely that chance is to be taken, while `biome(tag)` limits it to biomes with that tag and `sink(n)` pushes the bottom layer that many blocks into the ground. A key can also name a block state, like `furnace#2`.

Structures stand on the surface that the dimension reports through `.column(..)` (see below), so a dimension without it gets none. To find one later, `world.structure_near("mymod:obelisk", pos, radius)` gives the closest, even in cubes that don't exist yet.

## Spawns
`r.spawn(entity)` decides where a mob shows up by itself:
- `biome(tag)` and `ground(tag)`: the mob needs a biome with that tag, and a block with that tag under it
- `dimension(name)`: only in that dimension (full name)
- `light(min, max)` and `y(min, max)`: the light level (0 to 15) and heights it needs
- `group(min, max)`: how many appear together
- `weight(n)` and `chance(c)`: how likely it is picked among the rules that fit, and how likely it is to happen per try

```rust
r.spawn("ghost").light(0, 4).weight(1);
r.spawn("sheep").biome("animals").ground("animal_spawn").light(7, 15).group(2, 3).chance(0.05);
```

## Biomes
A biome is a name, three colors and a climate, plus whatever data and tags you attach.

```rust
static DUST: Biome = Biome::new();
r.biome("dust").handle(&DUST).colors(0xb5764a, 0x9a6040, 0x8a5a40).climate(1.5, 0.0);
```
`colors(grass, foliage, water)` is what tinted blocks take in this biome. Tags (`tag("animals")`) are how everything else asks what a biome is, since spawns, ores and structures look at tags and never at names. `set(&COMPONENT, value)` attaches data, like how many trees base grows there, and `handle` gives you the static for the generator.

Registering a biome doesn't put it anywhere. The generator of a dimension decides where it appears, and that can be underground too: base's `deep_caves` is a biome of whole cubes deep below the surface.

## Dimensions
`r.dimension("mars")` makes a separate world with its own terrain, sky and gravity, and players get there with `/dim mymod:mars`. The two things it can't go without are a generator and a spawn point.

```rust
r.dimension("mars").handle(&MARS)
    .generator(|g| { MARS_TERRAIN.generate(g); })
    .spawn(|seed| [8, MARS_TERRAIN.column(seed, 8, 8).height + 1, 8])
    .height(-4, 12)
    .sky_above(11)
    .gravity(12.0)
    .fixed_time(5000)
    .sky(0xd8a080, 0xe0b090);
```

All the settings:
- `generator(f)`: fills one cube, see [writing a generator](#writing-a-generator)
- `spawn(f)`: where a new player starts, from the seed
- `player_spawn()`: new players start here and respawn here (without any dimension doing it, the first registered is used)
- `column(f)`: surface height and biome of a column without generating it. Structures stand on this, so it has to agree with your generator
- `height(min_cube, max_cube)`: the limits of the world, in cubes and inclusive
- `sky_above(cube_y)`: cubes at this height and above are empty, so their sky light is free
- `no_sky_light()`: no sky light at all, for caves and the like
- `gravity(g)`: blocks per second squared, 32 by default
- `day_length(ticks)` or `fixed_time(time)`: either a day of that length, or a time of day that never changes
- `sky(sky, fog)`: the two colors
- `ambient(level)`: the lowest light any block gets, 0 to 15
- `no_block_updates()`: nothing grows, falls or flows here

## Terrain presets
Most dimensions need real terrain, and writing noise by hand is the slow way. The `terrain` module builds it from parts: a base height, features that add to it, biome rules that choose a biome and its surface, then water and caves.

```rust
static MARS_TERRAIN: LazyLock<Terrain> = LazyLock::new(|| {
    Terrain::new(&BASALT, 60.0)
        .feature(preset::hills(8.0, 220.0))
        .feature(preset::craters(32.0, 16.0, 150.0))
        .feature(preset::dunes(3.0, 22.0).mask(Mask::noise(500.0, 0.15, 0.45)))
        .biome(BiomeRule::new(&DUST).surface(&REGOLITH, &REGOLITH, 3))
});
```
`Terrain::new(stone, base_height)` is the stone block and the ground height when nothing is added. In the generator, `.generate(g)` fills a whole cube, and `.column(seed, x, z)` gives the height and biome of a single column.

### Features
A feature adds height to every column. These come ready:

| preset | what it makes |
|---|---|
| `continents(depth)` | land and oceans, `depth` blocks above or below the base |
| `hills(height, size)` | rolling hills |
| `mountains(height, width)` | ranges along ridge lines `width` blocks apart |
| `plateaus(height, size, step)` | flat mesas in terraces |
| `canyons(depth, width)` | winding cuts, filled with water below sea level |
| `dunes(height, wavelength)` | parallel sand waves |
| `craters(radius, depth, spacing)` | bowls with a raised rim |

Your own is `Feature::new(|fields, x, z| height)`. `fields` hands out noise (`fbm`, `ridge` for river-like lines) and the climate (`continent`, `temperature`, `humidity`). The easiest start is copying a preset from `crates/sdk/src/terrain.rs` and changing it.

### Masks
`.mask(..)` limits a feature to some places, with a strength from 0 to 1 that fades in between. `Mask::noise(scale, from, to)` makes patches about `scale` blocks across, and `Mask::Temperature`, `Humidity` and `Continent` take a `(from, to)` range of the climate (swap the two to turn the ramp around). Masks combine with `.and(..)` and flip with `.not()`.

```rust
preset::mountains(150.0, 260.0).mask(Mask::noise(700.0, -0.1, 0.3).salt(0x44).and(Mask::Continent(0.0, 0.2))).named("mountains")
```
`salt` gives a noise its own pattern, so two masks of the same size don't line up, and `named` lets a biome rule ask how strong the feature is.

### Biome rules
Rules are tried in order and the **first one that fits wins**, so specific ones go first and a plain catch-all goes last. A rule starts as `BiomeRule::new(&BIOME)`, and every condition after it is optional:
- `temperature(min, max)`, `humidity(min, max)`, `continent(min, max)`: climate ranges, the fields run about -1.4 to 1.4
- `height(min, max)`: the surface height
- `feature("mountains", 0.45, MAX)`: where the named feature is strong

Then the surface it makes:
- `surface(top, filler, depth)`: a top block, `depth` blocks of filler, then the terrain's stone
- `under(block, layers)`: the lowest layers of filler become another block (sandstone under sand)
- `deep_top(block, below)`: a different top block where the ground is lower than `below` (gravel on a sea floor)
- `bare_above(y)`: plain stone above this height
- `frozen(block)`: the water surface turns into this block

### Water, caves and underground biomes
`.sea(level, &WATER)` fills everything below a level with water. `.caves(Caves::new(&LAVA, from_y, to_y))` carves tunnels and big caverns, and between those two heights some levels hold flat lava lakes. `.cave_biome(&BIOME, depth, size, threshold)` gives whole cubes at least `depth` blocks down a biome of their own, in patches about `size` across.

Trees and plants aren't part of `Terrain`; you place them yourself after `generate`, as shown below.

## Writing a generator
A generator receives a `Gen`, the cube being made. `g.origin()` is the block position of its corner (`g.cube()` is the cube's own), `g.seed()` the world seed, `g.set(x, y, z, state)` and `g.get(x, y, z)` work with blocks (each coordinate 0 to 15, states come from `BLOCK.id()`), and `g.biomes()` is the biome id of each of the 256 columns, indexed `z * 16 + x`.

```rust
fn island(g: &mut Gen) {
    let [ox, oy, oz] = g.origin();
    for y in 0..16usize { for z in 0..16usize { for x in 0..16usize {
        let (wx, wy, wz) = ((ox + x as i32) as f32, oy + y as i32, (oz + z as i32) as f32);
        if (wx * wx + wz * wz).sqrt() < 24.0 && wy <= 63 {
            g.set(x, y, z, STONE.id());
        }
    }}}
}
```
That's `void` from testmod, a round island in an empty dimension.

Two rules to keep. A cube only writes its own blocks, so for something bigger than a cube (a tree) every cube asks what could reach it and places its own part. Base splits the world into 5x5 cells that each decide from the seed whether they hold a tree, and a cube checks the cells around it (`tree_in_cell` in `mods/base/src/worldgen.rs`). And stay away from `sin`, `cos`, `exp` and the like in terrain, since they give slightly different results on different platforms and neighboring cubes would stop matching. `sqrt` and plain arithmetic are safe, and `noise` (`hash`, `unit`, `perlin2`, `perlin3`, `fbm2`) was written for this.

### Decorations
`Terrain::generate` returns the surface `height` and `biome` of every column of the cube, which is what you place things on. Base does grass, flowers and cacti like this:

```rust
let Some(cols) = TERRAIN.generate(g) else { return };
for z in 0..16usize { for x in 0..16usize {
    let col = &cols[z * 16 + x];
    let y = col.height + 1 - oy;           // the block above the ground, inside this cube
    if (0..16).contains(&y) && g.get(x, y as usize, z) == 0 {
        g.set(x, y as usize, z, TALL_GRASS.id());
    }
}}
```
How much grows where is data on the biome (the components `trees`, `grass`, `tree_kinds` and `flowers`), which is why another mod can give a biome its own trees without touching code, by attaching those components with `r.set_on` (see [what a mod can't change](#what-a-mod-cant-change)).

## What a mod can't change
The rules that pick biomes are code inside the mod that owns the dimension, so a mod can't put a new biome into base's overworld. To someone else's dimension it can still add ores, structures and spawns, attach data to existing biomes (`r.set_on(Target::Biome("base:forest"), &COMPONENT, value)`) and tag them, so that its own ores and structures can ask for the tag.

Anything beyond that is a dimension of your own with your own terrain, which is exactly what `mars` and `void` are.
