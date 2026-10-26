# Models and animations guide

How entities look and move: models written in code or made in Blockbench, keyframe animations, the first-person hand, and armor drawn on the wearer.\
Custom shapes for blocks are a different, simpler thing (`model_box`), they live in the blocks part of the [cheat sheet](modding.md).

## Note
Examples to copy from:
- `mods/testmod/assets/models/idol.geo.json` and `animations/idol.animation.json`: a model with mesh wings, an idle and a bow
- `mods/testmod/assets/animations/player.animation.json`: a wave for the player, first-person hand included
- `mods/testmod/src/looks.rs`: entities with models from code (`wisp`, `orb`) and from a file (`idol`)
- `mods/base/src/mobs.rs`: the `biped`, `quadruped` and `spider` mobs

## Quick links
[the basics](#the-basics) | [model in code](#model-in-code) | [model from Blockbench](#model-from-blockbench) | [roles](#roles) | [animations](#animations) | [first person hand](#first-person-hand) | [armor](#armor-and-other-worn-items)

## The basics
An entity has a size, which is its hitbox, and a model, which is what gets drawn. The two are independent: `.size(0.6, 1.8)` is in blocks, and the model can be bigger or smaller than the box it lives in.

A model is a list of bones. Each one has a pivot (the point it turns around), boxes or loose triangles drawn around that point, and optionally a parent bone it moves with. Everything is measured in model pixels, 16 to a block, with y up from the feet and the model looking along -Z (north).

The skin lives in `textures/entity/<name>.png`:
```rust
static GHOST: Entity = Entity::named("ghost");
r.entity(&GHOST).size(0.6, 1.8).health(10.0).texture("ghost", 64, 32).model(biped(false));
```
`texture` takes the file (`entity/ghost.png` of your mod, or a full name for someone else's) and the skin's size in pixels, which the boxes need to work out their UV unwrap.

## Model in code
`.model(parts)` takes a `Vec<Part>`. Three are ready made: `biped(arms_forward)` is the classic humanoid on a 64x32 skin (`true` holds the arms out like a zombie), `quadruped(body, leg, head)` is an animal on a 64x64 skin with sizes in model pixels, and `spider()` is head, two body parts and eight legs.

For your own, a `Part` starts from a pivot and a [role](#roles) and collects boxes with `cube`:
```rust
Part::new([0.0, 3.0, 0.0], part_role::STATIC)
    .cube([-3.0, -3.0, -3.0], [6.0, 6.0, 6.0], [0.0, 0.0])
```
Everything you can do with a part:
- `cube(origin, size, uv)`: a box. `origin` is relative to the pivot, `uv` the top-left corner of its unwrap on the skin
- `mirrored()`: mirrors the last box's unwrap, handy for an arm that shares its texture with the other one
- `named("ring")`: the name animations use for this bone
- `child_of(index)`: hangs it from the part at that index in the same list
- `rotation(x, y, z)`: a resting rotation in radians
- `mesh(triangles)`: loose triangles in model pixels, three corners each. `sphere(center, radius, rings, segments, uv)` builds a ball

This is the orb from testmod, a ball with a ring of stones hanging off it:
```rust
let ring = Part::new([0.0, 8.0, 0.0], part_role::STATIC).named("ring").child_of(0);
r.entity(&ORB).texture("orb", 32, 16).model(vec![
    Part::new([0.0, 8.0, 0.0], part_role::STATIC).named("core").mesh(sphere([0.0, 8.0, 0.0], 4.0, 8, 12, [0.0, 0.0, 16.0, 16.0])),
    ring.cube([6.0, -1.0, -1.0], [2.0, 2.0, 2.0], [16.0, 0.0]),
]);
```

## Model from Blockbench
Make the model as a Bedrock Entity, export it as `<name>.geo.json` and drop it into `assets/models/`. A file named like the entity is picked up on its own (`idol.geo.json` for `idol`), and `.model_file("name")` points at another one. Either way the file replaces any model written in code. Animations work the same way, from `animations/<name>.animation.json`.

The engine reads bones with their `pivot`, `parent` and resting `rotation`, boxes with `origin` (absolute, the way Blockbench writes it), `size`, `uv` (a pair or per face), `mirror` and `inflate`, and `poly_mesh` bones, triangles and quads alike, with normalized uvs if you use them. The geometry's `texture_width` and `texture_height` override the numbers you gave to `.texture(..)`, so the skin can be any size.

Two things trip people up. A rotated box is drawn unrotated (the log warns about it), so put it in its own bone and rotate the bone. And list parent bones before their children, as Blockbench does. Molang expressions aren't evaluated, only plain numbers.

## Roles
A role tells the engine what a bone should do on its own, without any keyframes: look where the entity looks, swing while it walks. In code you pass it as `part_role::..`. In a Blockbench file it is worked out from the bone's name (case and `_` don't matter):

| bone name | role |
|---|---|
| `head` | follows where the entity looks |
| `leftArm`, `rightArm` | swing while walking |
| `leftLeg`, `rightLeg` (also `leg0`, `leg1`) | swing while walking |
| `leg2`, `leg3` | the front legs of a four-legged animal, opposite to the back ones |
| anything else | stays still |

Code has two more for special cases: `ARM_FORWARD_LEFT`/`RIGHT` for zombie arms and `LEG_SWEEP_A`/`B` for spider legs that sweep sideways. The player's sneaking pose comes from the bones called `body`, `head` and the arms and legs.

## Animations
One file holds any number of animations, and an animation's name is whatever follows the last dot, so `animation.idol.bow` is just `bow`:
```json
{
  "format_version": "1.8.0",
  "animations": {
    "animation.idol.bow": {
      "loop": "hold_on_last_frame",
      "animation_length": 0.8,
      "bones": {
        "head": { "rotation": { "0": [0, 0, 0], "0.4": [40, 0, 0] } }
      }
    }
  }
}
```
Each bone can carry `rotation` (degrees), `position` (model pixels) and `scale`, as keyframes at times in seconds. A keyframe is a plain `[x, y, z]` or an object with `pre`, `post` and `"lerp_mode": "catmullrom"` when you want a jump or a smooth curve. Between keys the value is linear. `loop` is `true` to repeat, `false` to play once and go back to normal, or `"hold_on_last_frame"` to stay in the last pose.

Keyframes are added on top of a bone's role and resting angle, so even a walking leg can be animated. The angles follow Blockbench's convention: a Bedrock arm turned -90 degrees on x points forward.

### When they play
`idle` plays all the time and `walk` while the entity moves, with nothing for you to do but name them that way. Any other animation is started from code and everyone who sees the entity sees it:
```rust
world.play_animation(entity_id, "bow", false);   // false plays once, true repeats
world.play_animation(entity_id, "", false);      // back to idle and walk
```
The file is found by the entity's name, or by `.animations("name")` when it is called something else.

### Animations for something that already exists
A file can also be attached to an entity that belongs to someone else, the player for example:
```rust
r.set_text_on(Target::Entity("core:player"), &ANIMATIONS, "mymod:player");
```
After that the player has the animations of your `animations/player.animation.json`, and `play_animation(player, "wave", false)` plays one.

## First person hand
In first person all you see is the hand, and it can be animated as well. A bone called `hand` in the animation moves and turns it. No model has such a bone, only the animation names it. The wave in `player.animation.json` raises the arm and moves `hand` along, so the hand you see follows suit.

## Armor and other worn items
An item shows up on the wearer when it sits in an equipment slot. The slots are declared once, with the bones what is worn there covers and the silhouette of the empty slot, and an item only names its slot:
```rust
static HEAD: EquipmentSlot = EquipmentSlot::named("base:head");    // the base game's, or your own
r.equipment_slot(&BACK, worn_parts::BODY, "slot_back");             // a slot of your mod
r.item(&RUBY_HELMET).wearable(&HEAD, 1.0, "armor/ruby_1")
```
`worn_parts` are `HEAD`, `BODY`, `ARMS`, `LEGS`, combined with `|`. `wearable` takes how far the piece sticks out from the body in pixels (vanilla armor uses 1.0, and 0.5 for the leggings layer) and its texture.
The slot then takes the item in the inventory, shift-clicks put it there, and a new slot of any mod shows up in the inventory screen by itself (`.equipment()` on the menu, `ui.equipment()` on its screen). That texture is drawn on the same boxes as the wearer's, so it follows the wearer's skin layout, and the 64x32 files in `mods/base/assets/textures/entity/armor/` show how (the name is under `entity/`). It works on any model with the right roles, mobs made with `biped` included.

Items in the world and in the hand are flat sprites with some thickness, or the boxes of a block, so there are no custom item models. `F3+T` reloads models and animations along with textures while the game runs.
