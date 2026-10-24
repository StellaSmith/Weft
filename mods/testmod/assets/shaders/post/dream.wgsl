// A daydream: the picture sways, warms up and darkens at the edges, scaled by strength.
fn apply(p: PostPixel) -> vec4<f32> {
    let s = p.strength;
    let wave = vec2(sin(p.uv.y * 18.0 + p.time * 2.0), cos(p.uv.x * 14.0 + p.time * 1.7)) * 0.006 * s;
    let c = scene_at(p, p.uv + wave).rgb;
    let warm = c * mix(vec3(1.0), vec3(1.15, 1.0, 0.8), s);
    let d = distance(p.uv, vec2(0.5));
    let vignette = 1.0 - smoothstep(0.35, 0.75, d) * 0.6 * s;
    return vec4(warm * vignette, 1.0);
}
