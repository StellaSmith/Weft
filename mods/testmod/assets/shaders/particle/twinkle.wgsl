// Sparks: the glint twinkles and drifts from white through cyan to violet as it ages.
fn shade(p: ParticleFrag) -> vec4<f32> {
    let flicker = 0.7 + 0.3 * sin(p.time * 30.0 + p.life * 20.0);
    let tint = mix(vec3(1.0, 1.0, 1.0), mix(vec3(0.3, 0.9, 1.0), vec3(0.7, 0.3, 1.0), p.life), min(p.life * 2.0, 1.0));
    return vec4(p.albedo.rgb * tint * flicker, p.albedo.a * p.color.a);
}
