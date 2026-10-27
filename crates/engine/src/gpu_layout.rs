//! Everything the engine's shaders and its Rust code must agree on, written once here: bit
//! layouts and mode numbers (`<GROUP>_<NAME>` constants), the structs of GPU buffers and each
//! shader's bindings. `prelude(shader)` puts them in front of the shader, so an engine shader, a
//! shader pack's replacement and a snippet only write functions.

macro_rules! shared {
    ($($(#[$doc:meta])* $group:ident { $($(#[$cdoc:meta])* $name:ident = $v:expr),* $(,)? })*) => {
        $($(#[$doc])* pub mod $group { $($(#[$cdoc])* pub const $name: u32 = $v;)* })*

        /// The constants as WGSL.
        pub fn wgsl() -> String {
            let mut s = String::new();
            $($(s.push_str(&format!("const {}_{}: u32 = {}u;\n", stringify!($group).to_uppercase(), stringify!($name), $group::$name));)*)*
            s
        }
    };
}

shared! {
    /// Block face record, four words. Word 0: x, y, z in the cube (4 bits each), direction,
    /// corner AO (2 bits per corner), tint kind. Word 1: texture, biome. Words 2 and 3: corner
    /// light, 16 bits per corner.
    face {
        DIR_SHIFT = 12,
        AO_SHIFT = 16,
        TINT_SHIFT = 24,
        TEX_MASK = 0xFFFF,
        BIOME_SHIFT = 16,
    }
    /// Free-form quad record, eight words: four corners (10 bits per axis), texture and biome
    /// (like a face's word 1), uv (5 bits each) and tint kind with `FLAT`, corner light.
    quad {
        TINT_SHIFT = 24,
        /// Tint flag: no directional shading (plants).
        FLAT = 16,
    }
    /// Tint kinds of faces and quads (0 none); mods' kinds follow the engine's.
    tint {
        MASK = 15,
    }
    /// Entity box modes (`entity.wgsl`).
    box_mode {
        SKIN = 0,
        FACES = 1,
        SOLID = 2,
        FLAMES = 3,
        TEXEL = 4,
        FILLED = 5,
    }
    /// UI quad modes (`ui.wgsl`).
    ui_mode {
        SOLID = 0,
        TEXTURE = 1,
        ICON = 2,
        FONT = 3,
        GUI = 4,
    }
    /// Particle data word y: texture index and shader slot.
    particle {
        TEX_MASK = 0xFFFF,
        SLOT_SHIFT = 24,
    }
}

/// A host-visible buffer per frame in flight (`slots`), holding at least `needed` elements of
/// `stride` bytes; a slot too small is replaced by one grown to the next power of two.
pub fn frame_buffer(gpu: &mut dyn weft_render::Backend, slots: &mut Vec<(weft_render::BufferId, u32)>, slot: usize, needed: u32, stride: u64, usage: u32, label: &str) -> weft_render::BufferId {
    use weft_render::usage::HOST;
    while slots.len() < gpu.frames_in_flight() {
        slots.push((gpu.create_buffer(stride * 1024, usage | HOST, label), 1024));
    }
    if slots[slot].1 < needed {
        gpu.destroy_buffer(slots[slot].0);
        let cap = needed.next_power_of_two();
        slots[slot] = (gpu.create_buffer(stride * cap as u64, usage | HOST, label), cap);
    }
    slots[slot].0
}

/// WGSL spelling of a field type.
pub trait Wgsl {
    const TY: &'static str;
}

macro_rules! wgsl_types {
    ($($t:ty => $s:literal),* $(,)?) => { $(impl Wgsl for $t { const TY: &'static str = $s; })* };
}
wgsl_types! {
    f32 => "f32", u32 => "u32", i32 => "i32",
    [f32; 2] => "vec2<f32>", [f32; 3] => "vec3<f32>", [f32; 4] => "vec4<f32>",
    [u32; 4] => "vec4<u32>", [i32; 4] => "vec4<i32>",
    [[f32; 4]; 3] => "array<vec4<f32>, 3>", [[f32; 4]; 4] => "mat4x4<f32>", [[f32; 4]; 6] => "array<vec4<f32>, 6>",
}

/// Declares a GPU buffer struct: the Rust type and, from the same fields, its WGSL text
/// (`wgsl()`), under the WGSL name given after `as`.
macro_rules! gpu_struct {
    ($(#[$m:meta])* pub struct $name:ident as $wgsl:literal { $($(#[$fm:meta])* pub $f:ident: $t:ty),* $(,)? }) => {
        $(#[$m])*
        #[repr(C)]
        #[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
        pub struct $name { $($(#[$fm])* pub $f: $t),* }

        impl $name {
            pub fn wgsl() -> String {
                let mut s = format!("struct {} {{\n", $wgsl);
                $(s.push_str(&format!("    {}: {},\n", stringify!($f), <$t as Wgsl>::TY));)*
                s.push_str("}\n");
                s
            }
        }
    };
}

gpu_struct! {
    /// What every world pass sees of the frame: camera, fog, light, time. One buffer a frame
    /// for the world, entities, models and particles; the hand and the inventory figure have
    /// their own camera.
    pub struct Scene as "Scene" {
        pub view_proj: [[f32; 4]; 4],
        /// Camera right and up in world space (particle billboards).
        pub right: [f32; 4],
        pub up: [f32; 4],
        pub fog_color: [f32; 4],
        /// x fog start, y fog end, z daylight 0..1, w seconds (shader effects).
        pub params: [f32; 4],
        /// x the dimension's ambient light 0..1, y the Brightness setting 0 (moody) .. 1
        /// (bright), z game ticks with the fraction of the current one (animated textures;
        /// wraps every 2^20).
        pub light: [f32; 4],
    }
}

gpu_struct! {
    /// What only the world shader needs.
    pub struct Globals as "Globals" {
        /// Camera position inside its cube (0..16) in xyz.
        pub cam_frac: [f32; 4],
        /// z biome count, w blended color tiles.
        pub ids: [u32; 4],
        /// Origins are relative to this cube.
        pub cam_cube: [i32; 4],
        /// x start of the overlay table in `frames` (per texture: overlay texture + 1, or 0),
        /// z tint kinds.
        pub extra: [u32; 4],
    }
}

gpu_struct! {
    /// A box of an entity or a held item (`entity.wgsl`): its unit cube's placement and how it
    /// is textured (`box_mode`).
    pub struct GpuBox as "Box" {
        /// Unit cube -> camera-relative world, rows of a 3x4 matrix.
        pub rows: [[f32; 4]; 3],
        /// Box size in texels (w, h, d) for the classic unwrap; block faces: min corner in the
        /// block, w = 1 when set.
        pub dims: [f32; 4],
        /// Texel origin and texture size.
        pub uvo: [f32; 4],
        /// Tint (rgb) and hurt flash (a).
        pub color: [f32; 4],
        /// Skin texture, `box_mode`, face textures packed 16 bit.
        pub ids: [u32; 4],
        /// x last face textures, y light as the world stores it (all ones: full light); block
        /// faces: z the faces tinted by w (0xRRGGBB).
        pub ids2: [u32; 4],
    }
}

gpu_struct! {
    /// One model drawn (`model.wgsl`).
    pub struct Instance as "Instance" {
        /// First bone matrix, first box and box count, first mesh corner of its kind (filled in
        /// when queued).
        pub bones: u32,
        pub cuboids: u32,
        pub cuboid_count: u32,
        pub vertices: u32,
        /// Tint (rgb) and hurt flash.
        pub color: [f32; 3],
        pub hurt: f32,
        pub tex: u32,
        /// Extra model pixels on every side (worn layers).
        pub inflate: f32,
        pub tex_size: [f32; 2],
        /// Bit per part role this layer draws.
        pub roles: u32,
        /// Entity shader slot (`shaders/entity/<name>.wgsl`, 0 none) and glow mask texture (all
        /// ones: none).
        pub shader: u32,
        pub glow: u32,
        /// Light as the world stores it, all ones: full light.
        pub light: u32,
    }
}

gpu_struct! {
    pub struct GpuCuboid as "Cuboid" {
        /// Corner in model pixels and the bone moving it.
        pub origin: [f32; 3],
        pub bone: u32,
        pub size: [f32; 3],
        pub inflate: f32,
        pub faces: [[f32; 4]; 6],
        pub role: u32,
        // Three words, not a vec3: WGSL would align that to 16 bytes.
        pub pad0: u32,
        pub pad1: u32,
        pub pad2: u32,
    }
}

gpu_struct! {
    pub struct GpuCorner as "Corner" {
        pub pos: [f32; 3],
        pub bone: u32,
        pub uv: [f32; 2],
        pub role: u32,
        pub pad: u32,
    }
}

gpu_struct! {
    /// A bone's transform, three rows.
    pub struct GpuBone as "Bone" {
        pub rows: [[f32; 4]; 3],
    }
}

gpu_struct! {
    pub struct GpuParticle as "Particle" {
        /// Center relative to the camera, and edge length in blocks.
        pub pos_size: [f32; 4],
        /// x color 0xRRGGBBAA, y texture (16 bits) | shader slot << 24, z uv rectangle as four
        /// bytes (u0 v0 u1 v1, /255), w light: sky, red, green, blue bytes (0..15), all ones =
        /// unlit.
        pub data: [u32; 4],
        /// x life fraction 0..1.
        pub extra: [f32; 4],
    }
}

gpu_struct! {
    /// A UI quad (`ui.wgsl`).
    pub struct Quad as "Quad" {
        /// Corner, and the two edge vectors, in physical pixels.
        pub p0: [f32; 2],
        pub u: [f32; 2],
        pub v: [f32; 2],
        /// Text effect slot (top bit set on shadows) or GUI fill slot.
        pub fx: u32,
        /// Glyph number in its text.
        pub index: f32,
        pub uv: [f32; 4],
        pub color: u32,
        pub tex: u32,
        /// A texture animated as a strip of square frames: frame count | ticks per frame << 16
        /// (0: still).
        pub layer: u32,
        pub mode: u32,
        /// Visible rectangle in physical pixels; empty (all zero) means unclipped.
        pub clip: [f32; 4],
    }
}

gpu_struct! {
    pub struct Screen as "Screen" {
        /// Pixels, seconds since start, GUI scale.
        pub size: [f32; 4],
    }
}

gpu_struct! {
    /// One screen effect pass (`post.wgsl`).
    pub struct Pass as "Pass" {
        /// Bindless index of the image to read, the effect's slot, its strength and the time.
        pub scene: u32,
        pub slot: u32,
        pub strength: f32,
        pub time: f32,
        pub size: [f32; 2],
        pub pad: [f32; 2],
    }
}

/// Group 1 of each engine shader: (name, WGSL type) per binding, in order.
fn bindings(shader: &str) -> &'static [(&'static str, &'static str)] {
    match shader {
        "world.wgsl" => &[
            ("g", "Globals"),
            ("origins", "array<vec4<i32>>"),
            ("records", "array<u32>"),
            // Per biome a color per tint kind (`g.extra.z` kinds), 0xRRGGBB. Then the blended
            // colors of loaded block columns, 0xFFRRGGBB, in 16x16 tiles addressed by column
            // position modulo g.ids.w.
            ("biomes", "array<u32>"),
            // Two words per texture, see `GameTextures::world_tables`.
            ("tex_info", "array<u32>"),
            // Animated frames: texture | ticks << 16; then the overlay table.
            ("frames", "array<u32>"),
            // Camera-relative positions of moving structures drawn this frame (0: none, the
            // world's cubes): a cube's `origin.w` picks one, its `origin.xyz` is then the cube in
            // the structure.
            ("offsets", "array<vec4<f32>>"),
            // Glow mask texture per bindless texture index, all ones: none.
            ("glow_map", "array<u32>"),
            // Block shader slot (`shaders/block/<name>.wgsl`) per texture id, 0 none.
            ("tex_shader", "array<u32>"),
            ("scene", "Scene"),
        ],
        // `item_boxes`: item models in their own space, kept between frames
        // (`EntityRenderer::item_model`); an item instance is a box in `boxes` whose rows place
        // the model, `ids.z` its first box, `color.a` the hurt flash and `ids2.y` the light.
        "entity.wgsl" => &[("boxes", "array<Box>"), ("scene", "Scene"), ("glow_map", "array<u32>"), ("item_boxes", "array<Box>")],
        "model.wgsl" => &[("instances", "array<Instance>"), ("scene", "Scene"), ("cuboids", "array<Cuboid>"), ("corners", "array<Corner>"), ("bones", "array<Bone>")],
        "particle.wgsl" => &[("particles", "array<Particle>"), ("scene", "Scene")],
        "ui.wgsl" => &[("quads", "array<Quad>"), ("screen", "Screen")],
        "post.wgsl" => &[("passes", "array<Pass>")],
        _ => &[],
    }
}

/// Every GPU struct as WGSL.
fn structs() -> String {
    [Scene::wgsl(), Globals::wgsl(), GpuBox::wgsl(), Instance::wgsl(), GpuCuboid::wgsl(), GpuCorner::wgsl(), GpuBone::wgsl(), GpuParticle::wgsl(), Quad::wgsl(), Screen::wgsl(), Pass::wgsl()].concat()
}

/// What goes in front of an engine shader (or a pack's replacement of it): the constants, every
/// GPU struct, the shader's bindings, then `common` (lighting, fog, samplers and the structs
/// snippets get).
pub fn prelude(shader: &str, common: &str) -> String {
    let mut s = wgsl();
    s.push_str(&structs());
    for (i, (name, ty)) in bindings(shader).iter().enumerate() {
        s.push_str(&format!("@group(1) @binding({i}) var<storage, read> {name}: {ty};\n"));
    }
    s.push_str(common);
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// naga's layout of every generated struct matches the Rust one, so a field the WGSL rules
    /// align differently (a lone vec3) can't slip in.
    #[test]
    fn layouts_match_rust() {
        let src = format!("{}{}", wgsl(), structs());
        let module = naga::front::wgsl::parse_str(&src).expect("structs parse");
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        let sizes = [
            ("Scene", size_of::<Scene>()),
            ("Globals", size_of::<Globals>()),
            ("Box", size_of::<GpuBox>()),
            ("Instance", size_of::<Instance>()),
            ("Cuboid", size_of::<GpuCuboid>()),
            ("Corner", size_of::<GpuCorner>()),
            ("Bone", size_of::<GpuBone>()),
            ("Particle", size_of::<GpuParticle>()),
            ("Quad", size_of::<Quad>()),
            ("Screen", size_of::<Screen>()),
            ("Pass", size_of::<Pass>()),
        ];
        for (name, rust) in sizes {
            let (h, _) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some(name)).expect(name);
            assert_eq!(layouter[h].size as usize, rust, "{name}");
        }
    }
}
