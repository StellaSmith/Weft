struct Globals {
    view_proj: mat4x4<f32>,
    // Camera position inside its cube (0..16) in xyz.
    cam_frac: vec4<f32>,
    sky_color: vec4<f32>,
    fog_color: vec4<f32>,
    // x daylight 0..1, y fog start, z fog end, w time in seconds
    params: vec4<f32>,
    // x block texture array, y ambient light 0..15
    ids: vec4<u32>,
}

@group(1) @binding(0) var<storage, read> g: Globals;
@group(1) @binding(1) var<storage, read> origins: array<vec4<i32>>;
@group(1) @binding(2) var<storage, read> records: array<u32>;
// Per biome: grass, foliage, water as 0xRRGGBB.
@group(1) @binding(3) var<storage, read> biomes: array<vec4<u32>>;
// Two words per texture name, see world_render.rs.
@group(1) @binding(4) var<storage, read> tex_info: array<u32>;
// Animated frames: layer | ticks << 16.
@group(1) @binding(5) var<storage, read> frames: array<u32>;

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
var<private> TRI_FLIP: array<u32, 6> = array<u32, 6>(1u, 2u, 3u, 1u, 3u, 0u);
var<private> SHADE: array<f32, 6> = array<f32, 6>(0.6, 0.6, 0.5, 1.0, 0.8, 0.8);
var<private> AO: array<f32, 4> = array<f32, 4>(0.42, 0.62, 0.8, 1.0);

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) color: vec3<f32>,
    @location(3) light: vec4<f32>,
    @location(4) dist: f32,
    @location(5) @interpolate(flat) layer2: u32,
    @location(6) @interpolate(flat) blend: f32,
}

fn unpack_rgb(c: u32) -> vec3<f32> {
    let v = vec3<f32>(f32((c >> 16u) & 255u), f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
    return pow(v, vec3(2.2));
}

fn tint_color(word: u32) -> vec3<f32> {
    let tint = (word >> 24u) & 3u;
    if tint == 0u {
        return vec3(1.0);
    }
    let b = biomes[(word >> 16u) & 255u];
    if tint == 1u {
        return unpack_rgb(b.x);
    } else if tint == 2u {
        return unpack_rgb(b.y);
    }
    return unpack_rgb(b.z);
}

struct Frame {
    a: u32,
    b: u32,
    blend: f32,
}

fn texture_frame(name: u32) -> Frame {
    let base = tex_info[name * 2u];
    let info = tex_info[name * 2u + 1u];
    if info == 0u {
        return Frame(base, base, 0.0);
    }
    let count = info & 65535u;
    let total = f32((info >> 16u) & 32767u);
    let ticks = g.params.w * 20.0;
    let t = ticks - floor(ticks / total) * total;
    var acc = 0.0;
    for (var i = 0u; i < count; i++) {
        let e = frames[base + i];
        let d = f32(e >> 16u);
        if t < acc + d || i + 1u == count {
            let next = frames[base + (i + 1u) % count];
            var f = 0.0;
            if (info >> 31u) == 1u {
                f = (t - acc) / d;
            }
            return Frame(e & 65535u, next & 65535u, f);
        }
        acc += d;
    }
    return Frame(base, base, 0.0);
}

fn unpack_light(l: u32) -> vec4<f32> {
    return vec4<f32>(f32(l & 15u), f32((l >> 4u) & 15u), f32((l >> 8u) & 15u), f32((l >> 12u) & 15u)) / 15.0;
}

fn finish(local: vec3<f32>, origin: vec4<i32>) -> vec4<f32> {
    let pos = vec3<f32>(origin.xyz) * 16.0 + local - g.cam_frac.xyz;
    return vec4(pos, 1.0);
}

@vertex
fn vs_face(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let f = vi / 6u;
    let w0 = records[f * 4u];
    let w1 = records[f * 4u + 1u];
    let w2 = records[f * 4u + 2u];
    let w3 = records[f * 4u + 3u];
    let ao = (w0 >> 16u) & 255u;
    let a = vec4<u32>(ao & 3u, (ao >> 2u) & 3u, (ao >> 4u) & 3u, (ao >> 6u) & 3u);
    let flip = a.x + a.z < a.y + a.w;
    var c = TRI[vi % 6u];
    if flip {
        c = TRI_FLIP[vi % 6u];
    }
    let dir = (w0 >> 12u) & 7u;
    let local = vec3<f32>(f32(w0 & 15u), f32((w0 >> 4u) & 15u), f32((w0 >> 8u) & 15u)) + CORNERS[dir * 4u + c];
    let pos = finish(local, origins[ii]);
    var out: VOut;
    out.clip = g.view_proj * pos;
    out.dist = length(pos.xyz);
    out.uv = UVS[c];
    let fr = texture_frame(w1 & 65535u);
    out.layer = fr.a;
    out.layer2 = fr.b;
    out.blend = fr.blend;
    var lw = w2;
    if c >= 2u {
        lw = w3;
    }
    let l = (lw >> ((c & 1u) * 16u)) & 65535u;
    out.light = unpack_light(l);
    out.color = tint_color(w1) * SHADE[dir] * AO[a[c]];
    return out;
}

fn decode_pos(w: u32) -> vec3<f32> {
    return (vec3<f32>(f32(w & 1023u), f32((w >> 10u) & 1023u), f32((w >> 20u) & 1023u)) - 32.0) / 16.0;
}

@vertex
fn vs_quad(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let q = vi / 6u;
    let c = TRI[vi % 6u];
    let base = q * 8u;
    let p0 = decode_pos(records[base]);
    let p1 = decode_pos(records[base + 1u]);
    let p2 = decode_pos(records[base + 2u]);
    let local = decode_pos(records[base + c]);
    let w4 = records[base + 4u];
    let uvw = records[base + 5u];
    let u0 = f32(uvw & 31u) / 16.0;
    let v0 = f32((uvw >> 5u) & 31u) / 16.0;
    let u1 = f32((uvw >> 10u) & 31u) / 16.0;
    let v1 = f32((uvw >> 15u) & 31u) / 16.0;
    var uvs = array<vec2<f32>, 4>(vec2(u0, v1), vec2(u1, v1), vec2(u1, v0), vec2(u0, v0));
    var lw = records[base + 6u];
    if c >= 2u {
        lw = records[base + 7u];
    }
    let l = (lw >> ((c & 1u) * 16u)) & 65535u;
    let n = normalize(cross(p1 - p0, p2 - p1));
    let shade = 0.75 + 0.25 * n.y - 0.15 * abs(n.x);
    let pos = finish(local, origins[ii]);
    var out: VOut;
    out.clip = g.view_proj * pos;
    out.dist = length(pos.xyz);
    out.uv = uvs[c];
    let fr = texture_frame(w4 & 65535u);
    out.layer = fr.a;
    out.layer2 = fr.b;
    out.blend = fr.blend;
    out.light = unpack_light(l);
    out.color = tint_color(w4) * shade;
    return out;
}

fn lightmap(v: f32) -> f32 {
    // Classic brightness table, halfway towards the "bright" gamma setting.
    let b = v / (3.0 - 2.0 * v);
    let bright = 1.0 - pow(1.0 - b, 4.0);
    return mix(b, bright, 0.5) * 0.96 + 0.04;
}

fn shade(in: VOut, albedo: vec4<f32>) -> vec4<f32> {
    let ambient = f32(g.ids.y) / 15.0;
    let sky = max(in.light.x * g.params.x, ambient);
    let block = in.light.yzw;
    let light = max(vec3(lightmap(sky)) * mix(vec3(0.55, 0.6, 0.85), vec3(1.0), g.params.x), vec3(lightmap(block.x), lightmap(block.y), lightmap(block.z)));
    var rgb = albedo.rgb * in.color * light;
    let fog = clamp((in.dist - g.params.y) / max(g.params.z - g.params.y, 1.0), 0.0, 1.0);
    rgb = mix(rgb, g.fog_color.rgb, fog);
    return vec4(rgb, albedo.a);
}

fn sample(in: VOut) -> vec4<f32> {
    let a = textureSample(texture_arrays[g.ids.x], nearest_mipped, in.uv, in.layer);
    if in.blend <= 0.0 {
        return a;
    }
    let b = textureSample(texture_arrays[g.ids.x], nearest_mipped, in.uv, in.layer2);
    return mix(a, b, in.blend);
}

@fragment
fn fs_opaque(in: VOut) -> @location(0) vec4<f32> {
    let albedo = sample(in);
    return vec4(shade(in, albedo).rgb, 1.0);
}

@fragment
fn fs_cutout(in: VOut) -> @location(0) vec4<f32> {
    let albedo = sample(in);
    if albedo.a < 0.5 {
        discard;
    }
    return vec4(shade(in, albedo).rgb, 1.0);
}

@fragment
fn fs_translucent(in: VOut) -> @location(0) vec4<f32> {
    let albedo = sample(in);
    return shade(in, albedo);
}
