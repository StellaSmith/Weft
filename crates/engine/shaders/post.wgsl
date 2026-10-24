// Screen effects: each active effect is one full screen pass over the previous result.

struct Pass {
    // Bindless index of the image to read, the effect's slot, its strength and the time.
    scene: u32,
    slot: u32,
    strength: f32,
    time: f32,
    size: vec2<f32>,
    _pad: vec2<f32>,
}

// What a screen effect (`shaders/post/<name>.wgsl`, `fn apply(p: PostPixel) -> vec4<f32>`) gets.
struct PostPixel {
    // 0..1 across the screen, top left first.
    uv: vec2<f32>,
    // The image so far at `uv`, linear.
    color: vec4<f32>,
    // 0..1 as the mod set it, for fading in and out.
    strength: f32,
    time: f32,
    // Pixels.
    size: vec2<f32>,
    scene: u32,
}

// The image so far anywhere else (blur, waves).
fn scene_at(p: PostPixel, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(textures[p.scene], linear_clamp, uv, 0.0);
}

@group(1) @binding(0) var<storage, read> passes: array<Pass>;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) pass_index: u32,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VOut;
    out.clip = vec4(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2(p.x, 1.0 - p.y);
    out.pass_index = ii;
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let pass_ = passes[in.pass_index];
    let color = textureSampleLevel(textures[pass_.scene], linear_clamp, in.uv, 0.0);
    let px = PostPixel(in.uv, color, pass_.strength, pass_.time, pass_.size, pass_.scene);
    return vec4(post_apply(pass_.slot, px).rgb, 1.0);
}
