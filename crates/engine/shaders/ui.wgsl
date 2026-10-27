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
    if q.mode == UI_MODE_FONT && (q.fx & 0x7fffffffu) != 0u {
        let g = apply_fx(q.fx & 0x7fffffffu, Glyph(vec2(0.0), out.color, screen.size.z, q.index, c, screen.size.w, (q.fx >> 31u) == 1u));
        p += g.offset;
        out.color = g.color;
    }
    out.clip = vec4(p.x / screen.size.x * 2.0 - 1.0, 1.0 - p.y / screen.size.y * 2.0, 0.0, 1.0);
    out.uv = mix(q.uv.xy, q.uv.zw, c);
    // A strip of frames (`.anim` next to the texture): the frame of this tick.
    if q.layer != 0u && q.mode != UI_MODE_FONT {
        let frames = f32(q.layer & 0xffffu);
        let f = floor(screen.size.z * 20.0 / f32(max(q.layer >> 16u, 1u))) % frames;
        out.uv.y = (out.uv.y + f) / frames;
    }
    out.tex = q.tex;
    out.layer = q.layer;
    out.mode = q.mode;
    out.rect = q.clip;
    out.fx = q.fx;
    out.size = vec2(length(q.u), length(q.v));
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    if in.rect.z > in.rect.x && (in.clip.x < in.rect.x || in.clip.x > in.rect.z || in.clip.y < in.rect.y || in.clip.y > in.rect.w) {
        discard;
    }
    if in.mode == UI_MODE_SOLID {
        return in.color;
    }
    if in.mode == UI_MODE_GUI {
        return gui_fill(in.fx, GuiPixel(in.uv, in.size, in.color, screen.size.z));
    }
    if in.mode == UI_MODE_ICON {
        let t = textureSampleLevel(textures[in.tex], nearest_clamp, in.uv, 0.0);
        if t.a < 0.1 {
            discard;
        }
        return t * in.color;
    }
    let t = textureSampleLevel(textures[in.tex], nearest_clamp, in.uv, 0.0);
    if in.mode == UI_MODE_FONT {
        return vec4(in.color.rgb, in.color.a * t.r);
    }
    return t * in.color;
}
