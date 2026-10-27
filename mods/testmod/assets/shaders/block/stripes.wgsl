// Magenta bands sliding up over the block's own lit texture, two per block.
fn shade(p: BlockPixel) -> vec4<f32> {
    let band = step(0.5, fract(p.pos.y * 2.0 - p.time));
    let lit = p.albedo.rgb * p.light;
    return vec4(mix(lit, vec3(0.8, 0.05, 0.8), band * 0.7), p.albedo.a);
}
