// Wisps: a shell that breathes, with holes sweeping across it.
fn shade(p: EntityPixel) -> vec4<f32> {
    let pulse = 0.75 + 0.25 * sin(p.time * 3.0);
    let holes = fract(sin(dot(floor(p.uv * 64.0), vec2(12.9898, 78.233))) * 43758.5453);
    let sweep = fract(p.time * 0.5);
    let cut = select(1.0, 0.0, holes > 0.55 + 0.4 * abs(sweep - 0.5));
    let lit = max(p.light, vec3(0.35)) * pulse;
    return vec4(p.albedo.rgb * lit * vec3(0.9, 1.0, 1.2), p.albedo.a * cut);
}
