// Letters bob up and down one after another.
fn effect(g: Glyph) -> Glyph {
    var o = g;
    o.offset.y = round(sin(g.time * 6.0 - g.index * 0.5) * 1.5) * g.scale;
    return o;
}
