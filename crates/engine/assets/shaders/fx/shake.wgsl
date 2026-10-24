// Letters jitter in place, a few times a second.
fn effect(g: Glyph) -> Glyph {
    var o = g;
    let t = floor(g.time * 12.0);
    let h = fract(sin(vec2(g.index * 12.9898 + t, g.index * 78.233 + t * 1.7)) * 43758.5453);
    o.offset = round((h - 0.5) * 2.0) * g.scale;
    return o;
}
