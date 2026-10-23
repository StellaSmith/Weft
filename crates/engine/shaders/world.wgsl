struct Globals {
    view_proj: mat4x4<f32>,
    // Camera position inside its cube (0..16) in xyz.
    cam_frac: vec4<f32>,
    sky_color: vec4<f32>,
    fog_color: vec4<f32>,
    // x daylight 0..1, y fog start, z fog end, w time in seconds
    params: vec4<f32>,
    // x block texture array, y ambient light 0..15, z biome count, w blended colour tiles
    ids: vec4<u32>,
    // x brightness setting 0 (moody) .. 1 (bright)
    look: vec4<f32>,
    // Origins are relative to this cube.
    cam_cube: vec4<i32>,
    // x: start of the overlay table in `frames` (per texture: overlay texture + 1, or 0)
    extra: vec4<u32>,
}

@group(1) @binding(0) var<storage, read> g: Globals;
@group(1) @binding(1) var<storage, read> origins: array<vec4<i32>>;
@group(1) @binding(2) var<storage, read> records: array<u32>;
// Per biome: grass, foliage, water as 0xRRGGBB. Then the blended colours of loaded block
// columns, 0xFFRRGGBB, in 16x16 tiles addressed by column position modulo g.ids.w.
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
    // Overlay layer + 1 (0: none) and the tint it takes; without an overlay the tint is already
    // in `color`.
    @location(7) @interpolate(flat) overlay: u32,
    @location(8) tint: vec3<f32>,
}

fn unpack_rgb(c: u32) -> vec3<f32> {
    return vec3<f32>(f32((c >> 16u) & 255u), f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
}

fn column_tint(x: i32, z: i32, tint: u32) -> u32 {
    let t = i32(g.ids.w);
    let tx = ((x >> 4u) % t + t) % t;
    let tz = ((z >> 4u) % t + t) % t;
    let i = g.ids.z + u32(tz * t + tx) * 256u + u32((z & 15) * 16 + (x & 15));
    return biomes[i][tint - 1u];
}

// Tint at a block corner: average of the four columns touching it, falling back to the face's
// own biome where colours are not uploaded yet.
fn tint_color(word: u32, x: i32, z: i32) -> vec3<f32> {
    let tint = (word >> 24u) & 3u;
    if tint == 0u {
        return vec3(1.0);
    }
    var sum = vec3(0.0);
    var n = 0.0;
    for (var k = 0; k < 4; k++) {
        let c = column_tint(x - 1 + (k & 1), z - 1 + (k >> 1u), tint);
        if c != 0u {
            sum += unpack_rgb(c);
            n += 1.0;
        }
    }
    if n > 0.0 {
        return sum / n;
    }
    return unpack_rgb(biomes[(word >> 16u) & 255u][tint - 1u]);
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
    let f = vi / 4u;
    let w0 = records[f * 4u];
    let w1 = records[f * 4u + 1u];
    let w2 = records[f * 4u + 2u];
    let w3 = records[f * 4u + 3u];
    let ao = (w0 >> 16u) & 255u;
    let a = vec4<u32>(ao & 3u, (ao >> 2u) & 3u, (ao >> 4u) & 3u, (ao >> 6u) & 3u);
    let flip = a.x + a.z < a.y + a.w;
    // Indexed as (0 1 2)(0 2 3); starting one corner later puts the diagonal on 1-3.
    var c = vi % 4u;
    if flip {
        c = (c + 1u) % 4u;
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
    let o = origins[ii] + g.cam_cube;
    let tint = tint_color(w1, o.x * 16 + i32(local.x), o.z * 16 + i32(local.z));
    let ov = frames[g.extra.x + (w1 & 65535u)];
    if ov != 0u {
        out.overlay = texture_frame(ov - 1u).a + 1u;
        out.tint = tint;
        out.color = vec3(SHADE[dir] * AO[a[c]]);
    } else {
        out.overlay = 0u;
        out.tint = vec3(1.0);
        out.color = tint * SHADE[dir] * AO[a[c]];
    }
    return out;
}

fn decode_pos(w: u32) -> vec3<f32> {
    return (vec3<f32>(f32(w & 1023u), f32((w >> 10u) & 1023u), f32((w >> 20u) & 1023u)) - 32.0) / 16.0;
}

@vertex
fn vs_quad(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let q = vi / 4u;
    let c = vi % 4u;
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
    // Face shading blended by normal; plants opt out like vanilla's crossed squares.
    var shade = n.x * n.x * 0.6 + n.z * n.z * 0.8 + n.y * n.y * select(0.5, 1.0, n.y > 0.0);
    if (w4 & (1u << 26u)) != 0u {
        shade = 1.0;
    }
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
    let o = origins[ii] + g.cam_cube;
    out.color = tint_color(w4, o.x * 16 + i32(round(local.x)), o.z * 16 + i32(round(local.z))) * shade;
    out.overlay = 0u;
    out.tint = vec3(1.0);
    return out;
}

fn brightness_curve(level: f32) -> f32 {
    return level / (4.0 - 3.0 * level);
}

// Vanilla 26.1's lightmap: dim ambient, sky light that turns blue and weaker at night, warm block
// light that whitens towards both ends, then the Brightness setting bends it up keeping the hue.
fn lightmap(light: vec4<f32>) -> vec3<f32> {
    let day = clamp((g.params.x - 0.12) / 0.88, 0.0, 1.0);
    let sky = brightness_curve(light.x) * mix(0.24, 1.0, day);
    var c = max(vec3(0.04), vec3(brightness_curve(f32(g.ids.y) / 15.0))) + mix(vec3(0.478, 0.478, 1.0), vec3(1.0), day) * sky;
    let block = light.yzw;
    let p = (2.0 * block - 1.0) * (2.0 * block - 1.0);
    let tint = mix(vec3(1.0, 0.847, 0.549), vec3(1.0), 0.9 * p);
    c += tint * vec3(brightness_curve(block.x), brightness_curve(block.y), brightness_curve(block.z)) * 1.4;
    c = clamp(c, vec3(0.0), vec3(1.0));
    let m = max(max(c.x, c.y), c.z);
    let bright = c * ((1.0 - pow(1.0 - m, 4.0)) / max(m, 1e-4));
    return mix(c, bright, g.look.x);
}

fn shade(in: VOut, albedo: vec4<f32>) -> vec4<f32> {
    let light = lightmap(in.light);
    // Shading factors are meant for gamma space like vanilla's; the target is sRGB, so apply them
    // as their linear equivalent or AO and shadows come out washed out.
    var rgb = albedo.rgb * pow(in.color * light, vec3(2.2));
    let fog = clamp((in.dist - g.params.y) / max(g.params.z - g.params.y, 1.0), 0.0, 1.0);
    rgb = mix(rgb, g.fog_color.rgb, fog);
    return vec4(rgb, albedo.a);
}

// Nearest texels with antialiased borders (vanilla 26.1's sampleNearest): up close each texel
// keeps its colour but its edge blends over one screen pixel, so blocks look clean, not jagged.
fn nearest_uv(uv: vec2<f32>, du: vec2<f32>, dv: vec2<f32>) -> vec2<f32> {
    let size = vec2<f32>(textureDimensions(texture_arrays[g.ids.x]).xy);
    let per_pixel = sqrt(du * du + dv * dv) * size;
    let t = uv * size;
    let center = round(t) - 0.5;
    let off = clamp((t - center - 0.5) / max(per_pixel, vec2(1e-5)) + 0.5, vec2(0.0), vec2(1.0));
    return (center + off) / size;
}

fn sample(in: VOut) -> vec4<f32> {
    let du = dpdx(in.uv);
    let dv = dpdy(in.uv);
    let uv = nearest_uv(in.uv, du, dv);
    let a = textureSampleGrad(texture_arrays[g.ids.x], linear_mipped, uv, in.layer, du, dv);
    if in.blend <= 0.0 {
        return a;
    }
    let b = textureSampleGrad(texture_arrays[g.ids.x], linear_mipped, uv, in.layer2, du, dv);
    return mix(a, b, in.blend);
}

// The tinted overlay over the base texture; its tint is a gamma-space factor like the others.
fn with_overlay(in: VOut, albedo: vec4<f32>) -> vec4<f32> {
    if in.overlay == 0u {
        return albedo;
    }
    let du = dpdx(in.uv);
    let dv = dpdy(in.uv);
    let ov = textureSampleGrad(texture_arrays[g.ids.x], linear_mipped, nearest_uv(in.uv, du, dv), in.overlay - 1u, du, dv);
    return vec4(mix(albedo.rgb, ov.rgb * pow(in.tint, vec3(2.2)), ov.a), albedo.a);
}

@fragment
fn fs_opaque(in: VOut) -> @location(0) vec4<f32> {
    let albedo = with_overlay(in, sample(in));
    return vec4(shade(in, albedo).rgb, 1.0);
}

// Mip level from the smaller screen derivative: a plant quad seen almost edge-on would otherwise
// pick a tiny mip whose averaged texels cover the whole quad and draw it as a solid strip.
fn sample_cutout(in: VOut) -> vec4<f32> {
    let size = f32(textureDimensions(texture_arrays[g.ids.x]).x);
    let t = in.uv * size;
    let lod = max(log2(max(min(length(dpdx(t)), length(dpdy(t))), 1e-6)), 0.0);
    let uv = nearest_uv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let a = textureSampleLevel(texture_arrays[g.ids.x], linear_mipped, uv, in.layer, lod);
    if in.blend <= 0.0 {
        return a;
    }
    let b = textureSampleLevel(texture_arrays[g.ids.x], linear_mipped, uv, in.layer2, lod);
    return mix(a, b, in.blend);
}

@fragment
fn fs_cutout(in: VOut) -> @location(0) vec4<f32> {
    let albedo = with_overlay(in, sample_cutout(in));
    if albedo.a < 0.5 {
        discard;
    }
    return vec4(shade(in, albedo).rgb, 1.0);
}

@fragment
fn fs_translucent(in: VOut) -> @location(0) vec4<f32> {
    let albedo = with_overlay(in, sample(in));
    return shade(in, albedo);
}
