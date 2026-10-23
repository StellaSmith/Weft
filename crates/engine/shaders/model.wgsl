// Entity models posed on the GPU: one instance per entity and layer (skin or something worn),
// cuboids from a static per-kind table. Part rotations follow the roles in `part_role`.

struct Instance {
    // Feet position relative to the camera, body yaw.
    pos: vec3<f32>,
    yaw: f32,
    pitch: f32,
    swing: f32,
    hurt: f32,
    // Extra model pixels on every side (worn layers).
    inflate: f32,
    color: vec3<f32>,
    tex: u32,
    tex_size: vec2<f32>,
    // Bit per part role this layer draws.
    roles: u32,
    first_cuboid: u32,
}

struct Cuboid {
    pivot: vec3<f32>,
    role: u32,
    origin: vec3<f32>,
    inflate: f32,
    size: vec3<f32>,
    _pad: f32,
    uv: vec2<f32>,
    _pad2: vec2<f32>,
}

struct View {
    view_proj: mat4x4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,
}

@group(1) @binding(0) var<storage, read> instances: array<Instance>;
@group(1) @binding(1) var<storage, read> view: View;
@group(1) @binding(2) var<storage, read> cuboids: array<Cuboid>;

var<private> CORNERS: array<vec3<f32>, 24> = array<vec3<f32>, 24>(
    vec3(0.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 1.0), vec3(0.0, 1.0, 0.0),
    vec3(1.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0), vec3(1.0, 1.0, 0.0), vec3(1.0, 1.0, 1.0),
    vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(1.0, 0.0, 1.0), vec3(0.0, 0.0, 1.0),
    vec3(0.0, 1.0, 1.0), vec3(1.0, 1.0, 1.0), vec3(1.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0),
    vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(1.0, 1.0, 0.0),
    vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 1.0), vec3(1.0, 1.0, 1.0), vec3(0.0, 1.0, 1.0),
);
var<private> UVS: array<vec2<f32>, 4> = array<vec2<f32>, 4>(vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(1.0, 0.0), vec2(0.0, 0.0));
var<private> TRI: array<u32, 6> = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
var<private> SHADE: array<f32, 6> = array<f32, 6>(0.6, 0.6, 0.5, 1.0, 0.8, 0.8);

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) @interpolate(flat) tex: u32,
    @location(3) dist: f32,
}

fn face_rect(face: u32, d: vec3<f32>, o: vec2<f32>) -> vec4<f32> {
    let w = d.x;
    let h = d.y;
    let z = d.z;
    switch face {
        case 0u: { return vec4(o.x, o.y + z, z, h); }
        case 1u: { return vec4(o.x + z + w, o.y + z, z, h); }
        case 2u: { return vec4(o.x + z + w, o.y, w, z); }
        case 3u: { return vec4(o.x + z, o.y, w, z); }
        case 4u: { return vec4(o.x + z, o.y + z, w, h); }
        default: { return vec4(o.x + z * 2.0 + w, o.y + z, w, h); }
    }
}

fn rot_x(a: f32, v: vec3<f32>) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3(v.x, v.y * c - v.z * s, v.y * s + v.z * c);
}

// Same angles as the CPU path in entity_render.rs.
fn part_angle(role: u32, pitch: f32, swing: f32) -> f32 {
    switch role {
        case 1u: { return pitch; }
        case 2u, 5u, 6u: { return swing; }
        case 3u, 4u, 7u: { return -swing; }
        case 8u: { return -1.5707964 + swing * 0.2; }
        case 9u: { return -1.5707964 - swing * 0.2; }
        default: { return 0.0; }
    }
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let inst = instances[ii];
    let cub = cuboids[inst.first_cuboid + vi / 36u];
    var out: VOut;
    if ((inst.roles >> cub.role) & 1u) == 0u {
        out.clip = vec4(0.0, 0.0, 0.0, 1.0);
        return out;
    }
    let face = (vi % 36u) / 6u;
    let c = TRI[vi % 6u];
    let p = CORNERS[face * 4u + c];
    let grow = cub.inflate + inst.inflate;
    let local = cub.origin - vec3(grow) + p * (cub.size + vec3(2.0 * grow));
    let posed = (cub.pivot + rot_x(part_angle(cub.role, inst.pitch, inst.swing), local)) / 16.0;
    let cy = cos(inst.yaw);
    let sy = sin(inst.yaw);
    let world = inst.pos + vec3(posed.x * cy + posed.z * sy, posed.y, -posed.x * sy + posed.z * cy);
    out.clip = view.view_proj * vec4(world, 1.0);
    out.dist = length(world);
    let r = face_rect(face, cub.size, cub.uv);
    out.uv = (r.xy + UVS[c] * r.zw) / inst.tex_size;
    out.tex = inst.tex;
    out.color = pow(inst.color * SHADE[face] * mix(vec3(1.0), vec3(1.0, 0.4, 0.4), inst.hurt), vec3(2.2));
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(textures[in.tex], nearest_clamp, in.uv);
    if albedo.a < 0.5 {
        discard;
    }
    let f = clamp((in.dist - view.fog.x) / max(view.fog.y - view.fog.x, 1.0), 0.0, 1.0);
    return vec4(mix(albedo.rgb * in.color, view.fog_color.rgb, f), 1.0);
}
