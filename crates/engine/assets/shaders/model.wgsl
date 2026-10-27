// Entity models: boxes and free-form triangles of every kind in two static tables; each entity
// layer (skin or something worn) is one instance pointing at its bone matrices, posed on the CPU
// (hierarchy, keyframes, vanilla's limb swing) and uploaded per frame.

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) @interpolate(flat) tex: u32,
    @location(3) dist: f32,
    @location(4) @interpolate(flat) shader: u32,
    @location(5) @interpolate(flat) glow: u32,
}

fn place(bone: u32, p: vec3<f32>) -> vec3<f32> {
    let b = bones[bone];
    let q = vec4(p, 1.0);
    return vec3(dot(b.rows[0], q), dot(b.rows[1], q), dot(b.rows[2], q));
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let inst = instances[ii];
    var out: VOut;
    var world: vec3<f32>;
    var shade: f32;
    let boxes = inst.cuboid_count * 36u;
    if vi < boxes {
        let cub = cuboids[inst.cuboids + vi / 36u];
        if ((inst.roles >> cub.role) & 1u) == 0u {
            out.clip = vec4(0.0, 0.0, 0.0, 1.0);
            return out;
        }
        let face = (vi % 36u) / 6u;
        let c = TRI[vi % 6u];
        let grow = cub.inflate + inst.inflate;
        let local = cub.origin - vec3(grow) + CORNERS[face * 4u + c] * (cub.size + vec3(2.0 * grow));
        world = place(inst.bones + cub.bone, local);
        let r = cub.faces[face];
        out.uv = (r.xy + UVS[c] * r.zw) / inst.tex_size;
        shade = SHADE[face];
    } else {
        let k = vi - boxes;
        let v = corners[inst.vertices + k];
        if ((inst.roles >> v.role) & 1u) == 0u {
            out.clip = vec4(0.0, 0.0, 0.0, 1.0);
            return out;
        }
        world = place(inst.bones + v.bone, v.pos);
        out.uv = v.uv / inst.tex_size;
        // Face shading from the triangle's normal, like the world's free-form quads.
        let t = inst.vertices + k - k % 3u;
        let a = place(inst.bones + corners[t].bone, corners[t].pos);
        let b = place(inst.bones + corners[t + 1u].bone, corners[t + 1u].pos);
        let c2 = place(inst.bones + corners[t + 2u].bone, corners[t + 2u].pos);
        let n = normalize(cross(b - a, c2 - a) + vec3(0.0, 1e-6, 0.0));
        shade = n.x * n.x * 0.6 + n.z * n.z * 0.8 + n.y * n.y * select(0.5, 1.0, n.y > 0.0);
    }
    out.clip = view.view_proj * vec4(world, 1.0);
    out.dist = length(world);
    out.tex = inst.tex;
    var lit = vec3(1.0);
    if inst.light != 0xffffffffu {
        lit = lightmap(unpack_light(inst.light), view.light.x, view.light.y, view.light.z);
    }
    out.color = pow(inst.color * lit * shade * mix(vec3(1.0), vec3(1.0, 0.4, 0.4), inst.hurt), vec3(2.2));
    out.shader = inst.shader;
    out.glow = inst.glow;
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(textures[in.tex], nearest_clamp, in.uv);
    var light = in.color;
    if in.glow != 0xffffffffu {
        let e = textureSampleLevel(textures[in.glow], nearest_clamp, in.uv, 0.0).r;
        light = max(light, vec3(pow(e, 2.2)));
    }
    let c = entity_shade(in.shader, EntityPixel(in.uv, albedo, light, view.fog.w));
    if c.a < 0.5 {
        discard;
    }
    let f = fog_amount(in.dist, view.fog.x, view.fog.y);
    return vec4(mix(c.rgb, view.fog_color.rgb, f), 1.0);
}
