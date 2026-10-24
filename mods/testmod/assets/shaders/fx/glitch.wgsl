// `{fx:testmod:glitch}`: letters jump sideways now and then, tinted towards cyan or magenta.
fn effect(g: Glyph) -> Glyph {
    var o = g;
    let t = floor(g.time * 8.0);
    let h = fract(sin(g.index * 91.7 + t * 13.1) * 43758.5453);
    if h > 0.8 {
        o.offset.x = (h - 0.9) * 20.0 * g.scale;
        if !g.shadow {
            o.color = vec4(select(vec3(0.2, 1.0, 1.0), vec3(1.0, 0.2, 1.0), h > 0.9), g.color.a);
        }
    }
    return o;
}
