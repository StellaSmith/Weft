# Shaders guide

Small shader files can change how text, panels, mobs, particles and the whole screen look, and shader packs can replace the engine's own shaders altogether.\
Everything is written in WGSL and shipped as assets, so a mod needs no engine changes and next to no code.

## Note
Examples to copy from, all in `mods/testmod/assets/shaders/`:
- `fx/glitch.wgsl`: letters that jump sideways
- `gui/circuit.wgsl`: a panel with pulses running along traces
- `entity/ghost.wgsl`: a mob that breathes and has holes sweeping across it
- `particle/twinkle.wgsl`: sparks that flicker and shift color as they age
- `post/dream.wgsl`: a screen effect that sways and warms the picture

The engine's own text effects are in `crates/engine/assets/shaders/fx/`.

## Quick links
[snippets](#snippets) | [text effects](#text-effects) | [panels](#panels) | [entities](#entities) | [particles](#particles) | [screen effects](#screen-effects) | [shader packs](#shader-packs)

## Snippets
A snippet is a file `shaders/<kind>/<name>.wgsl` in your mod, holding one function whose name depends on the kind. The engine puts all snippets of a kind into a single shader and picks one by number for each text, panel, mob or particle, so a hundred snippets don't cost a hundred shaders.

| kind | folder | function | used by |
|---|---|---|---|
| text effects | `shaders/fx/` | `fn effect(g: Glyph) -> Glyph` | `{fx:mymod:name}` in a translation |
| panels | `shaders/gui/` | `fn fill(p: GuiPixel) -> vec4<f32>` | a theme class: `class.shader = mymod:name` |
| entities | `shaders/entity/` | `fn shade(p: EntityPixel) -> vec4<f32>` | `r.entity(..).shader("name")` |
| particles | `shaders/particle/` | `fn shade(p: ParticleFrag) -> vec4<f32>` | `r.particle(..).shader("name")` |
| screen effects | `shaders/post/` | `fn apply(p: PostPixel) -> vec4<f32>` | `client.post_effect("mymod:name", strength)` |

The file name is the snippet's name, so `shaders/fx/glitch.wgsl` in `testmod` is `testmod:glitch`. Whatever a function can see is in the struct it takes, and all of them carry `time`, the seconds since the game started. A resource pack can replace a snippet the way it replaces a texture, and a mistake in one shows up in the log with the file and the error.

## Text effects
An effect moves and tints each letter of the text between `{fx:mymod:name}` and `{/fx}`:
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
`Glyph` has `offset` (pixels added to the letter's corner), `color` (linear, with alpha), `time`, `index` (the letter's number in its text, handy for waves), `corner` (which corner of the letter, 0 to 1 on each axis), `scale` (physical pixels per GUI pixel) and `shadow`, which is true for the dark copy drawn under the text. Leave the shadow's color alone, or it turns bright.

The function runs for every corner of every letter, so it pays to keep it small. How to use the effects inside translations is in the [tooltips guide](tooltips.md#effects).

## Panels
A theme class can name a shader, and the panel is filled by it instead of a texture:
```
testmod_panel.shader = testmod:circuit
```
```wgsl
fn fill(p: GuiPixel) -> vec4<f32> {
    let edge = min(min(p.uv.x, 1.0 - p.uv.x) * p.size.x, min(p.uv.y, 1.0 - p.uv.y) * p.size.y);
    let border = step(edge, 3.0);
    return vec4(vec3(0.02) + vec3(0.25) * border, 0.97);
}
```
`GuiPixel` has `uv` (0 to 1 across the panel), `size` (in pixels), `color` (the theme's color for it, linear) and `time`. More on themes is in the [UI guide](ui.md#themes).

## Entities
An entity shader colors the pixels of a mob. It takes over from the normal lit skin, so it also decides how light affects the mob:
```wgsl
fn shade(p: EntityPixel) -> vec4<f32> {
    let pulse = 0.75 + 0.25 * sin(p.time * 3.0);
    let lit = max(p.light, vec3(0.35)) * pulse;
    return vec4(p.albedo.rgb * lit, p.albedo.a);
}
```
```rust
r.entity("wisp").texture("wisp", 32, 16).model(..).shader("ghost");
```
`EntityPixel` has `uv`, `albedo` (the skin's texel, linear, `a` is its alpha), `light` (a vec3 with shading, the hurt flash and glow already included) and `time`. A returned alpha under 0.5 cuts the pixel out, which is how the wisp gets its holes. The `_e` glow masks of the [textures guide](textures.md#emission) are a separate and simpler way to make parts of a skin shine.

## Particles
A particle shader colors a particle before lighting and fog are applied:
```wgsl
fn shade(p: ParticleFrag) -> vec4<f32> {
    let flicker = 0.7 + 0.3 * sin(p.time * 30.0 + p.life * 20.0);
    return vec4(p.albedo.rgb * flicker, p.albedo.a * p.color.a);
}
```
```rust
r.particle("spark").textures(&["glint_0", "glint_1"]).life(0.3, 0.6).fullbright().shader("twinkle");
```
`ParticleFrag` has `uv` (0 to 1 across the quad), `albedo` (the texture sample), `color` (the kind's color at this age, between the two you gave to `color(start, end)`), `life` (0 at birth, 1 at the end) and `time`.

The rest of the builder is `life(min, max)` in seconds, `size(min, max)` in blocks, `gravity(g)` (negative rises), `drag(d)`, `collide()`, `fullbright()`, `translucent()` and `shrink()`. From the server, `world.particles(&SPARK, pos).count(12).spread(0.4).speed(2.0)` makes them appear for everyone nearby. Registering a particle named `core:block` or `core:poof` changes how the engine's own block dust and smoke look.

## Screen effects
A screen effect works on the finished picture of the world, and fades in and out with a strength from 0 to 1:
```wgsl
fn apply(p: PostPixel) -> vec4<f32> {
    let wave = vec2(sin(p.uv.y * 18.0 + p.time * 2.0), 0.0) * 0.006 * p.strength;
    let c = scene_at(p, p.uv + wave).rgb;
    return vec4(c * mix(vec3(1.0), vec3(1.15, 1.0, 0.8), p.strength), 1.0);
}
```
```rust
r.key("dream", "KeyN");
r.client_frame(|c| {
    if c.pressed("mymod:dream") { c.post_effect("dream", 1.0); }
});
```
`PostPixel` has `uv` (0 to 1, top left first), `color` (the picture so far at that point, linear), `strength`, `time` and `size` in pixels, and `scene_at(p, uv)` reads the picture anywhere else, for blur and waves. `post_effect(name, 0.0)` turns an effect off, and several can be on at once, applied one after another in the order they were turned on.

## Shader packs
Snippets add to the engine's shaders, a shader pack replaces them. It is a folder `shaderpacks/<id>/` next to the game, chosen by players in the settings, with any of these files:

| file | what it draws |
|---|---|
| `world.wgsl` | blocks and liquids |
| `model.wgsl` | mobs and the player's model |
| `entity.wgsl` | the first-person hand and held items |
| `particle.wgsl` | particles |
| `post.wgsl` | screen effects |
| `ui.wgsl` | all screens and text |

A file replaces its shader completely and the ones a pack doesn't have stay as they were, so the easiest start is a copy of the original from `crates/engine/shaders/`, because a replacement has to keep the entry points and bindings. If it doesn't compile, the log says why and the built-in shader is used instead, so a broken pack can't leave anyone with a black screen. Mods' snippets keep working inside a pack's shaders, and an optional `pack.cfg` (`name = ...`, `description = ...`) names the pack in the list.

A shader pack is also how blocks get drawn differently, since a mod itself gets no shader hooks for them. Textures and models stay with resource packs, see the [resource packs guide](resourcepacks.md).
