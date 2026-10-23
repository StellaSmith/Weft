struct Quad {
    // Corner, and the two edge vectors, in physical pixels.
    p0: vec2<f32>,
    u: vec2<f32>,
    v: vec2<f32>,
    pad: vec2<f32>,
    uv: vec4<f32>,
    color: u32,
    tex: u32,
    layer: u32,
    // 0 solid colour, 1 texture, 2 texture array layer, 3 font (alpha from red)
    mode: u32,
    // Visible rectangle (x0, y0, x1, y1) in pixels; empty = everywhere.
    rect: vec4<f32>,
}

struct Screen {
    size: vec4<f32>,
}

@group(1) @binding(0) var<storage, read> quads: array<Quad>;
@group(1) @binding(1) var<storage, read> screen: Screen;

var<private> CORNER: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
);

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) tex: u32,
    @location(3) @interpolate(flat) layer: u32,
    @location(4) @interpolate(flat) mode: u32,
    @location(5) @interpolate(flat) rect: vec4<f32>,
}

fn unpack(c: u32) -> vec4<f32> {
    let v = vec4<f32>(f32((c >> 24u) & 255u), f32((c >> 16u) & 255u), f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
    return vec4(pow(v.rgb, vec3(2.2)), v.a);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    let q = quads[vi / 6u];
    let c = CORNER[vi % 6u];
    let p = q.p0 + q.u * c.x + q.v * c.y;
    var out: VOut;
    out.clip = vec4(p.x / screen.size.x * 2.0 - 1.0, 1.0 - p.y / screen.size.y * 2.0, 0.0, 1.0);
    out.uv = mix(q.uv.xy, q.uv.zw, c);
    out.color = unpack(q.color);
    out.tex = q.tex;
    out.layer = q.layer;
    out.mode = q.mode;
    out.rect = q.rect;
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    if in.rect.z > in.rect.x && (in.clip.x < in.rect.x || in.clip.x > in.rect.z || in.clip.y < in.rect.y || in.clip.y > in.rect.w) {
        discard;
    }
    if in.mode == 0u {
        return in.color;
    }
    if in.mode == 2u {
        let t = textureSampleLevel(texture_arrays[in.tex], nearest_clamp, in.uv, in.layer, 0.0);
        if t.a < 0.1 {
            discard;
        }
        return t * in.color;
    }
    let t = textureSampleLevel(textures[in.tex], nearest_clamp, in.uv, 0.0);
    if in.mode == 3u {
        return vec4(in.color.rgb, in.color.a * t.r);
    }
    return t * in.color;
}
