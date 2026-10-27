// Particles: camera-facing quads, one instance each, all kinds in one draw per blend mode.

var<private> CORNER: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
);

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) quad: vec2<f32>,
    @location(2) @interpolate(flat) data: vec4<u32>,
    @location(3) @interpolate(flat) life: f32,
    @location(4) dist: f32,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let p = particles[ii];
    let c = CORNER[vi % 6u];
    let world = p.pos_size.xyz + (view.right.xyz * (c.x - 0.5) + view.up.xyz * (0.5 - c.y)) * p.pos_size.w;
    var out: VOut;
    out.clip = view.view_proj * vec4(world, 1.0);
    let r = vec4<f32>(f32(p.data.z & 255u), f32((p.data.z >> 8u) & 255u), f32((p.data.z >> 16u) & 255u), f32(p.data.z >> 24u)) / 255.0;
    out.uv = mix(r.xy, r.zw, c);
    out.quad = c;
    out.data = p.data;
    out.life = p.extra.x;
    out.dist = length(world);
    return out;
}

fn unpack_color(c: u32) -> vec4<f32> {
    let v = vec4<f32>(f32((c >> 24u) & 255u), f32((c >> 16u) & 255u), f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
    return vec4(pow(v.rgb, vec3(2.2)), v.a);
}

fn shade(in: VOut) -> vec4<f32> {
    let albedo = textureSampleLevel(textures[in.data.y & PARTICLE_TEX_MASK], nearest_clamp, in.uv, 0.0);
    var color = unpack_color(in.data.x);
    color = particle_shade(in.data.y >> PARTICLE_SLOT_SHIFT, ParticleFrag(in.quad, albedo, color, in.life, view.params.w));
    if in.data.w != 0xffffffffu {
        let light = lightmap(unpack_light(in.data.w), view.params.z, view.look.x, view.look.y);
        color = vec4(color.rgb * pow(light, vec3(2.2)), color.a);
    }
    let fog = fog_amount(in.dist, view.params.x, view.params.y);
    return vec4(mix(color.rgb, view.fog_color.rgb, fog), color.a);
}

@fragment
fn fs_cutout(in: VOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if c.a < 0.5 {
        discard;
    }
    return vec4(c.rgb, 1.0);
}

@fragment
fn fs_blend(in: VOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if c.a < 0.01 {
        discard;
    }
    return c;
}
