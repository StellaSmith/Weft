struct Globals {
    view_proj: mat4x4<f32>,
    // Camera position inside its cube (0..16) in xyz.
    cam_frac: vec4<f32>,
    sky_color: vec4<f32>,
    fog_color: vec4<f32>,
    // x daylight 0..1, y fog start, z fog end, w time in seconds
    params: vec4<f32>,
    // y ambient light 0..15, z biome count, w blended colour tiles
    ids: vec4<u32>,
    // x brightness setting 0 (moody) .. 1 (bright)
    look: vec4<f32>,
    // Origins are relative to this cube.
    cam_cube: vec4<i32>,
    // x: start of the overlay table in `frames` (per texture: overlay texture + 1, or 0),
    // y: start of the glow map (glow mask texture per texture index, all ones: none)
    extra: vec4<u32>,
}

@group(1) @binding(0) var<storage, read> g: Globals;
@group(1) @binding(1) var<storage, read> origins: array<vec4<i32>>;
@group(1) @binding(2) var<storage, read> records: array<u32>;
// Per biome: grass, foliage, water as 0xRRGGBB. Then the blended colours of loaded block
// columns, 0xFFRRGGBB, in 16x16 tiles addressed by column position modulo g.ids.w.
// Per biome a colour per tint kind (`g.extra.z` kinds), then the blended columns.
@group(1) @binding(3) var<storage, read> biomes: array<u32>;
// Two words per texture name, see world_render.rs.
@group(1) @binding(4) var<storage, read> tex_info: array<u32>;
// Animated frames: texture | ticks << 16; then the overlay table and the glow map.
@group(1) @binding(5) var<storage, read> frames: array<u32>;

var<private> TRI_FLIP: array<u32, 6> = array<u32, 6>(1u, 2u, 3u, 1u, 3u, 0u);
var<private> AO: array<f32, 4> = array<f32, 4>(0.42, 0.62, 0.8, 1.0);

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Textures (bindless indices) of this frame and the next.
    @location(1) @interpolate(flat) layer: u32,
    @location(2) color: vec3<f32>,
    @location(3) light: vec4<f32>,
    @location(4) dist: f32,
    @location(5) @interpolate(flat) layer2: u32,
    @location(6) @interpolate(flat) blend: f32,
    // Overlay texture + 1 (0: none) and the tint it takes; without an overlay the tint is also
    // in `color` (`tint` then only colours emission).
    @location(7) @interpolate(flat) overlay: u32,
    @location(8) tint: vec3<f32>,
    // Glow mask texture (LabPBR emission), all ones: none.
    @location(9) @interpolate(flat) glow: u32,
}

fn glow_of(tex: u32) -> u32 {
    let i = g.extra.y + tex;
    if i >= arrayLength(&frames) {
        return 0xffffffffu;
    }
    return frames[i];
}

fn unpack_rgb(c: u32) -> vec3<f32> {
    return vec3<f32>(f32((c >> 16u) & 255u), f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
}

fn column_tint(x: i32, z: i32, tint: u32) -> u32 {
    let t = i32(g.ids.w);
    let tx = ((x >> 4u) % t + t) % t;
    let tz = ((z >> 4u) % t + t) % t;
    let i = g.ids.z + u32(tz * t + tx) * 256u + u32((z & 15) * 16 + (x & 15));
    return biomes[i * g.extra.z + tint - 1u];
}

// Tint at a block corner: average of the four columns touching it, falling back to the face's
// own biome where colours are not uploaded yet.
fn tint_color(tint: u32, biome: u32, x: i32, z: i32) -> vec3<f32> {
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
    return unpack_rgb(biomes[biome * g.extra.z + tint - 1u]);
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

// Corner light in a face word: four bits each.
fn unpack_corner_light(l: u32) -> vec4<f32> {
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
    let ao = (w0 >> FACE_AO_SHIFT) & 255u;
    let a = vec4<u32>(ao & 3u, (ao >> 2u) & 3u, (ao >> 4u) & 3u, (ao >> 6u) & 3u);
    let flip = a.x + a.z < a.y + a.w;
    // Indexed as (0 1 2)(0 2 3); starting one corner later puts the diagonal on 1-3.
    var c = vi % 4u;
    if flip {
        c = (c + 1u) % 4u;
    }
    let dir = (w0 >> FACE_DIR_SHIFT) & 7u;
    let local = vec3<f32>(f32(w0 & 15u), f32((w0 >> 4u) & 15u), f32((w0 >> 8u) & 15u)) + CORNERS[dir * 4u + c];
    let pos = finish(local, origins[ii]);
    var out: VOut;
    out.clip = g.view_proj * pos;
    out.dist = length(pos.xyz);
    out.uv = UVS[c];
    let fr = texture_frame(w1 & FACE_TEX_MASK);
    out.layer = fr.a;
    out.layer2 = fr.b;
    out.blend = fr.blend;
    out.glow = glow_of(fr.a);
    var lw = w2;
    if c >= 2u {
        lw = w3;
    }
    let l = (lw >> ((c & 1u) * 16u)) & 65535u;
    out.light = unpack_corner_light(l);
    let o = origins[ii] + g.cam_cube;
    let tint = tint_color((w0 >> FACE_TINT_SHIFT) & TINT_MASK, w1 >> FACE_BIOME_SHIFT, o.x * 16 + i32(local.x), o.z * 16 + i32(local.z));
    let ov = frames[g.extra.x + (w1 & FACE_TEX_MASK)];
    if ov != 0u {
        out.overlay = texture_frame(ov - 1u).a + 1u;
        out.tint = tint;
        out.color = vec3(SHADE[dir] * AO[a[c]]);
    } else {
        out.overlay = 0u;
        out.tint = tint;
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
    if ((uvw >> QUAD_TINT_SHIFT) & QUAD_FLAT) != 0u {
        shade = 1.0;
    }
    let pos = finish(local, origins[ii]);
    var out: VOut;
    out.clip = g.view_proj * pos;
    out.dist = length(pos.xyz);
    out.uv = uvs[c];
    let fr = texture_frame(w4 & FACE_TEX_MASK);
    out.layer = fr.a;
    out.layer2 = fr.b;
    out.blend = fr.blend;
    out.glow = glow_of(fr.a);
    out.light = unpack_corner_light(l);
    let o = origins[ii] + g.cam_cube;
    out.tint = tint_color((uvw >> QUAD_TINT_SHIFT) & TINT_MASK, w4 >> FACE_BIOME_SHIFT, o.x * 16 + i32(round(local.x)), o.z * 16 + i32(round(local.z)));
    out.color = out.tint * shade;
    out.overlay = 0u;
    return out;
}

fn shade(in: VOut, albedo: vec4<f32>) -> vec4<f32> {
    var light = in.color * lightmap(in.light, g.params.x, f32(g.ids.y) / 15.0, g.look.x);
    // Glowing texels are lifted towards full brightness by their mask: lit = albedo * max(light,
    // emission), so they stay their own (tinted, unshaded) colour in the dark.
    if in.glow != 0xffffffffu {
        let e = textureSampleLevel(textures[in.glow], nearest_clamp, in.uv, 0.0).r;
        light = max(light, select(in.tint, vec3(1.0), in.overlay != 0u) * e);
    }
    // Shading factors are meant for gamma space like vanilla's; the target is sRGB, so apply them
    // as their linear equivalent or AO and shadows come out washed out.
    var rgb = albedo.rgb * pow(light, vec3(2.2));
    let fog = fog_amount(in.dist, g.params.y, g.params.z);
    rgb = mix(rgb, g.fog_color.rgb, fog);
    return vec4(rgb, albedo.a);
}

// Nearest texels with antialiased borders (vanilla 26.1's sampleNearest): up close each texel
// keeps its colour but its edge blends over one screen pixel, so blocks look clean, not jagged.
fn nearest_uv(tex: u32, uv: vec2<f32>, du: vec2<f32>, dv: vec2<f32>) -> vec2<f32> {
    let size = vec2<f32>(textureDimensions(textures[tex]).xy);
    let per_pixel = sqrt(du * du + dv * dv) * size;
    let t = uv * size;
    let center = round(t) - 0.5;
    let off = clamp((t - center - 0.5) / max(per_pixel, vec2(1e-5)) + 0.5, vec2(0.0), vec2(1.0));
    return (center + off) / size;
}

fn sample(in: VOut) -> vec4<f32> {
    let du = dpdx(in.uv);
    let dv = dpdy(in.uv);
    let a = textureSampleGrad(textures[in.layer], linear_mipped, nearest_uv(in.layer, in.uv, du, dv), du, dv);
    if in.blend <= 0.0 {
        return a;
    }
    let b = textureSampleGrad(textures[in.layer2], linear_mipped, nearest_uv(in.layer2, in.uv, du, dv), du, dv);
    return mix(a, b, in.blend);
}

// The tinted overlay over the base texture; its tint is a gamma-space factor like the others.
fn with_overlay(in: VOut, albedo: vec4<f32>) -> vec4<f32> {
    if in.overlay == 0u {
        return albedo;
    }
    let du = dpdx(in.uv);
    let dv = dpdy(in.uv);
    let ov = textureSampleGrad(textures[in.overlay - 1u], linear_mipped, nearest_uv(in.overlay - 1u, in.uv, du, dv), du, dv);
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
    let size = f32(textureDimensions(textures[in.layer]).x);
    let t = in.uv * size;
    let lod = max(log2(max(min(length(dpdx(t)), length(dpdy(t))), 1e-6)), 0.0);
    let a = textureSampleLevel(textures[in.layer], linear_mipped, nearest_uv(in.layer, in.uv, dpdx(in.uv), dpdy(in.uv)), lod);
    if in.blend <= 0.0 {
        return a;
    }
    let b = textureSampleLevel(textures[in.layer2], linear_mipped, nearest_uv(in.layer2, in.uv, dpdx(in.uv), dpdy(in.uv)), lod);
    return mix(a, b, in.blend);
}

@fragment
fn fs_cutout(in: VOut) -> @location(0) vec4<f32> {
    let albedo = with_overlay(in, sample_cutout(in));
    // Vanilla's alpha test (GL_GREATER 0.1).
    if albedo.a <= 0.1 {
        discard;
    }
    return vec4(shade(in, albedo).rgb, 1.0);
}

@fragment
fn fs_translucent(in: VOut) -> @location(0) vec4<f32> {
    let albedo = with_overlay(in, sample(in));
    return shade(in, albedo);
}
