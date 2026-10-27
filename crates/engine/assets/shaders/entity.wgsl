

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) tex: u32,
    @location(3) @interpolate(flat) layer: u32,
    @location(4) @interpolate(flat) mode: u32,
    @location(5) dist: f32,
    @location(6) @interpolate(flat) glow: u32,
    // Unlit, unshaded color (linear) glowing texels take.
    @location(7) @interpolate(flat) tint: vec3<f32>,
}

// Texel rectangle (x, y, w, h) of a face in the classic box unwrap.
fn face_rect(face: u32, d: vec3<f32>, o: vec2<f32>) -> vec4<f32> {
    let w = d.x;
    let h = d.y;
    let z = d.z;
    switch face {
        case 0u: { return vec4(o.x, o.y + z, z, h); }
        case 1u: { return vec4(o.x + z + w, o.y + z, z, h); }
        case 2u: { return vec4(o.x + z + w, o.y, w, z); }
        case 3u: { return vec4(o.x + z, o.y, w, z); }
        case 4u: { return vec4(o.x + z, o.y + z, w, h); }
        default: { return vec4(o.x + z * 2.0 + w, o.y + z, w, h); }
    }
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    return box_vertex(boxes[vi / 36u], vi % 36u);
}

fn item_box(vi: u32, ii: u32) -> Box {
    let inst = boxes[ii];
    var b = item_boxes[inst.ids.z + vi / 36u];
    let r0 = b.rows[0];
    let r1 = b.rows[1];
    let r2 = b.rows[2];
    b.rows[0] = inst.rows[0].x * r0 + inst.rows[0].y * r1 + inst.rows[0].z * r2 + vec4(0.0, 0.0, 0.0, inst.rows[0].w);
    b.rows[1] = inst.rows[1].x * r0 + inst.rows[1].y * r1 + inst.rows[1].z * r2 + vec4(0.0, 0.0, 0.0, inst.rows[1].w);
    b.rows[2] = inst.rows[2].x * r0 + inst.rows[2].y * r1 + inst.rows[2].z * r2 + vec4(0.0, 0.0, 0.0, inst.rows[2].w);
    b.color.a = inst.color.a;
    b.ids2.y = inst.ids2.y;
    return b;
}

@vertex
fn vs_item(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    return box_vertex(item_box(vi, ii), vi % 36u);
}

// `v`: the vertex within the box, 6 per face.
fn box_vertex(b: Box, v: u32) -> VOut {
    let face = v / 6u;
    let c = TRI[v % 6u];
    let p = CORNERS[face * 4u + c];
    let local = vec4(p, 1.0);
    let world = vec3(dot(b.rows[0], local), dot(b.rows[1], local), dot(b.rows[2], local));
    var out: VOut;
    out.clip = scene.view_proj * vec4(world, 1.0);
    out.dist = length(world);
    let uv = UVS[c];
    out.mode = b.ids.y;
    out.tex = b.ids.x;
    out.glow = 0xffffffffu;
    if b.ids.y == BOX_MODE_TEXEL {
        // One texel for the whole box (pixels of held item sprites); `ids.w` = faces to skip.
        if (b.ids.w & (1u << face)) != 0u {
            out.clip = vec4(0.0, 0.0, 0.0, 1.0);
        }
        out.layer = b.ids.z;
        if out.layer < arrayLength(&glow_map) {
            out.glow = glow_map[out.layer];
        }
        out.uv = b.uvo.xy;
    } else if b.ids.y == BOX_MODE_FILLED {
        // Filled gizmo box: its alpha rides along as bits.
        out.layer = b.ids.z;
        out.uv = uv;
    } else if b.ids.y == BOX_MODE_FACES || b.ids.y == BOX_MODE_FLAMES {
        var packed = b.ids.z;
        if face >= 2u && face < 4u {
            packed = b.ids.w;
        } else if face >= 4u {
            packed = b.ids2.x;
        }
        out.layer = (packed >> ((face & 1u) * 16u)) & 65535u;
        if out.layer < arrayLength(&glow_map) {
            out.glow = glow_map[out.layer];
        }
        out.uv = uv;
        if b.dims.w > 0.5 {
            // A box inside a block (dims = min, uvo = max): the part of the texture the world
            // mesher would show there.
            let q = mix(b.dims.xyz, b.uvo.xyz, p);
            switch face {
                case 0u: { out.uv = vec2(q.z, 1.0 - q.y); }
                case 1u: { out.uv = vec2(1.0 - q.z, 1.0 - q.y); }
                case 2u: { out.uv = vec2(q.x, q.z); }
                case 3u: { out.uv = vec2(q.x, q.z); }
                case 4u: { out.uv = vec2(1.0 - q.x, 1.0 - q.y); }
                default: { out.uv = vec2(q.x, 1.0 - q.y); }
            }
        }
    } else {
        let r = face_rect(face, b.dims.xyz, b.uvo.xy);
        out.uv = (r.xy + uv * r.zw) / b.uvo.zw;
        out.layer = 0u;
    }
    let hurt = b.color.a;
    var tint = b.color.rgb;
    if b.ids.y == BOX_MODE_FACES && ((b.ids2.z >> face) & 1u) != 0u {
        tint *= vec3(f32((b.ids2.w >> 16u) & 255u), f32((b.ids2.w >> 8u) & 255u), f32(b.ids2.w & 255u)) / 255.0;
    }
    out.tint = pow(tint, vec3(2.2));
    var lit = vec3(1.0);
    if b.ids.y != BOX_MODE_SOLID && b.ids.y != BOX_MODE_FILLED && b.ids2.y != 0xffffffffu {
        lit = lightmap(unpack_light(b.ids2.y), scene.params.z, scene.light.x, scene.light.y);
    }
    // Gamma-space shading applied in linear space, as in the world shader.
    out.color = vec4(pow(tint * lit * SHADE[face] * mix(vec3(1.0), vec3(1.0, 0.4, 0.4), hurt), vec3(2.2)), 1.0);
    return out;
}

// The held item: depth squeezed into the near half (reverse Z), in front of the world (nothing the
// player collides with comes closer than ~0.2 blocks, depth 0.3) but still sorted within itself.
@vertex
fn vs_hand(@builtin(vertex_index) vi: u32) -> VOut {
    var out = box_vertex(boxes[vi / 36u], vi % 36u);
    out.clip.z = (0.5 + 0.5 * out.clip.z / out.clip.w) * out.clip.w;
    return out;
}

@vertex
fn vs_hand_item(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    var out = box_vertex(item_box(vi, ii), vi % 36u);
    out.clip.z = (0.5 + 0.5 * out.clip.z / out.clip.w) * out.clip.w;
    return out;
}

fn fog(in: VOut, rgb: vec3<f32>) -> vec3<f32> {
    let f = fog_amount(in.dist, scene.params.x, scene.params.y);
    return mix(rgb, scene.fog_color.rgb, f);
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    var albedo = vec4(1.0);
    if in.mode == BOX_MODE_SKIN {
        albedo = textureSample(textures[in.tex], nearest_clamp, in.uv);
    } else if in.mode == BOX_MODE_FACES || in.mode == BOX_MODE_FLAMES {
        albedo = textureSample(textures[in.layer], nearest_mipped, in.uv);
    } else if in.mode == BOX_MODE_TEXEL {
        albedo = textureSampleLevel(textures[in.layer], nearest_mipped, in.uv, 0.0);
    }
    if in.mode == BOX_MODE_FLAMES {
        // Flames give their own light.
        if albedo.a < 0.5 {
            discard;
        }
        return vec4(albedo.rgb, 1.0);
    }
    if albedo.a < 0.5 {
        discard;
    }
    var light = in.color.rgb;
    if (in.mode == BOX_MODE_FACES || in.mode == BOX_MODE_TEXEL) && in.glow != 0xffffffffu {
        let e = textureSampleLevel(textures[in.glow], nearest_clamp, in.uv, 0.0).r;
        light = max(light, in.tint * pow(e, 2.2));
    }
    return vec4(fog(in, albedo.rgb * light), 1.0);
}

@fragment
fn fs_overlay(in: VOut) -> @location(0) vec4<f32> {
    // Block faces mode: the breaking cracks, lit like the block.
    if in.mode == BOX_MODE_FACES {
        let t = textureSample(textures[in.layer], nearest_mipped, in.uv);
        if t.a < 0.1 {
            discard;
        }
        return vec4(t.rgb * in.color.rgb, t.a);
    }
    if in.mode == BOX_MODE_FILLED {
        return vec4(in.color.rgb, bitcast<f32>(in.layer));
    }
    return vec4(in.color.rgb, 1.0);
}
