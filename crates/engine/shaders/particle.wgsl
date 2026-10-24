// Particles: camera-facing quads, one instance each, all kinds in one draw per blend mode.

struct View {
    view_proj: mat4x4<f32>,
    // Camera right and up in world space, for the billboards.
    right: vec4<f32>,
    up: vec4<f32>,
    fog_color: vec4<f32>,
    // x fog start, y fog end, z daylight 0..1, w seconds since start
    params: vec4<f32>,
    // x ambient light 0..1, y brightness setting 0..1
    look: vec4<f32>,
}

struct Particle {
    // Centre relative to the camera, and edge length in blocks.
    pos_size: vec4<f32>,
    // x colour 0xRRGGBBAA, y texture array | layer << 8 | shader slot << 24,
    // z uv rectangle as four bytes (u0 v0 u1 v1, /255), w light: sky, red, green, blue bytes
    // (0..15), all ones = unlit
    data: vec4<u32>,
    // x life fraction 0..1
    extra: vec4<f32>,
}

// What a particle shader (`shaders/particle/<name>.wgsl`, `fn shade(p: ParticleFrag) -> vec4<f32>`)
// gets; it returns the colour before lighting and fog.
struct ParticleFrag {
    // 0..1 across the quad.
    uv: vec2<f32>,
    // The texture sample and the kind's colour at this age, both linear.
    albedo: vec4<f32>,
    color: vec4<f32>,
    // 0 at birth, 1 at the end of its life.
    life: f32,
    time: f32,
}

@group(1) @binding(0) var<storage, read> particles: array<Particle>;
@group(1) @binding(1) var<storage, read> view: View;

var<private> CORNER: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
);

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) quad: vec2<f32>,
    @location(2) @interpolate(flat) data: vec4<u32>,
    @location(3) @interpolate(flat) life: f32,
    @location(4) dist: f32,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let p = particles[ii];
    let c = CORNER[vi % 6u];
    let world = p.pos_size.xyz + (view.right.xyz * (c.x - 0.5) + view.up.xyz * (0.5 - c.y)) * p.pos_size.w;
    var out: VOut;
    out.clip = view.view_proj * vec4(world, 1.0);
    let r = vec4<f32>(f32(p.data.z & 255u), f32((p.data.z >> 8u) & 255u), f32((p.data.z >> 16u) & 255u), f32(p.data.z >> 24u)) / 255.0;
    out.uv = mix(r.xy, r.zw, c);
    out.quad = c;
    out.data = p.data;
    out.life = p.extra.x;
    out.dist = length(world);
    return out;
}

fn unpack_color(c: u32) -> vec4<f32> {
    let v = vec4<f32>(f32((c >> 24u) & 255u), f32((c >> 16u) & 255u), f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
    return vec4(pow(v.rgb, vec3(2.2)), v.a);
}

fn brightness_curve(level: f32) -> f32 {
    return level / (4.0 - 3.0 * level);
}

// The world shader's lightmap (vanilla 26.1's), for one sky + block light value.
fn lightmap(light: vec4<f32>) -> vec3<f32> {
    let day = clamp((view.params.z - 0.12) / 0.88, 0.0, 1.0);
    let sky = brightness_curve(light.x) * mix(0.24, 1.0, day);
    var c = max(vec3(0.04), vec3(brightness_curve(view.look.x))) + mix(vec3(0.478, 0.478, 1.0), vec3(1.0), day) * sky;
    let block = light.yzw;
    let p = (2.0 * block - 1.0) * (2.0 * block - 1.0);
    let tint = mix(vec3(1.0, 0.847, 0.549), vec3(1.0), 0.9 * p);
    c += tint * vec3(brightness_curve(block.x), brightness_curve(block.y), brightness_curve(block.z)) * 1.4;
    c = clamp(c, vec3(0.0), vec3(1.0));
    let m = max(max(c.x, c.y), c.z);
    let bright = c * ((1.0 - pow(1.0 - m, 4.0)) / max(m, 1e-4));
    return mix(c, bright, view.look.y);
}

fn shade(in: VOut) -> vec4<f32> {
    let tex = in.data.y & 255u;
    let layer = (in.data.y >> 8u) & 65535u;
    let albedo = textureSampleLevel(texture_arrays[tex], nearest_clamp, in.uv, layer, 0.0);
    var color = unpack_color(in.data.x);
    color = particle_shade(in.data.y >> 24u, ParticleFrag(in.quad, albedo, color, in.life, view.params.w));
    if in.data.w != 0xffffffffu {
        let l = vec4<f32>(f32(in.data.w & 255u), f32((in.data.w >> 8u) & 255u), f32((in.data.w >> 16u) & 255u), f32(in.data.w >> 24u));
        color = vec4(color.rgb * pow(lightmap(min(l, vec4(15.0)) / 15.0), vec3(2.2)), color.a);
    }
    let fog = clamp((in.dist - view.params.x) / max(view.params.y - view.params.x, 1.0), 0.0, 1.0);
    return vec4(mix(color.rgb, view.fog_color.rgb, fog), color.a);
}

@fragment
fn fs_cutout(in: VOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if c.a < 0.5 {
        discard;
    }
    return vec4(c.rgb, 1.0);
}

@fragment
fn fs_blend(in: VOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if c.a < 0.01 {
        discard;
    }
    return c;
}
