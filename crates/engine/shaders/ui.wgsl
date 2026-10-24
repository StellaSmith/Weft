struct Quad {
    // Corner, and the two edge vectors, in physical pixels.
    p0: vec2<f32>,
    u: vec2<f32>,
    v: vec2<f32>,
    // Text effect slot (font quads, top bit set on shadows) or GUI fill slot (mode 4), and the
    // glyph's number in its text.
    fx: u32,
    index: f32,
    uv: vec4<f32>,
    color: u32,
    tex: u32,
    layer: u32,
    // 0 solid colour, 1 texture, 2 texture array layer, 3 font (alpha from red), 4 GUI fill shader
    mode: u32,
    // Visible rectangle (x0, y0, x1, y1) in pixels; empty = everywhere.
    rect: vec4<f32>,
}

struct Screen {
    // Pixels, seconds since start, GUI scale.
    size: vec4<f32>,
}

// What a text effect (`shaders/fx/<name>.wgsl`, `fn effect(g: Glyph) -> Glyph`) may change.
struct Glyph {
    // Added to this corner of the glyph, physical pixels.
    offset: vec2<f32>,
    // Linear colour and alpha.
    color: vec4<f32>,
    time: f32,
    // Number of the glyph in its text, for waves running along it.
    index: f32,
    // Which corner, 0..1 in each axis.
    corner: vec2<f32>,
    // Physical pixels per GUI pixel.
    scale: f32,
    // The dark copy drawn under the text.
    shadow: bool,
}

// A themed panel filled by a shader (`shaders/gui/<name>.wgsl`, `fn fill(p: GuiPixel) -> vec4<f32>`).
struct GuiPixel {
    // 0..1 across the panel.
    uv: vec2<f32>,
    // Panel size in physical pixels.
    size: vec2<f32>,
    // The theme colour, linear.
    color: vec4<f32>,
    time: f32,
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
    @location(6) @interpolate(flat) fx: u32,
    @location(7) @interpolate(flat) size: vec2<f32>,
}

fn unpack(c: u32) -> vec4<f32> {
    let v = vec4<f32>(f32((c >> 24u) & 255u), f32((c >> 16u) & 255u), f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
    return vec4(pow(v.rgb, vec3(2.2)), v.a);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    let q = quads[vi / 6u];
    let c = CORNER[vi % 6u];
    var p = q.p0 + q.u * c.x + q.v * c.y;
    var out: VOut;
    out.color = unpack(q.color);
    if q.mode == 3u && (q.fx & 0x7fffffffu) != 0u {
        let g = apply_fx(q.fx & 0x7fffffffu, Glyph(vec2(0.0), out.color, screen.size.z, q.index, c, screen.size.w, (q.fx >> 31u) == 1u));
        p += g.offset;
        out.color = g.color;
    }
    out.clip = vec4(p.x / screen.size.x * 2.0 - 1.0, 1.0 - p.y / screen.size.y * 2.0, 0.0, 1.0);
    out.uv = mix(q.uv.xy, q.uv.zw, c);
    out.tex = q.tex;
    out.layer = q.layer;
    out.mode = q.mode;
    out.rect = q.rect;
    out.fx = q.fx;
    out.size = vec2(length(q.u), length(q.v));
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
    if in.mode == 4u {
        return gui_fill(in.fx, GuiPixel(in.uv, in.size, in.color, screen.size.z));
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
