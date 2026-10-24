// Colours cycle along the text.
fn effect(g: Glyph) -> Glyph {
    var o = g;
    if !g.shadow {
        let h = fract(g.time * 0.5 - g.index * 0.06);
        let rgb = clamp(abs(fract(vec3(h) + vec3(1.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, vec3(0.0), vec3(1.0));
        o.color = vec4(pow(rgb, vec3(2.2)), g.color.a);
    }
    return o;
}
