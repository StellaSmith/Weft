// The daydream's cold variant: the same sway, tinted blue and slightly washed out.
fn apply(p: PostPixel) -> vec4<f32> {
    let s = p.strength;
    let wave = vec2(sin(p.uv.y * 18.0 + p.time * 2.0), cos(p.uv.x * 14.0 + p.time * 1.7)) * 0.006 * s;
    let c = scene_at(p, p.uv + wave).rgb;
    let grey = dot(c, vec3(0.3, 0.59, 0.11));
    let cold = mix(c, vec3(grey) * vec3(0.8, 0.95, 1.25), 0.6 * s);
    let d = distance(p.uv, vec2(0.5));
    return vec4(cold * (1.0 - smoothstep(0.35, 0.75, d) * 0.6 * s), 1.0);
}
