// Brightness swells and fades.
fn effect(g: Glyph) -> Glyph {
    var o = g;
    if !g.shadow {
        let k = 0.75 + 0.25 * sin(g.time * 4.0);
        o.color = vec4(mix(g.color.rgb * k, vec3(1.0), (k - 0.75) * 1.2), g.color.a);
    }
    return o;
}
