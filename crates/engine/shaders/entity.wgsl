struct Box {
    // Unit cube -> camera-relative world, rows of a 3x4 matrix.
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // Box size in texels (w, h, d) for the classic unwrap; block faces: min corner in the block,
    // w = 1 when set.
    dims: vec4<f32>,
    // Texel origin and texture size.
    uvo: vec4<f32>,
    // Light colour (rgb) and hurt flash (a).
    color: vec4<f32>,
    // texture index, mode (0 skin, 1 block faces, 2 solid colour), face layers packed 16 bit.
    ids: vec4<u32>,
    ids2: vec4<u32>,
}

struct View {
    view_proj: mat4x4<f32>,
    fog_color: vec4<f32>,
    // x fog start, y fog end
    fog: vec4<f32>,
}

@group(1) @binding(0) var<storage, read> boxes: array<Box>;
@group(1) @binding(1) var<storage, read> view: View;

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
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) tex: u32,
    @location(3) @interpolate(flat) layer: u32,
    @location(4) @interpolate(flat) mode: u32,
    @location(5) dist: f32,
}

// Texel rectangle (x, y, w, h) of a face in the classic box unwrap.
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

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    return vertex(vi);
}

fn vertex(vi: u32) -> VOut {
    let b = boxes[vi / 36u];
    let face = (vi % 36u) / 6u;
    let c = TRI[vi % 6u];
    let p = CORNERS[face * 4u + c];
    let local = vec4(p, 1.0);
    let world = vec3(dot(b.r0, local), dot(b.r1, local), dot(b.r2, local));
    var out: VOut;
    out.clip = view.view_proj * vec4(world, 1.0);
    out.dist = length(world);
    let uv = UVS[c];
    out.mode = b.ids.y;
    out.tex = b.ids.x;
    if b.ids.y == 4u {
        // One texel for the whole box (pixels of held item sprites); `ids.w` = faces to skip.
        if (b.ids.w & (1u << face)) != 0u {
            out.clip = vec4(0.0, 0.0, 0.0, 1.0);
        }
        out.layer = b.ids.z;
        out.uv = b.uvo.xy;
    } else if b.ids.y == 1u || b.ids.y == 3u {
        var packed = b.ids.z;
        if face >= 2u && face < 4u {
            packed = b.ids.w;
        } else if face >= 4u {
            packed = b.ids2.x;
        }
        out.layer = (packed >> ((face & 1u) * 16u)) & 65535u;
        out.uv = uv;
        if b.dims.w > 0.5 {
            // A box inside a block (dims = min, uvo = max): the part of the texture the world
            // mesher would show there.
            let q = mix(b.dims.xyz, b.uvo.xyz, p);
            switch face {
                case 0u: { out.uv = vec2(q.z, 1.0 - q.y); }
                case 1u: { out.uv = vec2(1.0 - q.z, 1.0 - q.y); }
                case 2u: { out.uv = vec2(q.x, q.z); }
                case 3u: { out.uv = vec2(q.x, q.z); }
                case 4u: { out.uv = vec2(1.0 - q.x, 1.0 - q.y); }
                default: { out.uv = vec2(q.x, 1.0 - q.y); }
            }
        }
    } else {
        let r = face_rect(face, b.dims.xyz, b.uvo.xy);
        out.uv = (r.xy + uv * r.zw) / b.uvo.zw;
        out.layer = 0u;
    }
    let hurt = b.color.a;
    // Gamma-space shading applied in linear space, as in the world shader.
    out.color = vec4(pow(b.color.rgb * SHADE[face] * mix(vec3(1.0), vec3(1.0, 0.4, 0.4), hurt), vec3(2.2)), 1.0);
    return out;
}

// The held item: depth squeezed into the near half (reverse Z), in front of the world (nothing the
// player collides with comes closer than ~0.2 blocks, depth 0.3) but still sorted within itself.
@vertex
fn vs_hand(@builtin(vertex_index) vi: u32) -> VOut {
    var out = vertex(vi);
    out.clip.z = (0.5 + 0.5 * out.clip.z / out.clip.w) * out.clip.w;
    return out;
}

fn fog(in: VOut, rgb: vec3<f32>) -> vec3<f32> {
    let f = clamp((in.dist - view.fog.x) / max(view.fog.y - view.fog.x, 1.0), 0.0, 1.0);
    return mix(rgb, view.fog_color.rgb, f);
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    var albedo = vec4(1.0);
    if in.mode == 0u {
        albedo = textureSample(textures[in.tex], nearest_clamp, in.uv);
    } else if in.mode == 1u || in.mode == 3u {
        albedo = textureSample(texture_arrays[in.tex], nearest_mipped, in.uv, in.layer);
    } else if in.mode == 4u {
        albedo = textureSampleLevel(texture_arrays[in.tex], nearest_mipped, in.uv, in.layer, 0.0);
    }
    if in.mode == 3u {
        // Flames give their own light.
        if albedo.a < 0.5 {
            discard;
        }
        return vec4(albedo.rgb, 1.0);
    }
    if albedo.a < 0.5 {
        discard;
    }
    return vec4(fog(in, albedo.rgb * in.color.rgb), 1.0);
}

@fragment
fn fs_overlay(in: VOut) -> @location(0) vec4<f32> {
    // Block faces mode: the breaking cracks, lit like the block.
    if in.mode == 1u {
        let t = textureSample(texture_arrays[in.tex], nearest_mipped, in.uv, in.layer);
        if t.a < 0.1 {
            discard;
        }
        return vec4(t.rgb * in.color.rgb, t.a * view.fog.z);
    }
    return vec4(in.color.rgb, view.fog.z);
}
