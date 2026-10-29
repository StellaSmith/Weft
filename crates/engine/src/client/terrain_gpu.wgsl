// Terrain on the GPU: the same integer math as `weft_noise::fixed` and `server::terrain`, line
// by line, so every block comes out as the CPU makes it. One workgroup per cube: columns, then
// the cave grid, then the blocks.

const ONE: i32 = 4096;

// Description of the dimension's terrain (`terrain_gpu::describe`), words.
@group(1) @binding(0) var<storage, read> desc: array<i32>;
// Per cube: x, y, z, unused, then per mod feature 256 column heights and 25 corner heights.
@group(1) @binding(1) var<storage, read> jobs: array<i32>;
// Per cube: blocks two per word, biomes two per word, column heights, column biomes two per word.
@group(1) @binding(2) var<storage, read_write> outp: array<u32>;

struct Params {
    cubes: u32,
    custom: u32,
}
var<immediate> params: Params;

// Header word offsets.
const D_STONE: u32 = 0u;
const D_BASE: u32 = 1u;
const D_HAS_SEA: u32 = 2u;
const D_SEA: u32 = 3u;
const D_WATER: u32 = 4u;
const D_HAS_CAVES: u32 = 5u;
const D_LAVA: u32 = 6u;
const D_LAVA_FROM: u32 = 7u;
const D_LAVA_TO: u32 = 8u;
const D_FEATURES: u32 = 9u;
const D_RULES: u32 = 10u;
const D_CAVE_BIOMES: u32 = 11u;
const D_SEEDS: u32 = 12u;
const D_FEAT_OFF: u32 = 26u;
const D_RULE_OFF: u32 = 27u;
const D_CAVE_OFF: u32 = 28u;
const D_SCATTER_OFF: u32 = 29u;

// Seeds, in `D_SEEDS` order.
const S_CONT: u32 = 0u;
const S_TEMP: u32 = 1u;
const S_HUM: u32 = 2u;
const S_HILLS: u32 = 3u;
const S_MTN: u32 = 4u;
const S_PLAT: u32 = 5u;
const S_CAN: u32 = 6u;
const S_DUNE: u32 = 7u;
const S_CRAT: u32 = 8u;
const S_C1: u32 = 9u;
const S_C2: u32 = 10u;
const S_C3: u32 = 11u;
const S_C4: u32 = 12u;
const S_LAKE: u32 = 13u;

const FEATURE_WORDS: u32 = 8u;
const MASK_WORDS: u32 = 6u;
const RULE_WORDS: u32 = 24u;
const CAVE_WORDS: u32 = 5u;
const OUT_WORDS: u32 = 2560u;
const CUSTOM_WORDS: u32 = 281u;

fn seed(i: u32) -> u32 {
    return bitcast<u32>(desc[D_SEEDS + i]);
}

fn fmix(h0: u32) -> u32 {
    var h = h0;
    h ^= h >> 16u;
    h *= 0x85EBCA6Bu;
    h ^= h >> 13u;
    h *= 0xC2B2AE35u;
    return h ^ (h >> 16u);
}

fn hash(s: u32, x: i32, y: i32, z: i32) -> u32 {
    return fmix(s ^ fmix(bitcast<u32>(x) ^ fmix(bitcast<u32>(y) ^ fmix(bitcast<u32>(z) ^ 0x27D4EB2Fu))));
}

fn div_floor(p: i32, d: i32) -> vec2<i32> {
    var q = p / d;
    if (p % d != 0 && p < 0) {
        q -= 1;
    }
    return vec2<i32>(q, p - q * d);
}

fn lattice(p: i32, scale: i32) -> vec2<i32> {
    let s = clamp(scale, 1, (1 << 20) - 1);
    let d = div_floor(p, s);
    return vec2<i32>(d.x, i32((u32(d.y) * 4096u) / u32(s)));
}

fn fade(t: i32) -> i32 {
    let t3 = (((t * t) >> 12u) * t) >> 12u;
    return (t3 * (((t * (t * 6 - 15 * ONE)) >> 12u) + 10 * ONE)) >> 12u;
}

fn lerp(a: i32, b: i32, t: i32) -> i32 {
    return a + (((b - a) * t) >> 12u);
}

fn grad2(h: u32, x: i32, y: i32) -> i32 {
    switch (h & 7u) {
        case 0u: { return x + y; }
        case 1u: { return x - y; }
        case 2u: { return -x + y; }
        case 3u: { return -x - y; }
        case 4u: { return x; }
        case 5u: { return -x; }
        case 6u: { return y; }
        default: { return -y; }
    }
}

fn grad3(h: u32, x: i32, y: i32, z: i32) -> i32 {
    switch (h % 12u) {
        case 0u: { return x + y; }
        case 1u: { return -x + y; }
        case 2u: { return x - y; }
        case 3u: { return -x - y; }
        case 4u: { return x + z; }
        case 5u: { return -x + z; }
        case 6u: { return x - z; }
        case 7u: { return -x - z; }
        case 8u: { return y + z; }
        case 9u: { return -y + z; }
        case 10u: { return y - z; }
        default: { return -y - z; }
    }
}

fn perlin2(s: u32, x: i32, y: i32, scale: i32) -> i32 {
    let lx = lattice(x, scale);
    let ly = lattice(y, scale);
    let u = fade(lx.y);
    let v = fade(ly.y);
    let g00 = grad2(hash(s, lx.x, ly.x, 0), lx.y, ly.y);
    let g10 = grad2(hash(s, lx.x + 1, ly.x, 0), lx.y - ONE, ly.y);
    let g01 = grad2(hash(s, lx.x, ly.x + 1, 0), lx.y, ly.y - ONE);
    let g11 = grad2(hash(s, lx.x + 1, ly.x + 1, 0), lx.y - ONE, ly.y - ONE);
    let a = lerp(g00, g10, u);
    let b = lerp(g01, g11, u);
    return (lerp(a, b, v) * 2867) >> 12u;
}

fn perlin3(s: u32, x: i32, y: i32, z: i32, sx: i32, sy: i32, sz: i32) -> i32 {
    let lx = lattice(x, sx);
    let ly = lattice(y, sy);
    let lz = lattice(z, sz);
    let u = fade(lx.y);
    let v = fade(ly.y);
    let w = fade(lz.y);
    let x00 = lerp(grad3(hash(s, lx.x, ly.x, lz.x), lx.y, ly.y, lz.y), grad3(hash(s, lx.x + 1, ly.x, lz.x), lx.y - ONE, ly.y, lz.y), u);
    let x10 = lerp(grad3(hash(s, lx.x, ly.x + 1, lz.x), lx.y, ly.y - ONE, lz.y), grad3(hash(s, lx.x + 1, ly.x + 1, lz.x), lx.y - ONE, ly.y - ONE, lz.y), u);
    let x01 = lerp(grad3(hash(s, lx.x, ly.x, lz.x + 1), lx.y, ly.y, lz.y - ONE), grad3(hash(s, lx.x + 1, ly.x, lz.x + 1), lx.y - ONE, ly.y, lz.y - ONE), u);
    let x11 = lerp(grad3(hash(s, lx.x, ly.x + 1, lz.x + 1), lx.y, ly.y - ONE, lz.y - ONE), grad3(hash(s, lx.x + 1, ly.x + 1, lz.x + 1), lx.y - ONE, ly.y - ONE, lz.y - ONE), u);
    return lerp(lerp(x00, x10, v), lerp(x01, x11, v), w);
}

fn fbm(s: u32, x: i32, z: i32, scale: i32, octaves: u32) -> i32 {
    var sum = 0;
    var norm = 0;
    for (var o = 0u; o < min(octaves, 12u); o++) {
        let amp = ONE >> o;
        sum += (perlin2(s + o * 7919u, x, z, max(scale >> o, 1)) * amp) >> 12u;
        norm += amp;
    }
    if (norm == 0) {
        return 0;
    }
    return sum * ONE / norm;
}

fn ramp(v: i32, lo: i32, hi: i32) -> i32 {
    if (hi == lo) {
        return select(0, ONE, v >= hi);
    }
    return clamp(clamp(v - lo, -(1 << 18), 1 << 18) * ONE / (hi - lo), 0, ONE);
}

fn ridge(s: u32, x: i32, z: i32, scale: i32) -> i32 {
    return ONE - min(abs(fbm(s, x, z, scale, 4u) * 2), ONE);
}

fn continent(x: i32, z: i32) -> i32 {
    return ((fbm(seed(S_CONT), x, z, 900 * 16, 4u) * 6554) >> 12u) + 614;
}

fn temperature(x: i32, z: i32) -> i32 {
    return (fbm(seed(S_TEMP), x, z, 1100 * 16, 3u) * 5734) >> 12u;
}

fn humidity(x: i32, z: i32) -> i32 {
    return (fbm(seed(S_HUM), x, z, 800 * 16, 3u) * 5734) >> 12u;
}

fn craters(x: i32, z: i32, radius: i32, depth: i32, spacing: i32) -> i32 {
    let sp = clamp(spacing, 16, 512 * 16);
    let cx = div_floor(x, sp).x;
    let cz = div_floor(z, sp).x;
    let bx = x >> 4u;
    let bz = z >> 4u;
    var total = 0;
    for (var dz = -1; dz <= 1; dz++) {
        for (var dx = -1; dx <= 1; dx++) {
            let h = hash(seed(S_CRAT), cx + dx, 0, cz + dz);
            let h2 = fmix(h);
            if (i32(h & 4095u) > 2458) {
                continue;
            }
            let r = ((clamp(radius, 16, 512 * 16) * (2048 + i32((h >> 12u) & 4095u))) >> 12u) >> 4u;
            let r2 = max(r * r, 1);
            let px = ((cx + dx) * sp + ((i32(h2 & 4095u) * sp) >> 12u)) >> 4u;
            let pz = ((cz + dz) * sp + ((i32((h2 >> 12u) & 4095u) * sp) >> 12u)) >> 4u;
            let d2 = (bx - px) * (bx - px) + (bz - pz) * (bz - pz);
            if (d2 >= r2 * 2) {
                continue;
            }
            let d = i32((u32(d2) * 4096u) / u32(r2));
            if (d < ONE) {
                total -= ((ONE - d) * depth) >> 12u;
            } else if (d < 6554) {
                let rim = max(ONE - abs(d - 5325) * ONE / 1229, 0);
                total += ((rim * depth) >> 12u) / 4;
            }
        }
    }
    return total;
}

fn preset(kind: i32, a: i32, b: i32, c: i32, x: i32, z: i32) -> i32 {
    switch (kind) {
        case 1: { return (clamp(continent(x, z) * 5 / 2, -ONE, ONE) * a) >> 12u; }
        case 2: { return (fbm(seed(S_HILLS), x, z, b, 4u) * a) >> 12u; }
        case 3: {
            let r = ridge(seed(S_MTN), x, z, b);
            return ((((r * r) >> 12u) * a) >> 12u) + ((fbm(seed(S_HILLS), x, z, 60 * 16, 4u) * a) >> 12u) / 7;
        }
        case 4: {
            let v = (ramp(fbm(seed(S_PLAT), x, z, b, 3u), 0, 1024) * a) >> 12u;
            let step = max(c, 1);
            return div_floor(v, step).x * step;
        }
        case 5: {
            let r = ridge(seed(S_CAN), x, z, min(b, 1 << 20) * 40);
            return -((ramp(r, 3994, ONE) * a) >> 12u);
        }
        case 6: {
            let wl = clamp(b, 1, (1 << 18) - 1);
            let warp = (fbm(seed(S_DUNE), x, z, wl * 4, 2u) * wl) >> 12u;
            let t = div_floor(x + z / 2 + warp, wl).y * ONE / wl;
            var profile = 0;
            if (t < 3277) {
                profile = t * ONE / 3277;
            } else {
                profile = (ONE - t) * ONE / 819;
            }
            return (profile * a) >> 12u;
        }
        case 7: { return craters(x, z, a, b, c); }
        default: { return 0; }
    }
}

// Postfix mask steps: kind, then 5 words (noise: seed, scale, octaves, from, to; ranges: from,
// to; all: count).
fn mask(at: u32, len: u32, x: i32, z: i32) -> i32 {
    var stack: array<i32, 8>;
    var sp = 0u;
    for (var i = 0u; i < len; i++) {
        let m = at + i * MASK_WORDS;
        let kind = desc[m];
        var v = 0;
        switch (kind) {
            case 1: { v = ramp(fbm(bitcast<u32>(desc[m + 1u]), x, z, desc[m + 2u], u32(desc[m + 3u])), desc[m + 4u], desc[m + 5u]); }
            case 2: { v = ramp(continent(x, z), desc[m + 4u], desc[m + 5u]); }
            case 3: { v = ramp(temperature(x, z), desc[m + 4u], desc[m + 5u]); }
            case 4: { v = ramp(humidity(x, z), desc[m + 4u], desc[m + 5u]); }
            case 5: {
                let n = u32(desc[m + 1u]);
                var acc = ONE;
                for (var k = sp - n; k < sp; k++) {
                    acc = (acc * stack[k]) >> 12u;
                }
                sp -= n;
                v = acc;
            }
            default: {
                sp -= 1u;
                v = ONE - stack[sp];
            }
        }
        stack[sp] = v;
        sp += 1u;
    }
    return stack[0];
}

struct Column {
    height: i32,
    biome: i32,
    rule: i32,
}

// Column at block (`bx`, `bz`); `ci` indexes the uploaded heights of mods' own features, `stride`
// of them per feature.
fn column(job: u32, bx: i32, bz: i32, ci: u32, stride: u32) -> Column {
    let x = bx * 16;
    let z = bz * 16;
    var h = desc[D_BASE];
    var strengths: array<i32, 32>;
    let nf = u32(desc[D_FEATURES]);
    let fo = u32(desc[D_FEAT_OFF]);
    for (var i = 0u; i < nf; i++) {
        let f = fo + i * FEATURE_WORDS;
        var w = ONE;
        if (desc[f + 7u] > 0) {
            w = mask(u32(desc[f + 6u]), u32(desc[f + 7u]), x, z);
        }
        strengths[i] = w;
        if (w > 0) {
            var fh = 0;
            if (desc[f + 5u] >= 0) {
                fh = jobs[job + 4u + u32(desc[f + 5u]) * stride + ci];
            } else {
                fh = preset(desc[f], desc[f + 1u], desc[f + 2u], desc[f + 3u], x, z);
            }
            h += (w * fh) >> 12u;
        }
    }
    let height = h >> 4u;
    let temp = temperature(x, z);
    let humid = humidity(x, z);
    let cont = continent(x, z);
    let nr = u32(desc[D_RULES]);
    let ro = u32(desc[D_RULE_OFF]);
    var rule = i32(nr) - 1;
    if (nr == 0u) {
        rule = 0;
    }
    for (var i = 0u; i < nr; i++) {
        let r = ro + i * RULE_WORDS;
        var ok = height >= desc[r + 7u] && height <= desc[r + 8u]
            && temp >= desc[r + 1u] && temp < desc[r + 2u]
            && humid >= desc[r + 3u] && humid < desc[r + 4u]
            && cont >= desc[r + 5u] && cont < desc[r + 6u];
        let fi = desc[r + 9u];
        if (ok && fi != -1) {
            ok = fi >= 0 && fi < 32 && strengths[fi] > desc[r + 10u] && strengths[fi] <= desc[r + 11u];
        }
        if (ok) {
            rule = i32(i);
            break;
        }
    }
    var biome = 0;
    if (u32(rule) < nr) {
        biome = desc[ro + u32(rule) * RULE_WORDS];
    }
    return Column(height, biome, rule);
}

fn cave_node(x0: i32, y0: i32, z0: i32) -> vec2<i32> {
    let x = x0 * 16;
    let y = y0 * 16;
    let z = z0 * 16;
    let a = perlin3(seed(S_C1), x, y, z, 48 * 16, 32 * 16, 48 * 16);
    let b = perlin3(seed(S_C2), x, y, z, 48 * 16, 32 * 16, 48 * 16);
    let spaghetti = 328 - (abs(a) + abs(b));
    let cheese = perlin3(seed(S_C3), x, y, z, 90 * 16, 50 * 16, 90 * 16) - 1720;
    return vec2<i32>(spaghetti, cheese);
}

fn carves(tunnel: bool, cavern: i32, closed: i32, depth: i32) -> bool {
    let d = clamp(depth, -(1 << 16), 1 << 16);
    let near = max(ONE - d * ONE / (12 * 16), 0);
    let pit = max(ONE - d * ONE / (24 * 16), 0);
    let need = ((((closed * 1024) >> 12u) * near) >> 12u) - (((((ONE - closed) * 492) >> 12u) * pit) >> 12u);
    return tunnel || cavern > need;
}

// 0 nothing, 1 lava, 2 stone.
fn lake(wy: i32) -> i32 {
    let band = div_floor(wy, 64).x;
    let h = hash(seed(S_LAKE), 0, band, 0);
    if ((h & 1u) == 0u) {
        return 0;
    }
    let level = band * 64 + 16 + i32((h >> 8u) % 32u);
    let d = level - wy;
    if (d >= 1 && d <= 4) {
        return 1;
    }
    if (d >= 5 && d <= 6) {
        return 2;
    }
    return 0;
}

fn cavern_roof(x: i32, z: i32) -> i32 {
    return ramp(perlin2(seed(S_C4), x * 16, z * 16, 120 * 16), 1638, 1024);
}

var<workgroup> col_h: array<i32, 256>;
var<workgroup> col_biome: array<i32, 256>;
var<workgroup> col_rule: array<i32, 256>;
var<workgroup> biomes: array<i32, 256>;
var<workgroup> corners: array<i32, 25>;
var<workgroup> nodes: array<vec2<i32>, 125>;
var<workgroup> max_h: atomic<i32>;

fn node(gy: u32, gz: u32, gx: u32, k: u32) -> i32 {
    let n = nodes[(gy * 5u + gz) * 5u + gx];
    return select(n.y, n.x, k == 0u);
}

// Trilinear blend at (x, y, z) inside the cube, as `server::terrain::interpolate`.
fn field(x: u32, y: u32, z: u32, k: u32) -> i32 {
    let fx = i32(x % 4u) * 1024;
    let fy = i32(y % 4u) * 1024;
    let fz = i32(z % 4u) * 1024;
    let gx = x / 4u;
    let gy = y / 4u;
    let gz = z / 4u;
    let a = lerp(lerp(node(gy, gz, gx, k), node(gy, gz, gx + 1u, k), fx), lerp(node(gy, gz + 1u, gx, k), node(gy, gz + 1u, gx + 1u, k), fx), fz);
    let b = lerp(lerp(node(gy + 1u, gz, gx, k), node(gy + 1u, gz, gx + 1u, k), fx), lerp(node(gy + 1u, gz + 1u, gx, k), node(gy + 1u, gz + 1u, gx + 1u, k), fx), fz);
    return lerp(a, b, fy);
}

fn surface(x: u32, z: u32) -> i32 {
    let cx = x / 4u;
    let cz = z / 4u;
    let fx = i32(x % 4u);
    let fz = i32(z % 4u);
    let c00 = corners[cz * 5u + cx];
    let c10 = corners[cz * 5u + cx + 1u];
    let c01 = corners[(cz + 1u) * 5u + cx];
    let c11 = corners[(cz + 1u) * 5u + cx + 1u];
    return (c00 * (4 - fx) + c10 * fx) * (4 - fz) + (c01 * (4 - fx) + c11 * fx) * fz;
}

// The block at local (x, y, z) of column `i`, as `Terrain::generate`.
fn block(x: u32, y: u32, z: u32, ox: i32, oy: i32, oz: i32, cave: bool) -> u32 {
    let i = z * 16u + x;
    let h = col_h[i];
    let nr = u32(desc[D_RULES]);
    let stone = desc[D_STONE];
    var sea = -2147483647 - 1;
    if (desc[D_HAS_SEA] != 0) {
        sea = desc[D_SEA];
    }
    let water = desc[D_WATER];
    var top = stone;
    var filler = stone;
    var depth = 0;
    var under_b = -1;
    var under_n = 0;
    var ice = -1;
    var bare = false;
    let ri = u32(col_rule[i]);
    if (ri < nr) {
        let r = u32(desc[D_RULE_OFF]) + ri * RULE_WORDS;
        if (desc[r + 22u] != 0 && h > desc[r + 23u]) {
            bare = true;
        }
        if (desc[r + 12u] >= 0) {
            top = desc[r + 12u];
        }
        if (desc[r + 17u] >= 0 && h < desc[r + 18u]) {
            top = desc[r + 17u];
        }
        if (desc[r + 13u] >= 0) {
            filler = desc[r + 13u];
        }
        depth = desc[r + 14u];
        under_b = desc[r + 15u];
        under_n = desc[r + 16u];
        ice = desc[r + 19u];
    }
    let wy = oy + i32(y);
    var s = 0;
    if (wy > h) {
        if (wy <= sea) {
            if (wy == sea && ice >= 0) {
                s = ice;
            } else {
                s = water;
            }
        }
    } else if (bare) {
        s = stone;
    } else if (wy == h) {
        s = top;
    } else if (wy >= h - depth) {
        if (under_b >= 0 && wy < h - depth + under_n) {
            s = under_b;
        } else {
            s = filler;
        }
    } else {
        s = stone;
    }
    if (cave && wy <= h && !(h <= sea && wy > h - 4)) {
        let tunnel = field(x, y, z, 0u) > 0;
        let cavern = field(x, y, z, 1u);
        let closed = cavern_roof(ox + i32(x), oz + i32(z));
        if (carves(tunnel, cavern, closed, surface(x, z) - wy * 16)) {
            let l = lake(wy);
            let in_band = wy >= desc[D_LAVA_FROM] && wy < desc[D_LAVA_TO];
            if (l == 1 && in_band) {
                s = desc[D_LAVA];
            } else if (l == 2 && in_band) {
                s = stone;
            } else {
                s = 0;
            }
        }
    }
    return u32(s);
}

@compute @workgroup_size(256)
fn generate(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let cube = wg.x;
    if (cube >= params.cubes) {
        return;
    }
    let job = cube * (4u + params.custom * CUSTOM_WORDS);
    let ox = jobs[job] * 16;
    let oy = jobs[job + 1u] * 16;
    let oz = jobs[job + 2u] * 16;
    if (t == 0u) {
        atomicStore(&max_h, -2147483647 - 1);
    }
    workgroupBarrier();
    let c = column(job, ox + i32(t % 16u), oz + i32(t / 16u), t, CUSTOM_WORDS);
    col_h[t] = c.height;
    col_biome[t] = c.biome;
    col_rule[t] = c.rule;
    atomicMax(&max_h, c.height);
    if (t < 25u) {
        corners[t] = column(job, ox + i32(t % 5u) * 4, oz + i32(t / 5u) * 4, 256u + t, CUSTOM_WORDS).height;
    }
    workgroupBarrier();
    let mh = atomicLoad(&max_h);
    var sea = -2147483647 - 1;
    if (desc[D_HAS_SEA] != 0) {
        sea = desc[D_SEA];
    }
    let cave = desc[D_HAS_CAVES] != 0 && oy <= mh;
    if (cave && t < 125u) {
        nodes[t] = cave_node(ox + i32(t % 5u) * 4, oy + i32(t / 25u) * 4, oz + i32((t / 5u) % 5u) * 4);
    }
    workgroupBarrier();
    let out = cube * OUT_WORDS;
    // Biome per column: a cave biome deep enough under it, else the column's.
    var biome = c.biome;
    let ncb = u32(desc[D_CAVE_BIOMES]);
    let co = u32(desc[D_CAVE_OFF]);
    for (var k = 0u; k < ncb; k++) {
        let b = co + k * CAVE_WORDS;
        if (oy + 15 < c.height - desc[b + 1u] && perlin3(bitcast<u32>(desc[b + 4u]), (ox + i32(t % 16u)) * 16, (oy + 8) * 16, (oz + i32(t / 16u)) * 16, desc[b + 2u], desc[b + 2u], desc[b + 2u]) > desc[b + 3u]) {
            biome = desc[b];
            break;
        }
    }
    biomes[t] = biome;
    outp[out + 2048u + 128u + t] = bitcast<u32>(c.height);
    workgroupBarrier();
    if (t < 128u) {
        outp[out + 2048u + t] = u32(biomes[t * 2u]) | (u32(biomes[t * 2u + 1u]) << 16u);
        outp[out + 2048u + 128u + 256u + t] = u32(col_biome[t * 2u]) | (u32(col_biome[t * 2u + 1u]) << 16u);
        let z = t / 8u;
        let x = (t % 8u) * 2u;
        let empty = oy > max(mh, sea) + 16;
        for (var y = 0u; y < 16u; y++) {
            var a = 0u;
            var b = 0u;
            if (!empty) {
                a = block(x, y, z, ox, oy, oz, cave);
                b = block(x + 1u, y, z, ox, oy, oz, cave);
            }
            outp[out + ((y * 16u + z) * 16u + x) / 2u] = a | (b << 16u);
        }
    }
}

// 64-bit integers as (low, high) words, for `weft_noise::hash` (scattered things' sites).
fn mul32(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 65535u;
    let a1 = a >> 16u;
    let b0 = b & 65535u;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let mid = (p00 >> 16u) + (p01 & 65535u) + (p10 & 65535u);
    return vec2<u32>((p00 & 65535u) | (mid << 16u), a1 * b1 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u));
}

fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let p = mul32(a.x, b.x);
    return vec2<u32>(p.x, p.y + a.x * b.y + a.y * b.x);
}

fn shr64(a: vec2<u32>, n: u32) -> vec2<u32> {
    if (n == 0u) {
        return a;
    }
    if (n >= 32u) {
        return vec2<u32>(a.y >> (n - 32u), 0u);
    }
    return vec2<u32>((a.x >> n) | (a.y << (32u - n)), a.y >> n);
}

fn shl64(a: vec2<u32>, n: u32) -> vec2<u32> {
    if (n == 0u) {
        return a;
    }
    if (n >= 32u) {
        return vec2<u32>(0u, a.x << (n - 32u));
    }
    return vec2<u32>(a.x << n, (a.y << n) | (a.x >> (32u - n)));
}

fn widen(v: i32) -> vec2<u32> {
    return vec2<u32>(u32(v), select(0u, 4294967295u, v < 0));
}

// `weft_noise::hash(seed, x, 0, z)`.
fn hash64(seed: vec2<u32>, x: i32, z: i32) -> vec2<u32> {
    var h = seed ^ mul64(widen(x), vec2<u32>(0x7F4A7C15u, 0x9E3779B9u));
    h = h ^ mul64(widen(z), vec2<u32>(0x9E3779F9u, 0x165667B1u));
    h = mul64(h ^ shr64(h, 30u), vec2<u32>(0x1CE4E5B9u, 0xBF58476Du));
    h = mul64(h ^ shr64(h, 27u), vec2<u32>(0x133111EBu, 0x94D049BBu));
    return h ^ shr64(h, 31u);
}

fn mod64(a: vec2<u32>, m: u32) -> u32 {
    let wrap = (4294967295u % m + 1u) % m;
    return ((a.y % m) * wrap + a.x % m) % m;
}

// Distant terrain (`client::lod`): a tile of 32x32 cells of 2^level blocks and a cell around it,
// each its column at the cell's middle: the block on top (water above the ground under the sea),
// the one under it, the biome. Caves are left out.
const LOD_SAMPLES: u32 = 1156u;
// Per sample: ground height (low half) and biome; and what scattered things put on it: the
// highest canopy top + 32768 and its block, and how many of its blocks they cover.
var<workgroup> lod_ground: array<u32, 1156>;
var<workgroup> canopy: array<atomic<u32>, 1156>;
var<workgroup> cover: array<atomic<u32>, 1156>;

@compute @workgroup_size(256)
fn lod(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let tile = wg.x;
    if (tile >= params.cubes) {
        return;
    }
    let job = tile * (4u + params.custom * LOD_SAMPLES);
    let ox = jobs[job];
    let oz = jobs[job + 1u];
    let s = 1 << u32(jobs[job + 2u]);
    let nr = u32(desc[D_RULES]);
    let stone = desc[D_STONE];
    for (var k = t; k < LOD_SAMPLES; k += 256u) {
        let bx = ox + (i32(k % 34u) - 1) * s + s / 2;
        let bz = oz + (i32(k / 34u) - 1) * s + s / 2;
        let c = column(job, bx, bz, k, LOD_SAMPLES);
        var top = stone;
        var under = stone;
        var ice = -1;
        let ri = u32(c.rule);
        if (ri < nr) {
            let r = u32(desc[D_RULE_OFF]) + ri * RULE_WORDS;
            let bare = desc[r + 22u] != 0 && c.height > desc[r + 23u];
            if (!bare) {
                if (desc[r + 12u] >= 0) {
                    top = desc[r + 12u];
                }
                if (desc[r + 17u] >= 0 && c.height < desc[r + 18u]) {
                    top = desc[r + 17u];
                }
                if (desc[r + 13u] >= 0 && desc[r + 14u] > 0) {
                    under = desc[r + 13u];
                }
            }
            ice = desc[r + 19u];
        }
        var y = c.height;
        if (desc[D_HAS_SEA] != 0 && c.height < desc[D_SEA]) {
            y = desc[D_SEA];
            top = desc[D_WATER];
            if (ice >= 0) {
                top = ice;
            }
            under = top;
        }
        let o = (tile * LOD_SAMPLES + k) * 2u;
        outp[o] = (u32(y) & 65535u) | (u32(top) << 16u);
        outp[o + 1u] = (u32(c.biome) & 65535u) | (u32(under) << 16u);
        lod_ground[k] = (u32(c.height) & 65535u) | (u32(c.biome) << 16u);
        atomicStore(&canopy[k], 0u);
        atomicStore(&cover[k], 0u);
    }
    workgroupBarrier();
    // Trees: every site whose canopy may reach the samples, found as `dim::scatter_site` does
    // (standing on the nearest sample's ground), its canopy laid over the samples it covers.
    let so = u32(desc[D_SCATTER_OFF]);
    var sea = -2147483647 - 1;
    if (desc[D_HAS_SEA] != 0) {
        sea = desc[D_SEA];
    }
    var at = so + 1u;
    let count = select(0u, u32(desc[so]), so != 0u);
    for (var n = 0u; n < count; n++) {
        let cell = desc[at];
        let reach = desc[at + 1u];
        let key = vec2<u32>(u32(desc[at + 2u]), u32(desc[at + 3u]));
        let nb = u32(desc[at + 4u]);
        let table = at + 5u;
        at = table + nb * 5u;
        let c0 = div_floor(ox - s - reach, cell).x;
        let c1 = div_floor(ox + 33 * s + reach, cell).x;
        let d0 = div_floor(oz - s - reach, cell).x;
        let d1 = div_floor(oz + 33 * s + reach, cell).x;
        let wide = u32(c1 - c0 + 1);
        let cells = wide * u32(d1 - d0 + 1);
        for (var k = t; k < cells; k += 256u) {
            let cx = c0 + i32(k % wide);
            let cz = d0 + i32(k / wide);
            let h = hash64(key, cx, cz);
            let px = cx * cell + i32(mod64(h, u32(cell)));
            let pz = cz * cell + i32(mod64(shr64(h, 8u), u32(cell)));
            let si = clamp(div_floor(px - ox, s).x + 1, 0, 33);
            let sj = clamp(div_floor(pz - oz, s).x + 1, 0, 33);
            let g = lod_ground[u32(sj) * 34u + u32(si)];
            let gh = bitcast<i32>(g << 16u) >> 16u;
            let biome = g >> 16u;
            if (gh < sea || biome >= nb) {
                continue;
            }
            let b = table + biome * 5u;
            let limit = vec2<u32>(u32(desc[b]), u32(desc[b + 1u]));
            let bits = shr64(shl64(h, 17u) | shr64(h, 47u), 11u);
            if (!(bits.y < limit.y || (bits.y == limit.y && bits.x < limit.x))) {
                continue;
            }
            let r = desc[b + 2u];
            let top = u32(gh + 1 + desc[b + 3u] + 32768);
            let packed = (top << 16u) | u32(desc[b + 4u]);
            let a0 = clamp(div_floor(px - r - ox, s).x + 1, 0, 33);
            let a1 = clamp(div_floor(px + r - ox, s).x + 1, 0, 33);
            let e0 = clamp(div_floor(pz - r - oz, s).x + 1, 0, 33);
            let e1 = clamp(div_floor(pz + r - oz, s).x + 1, 0, 33);
            for (var j = e0; j <= e1; j++) {
                for (var i = a0; i <= a1; i++) {
                    let x0 = ox + (i - 1) * s;
                    let z0 = oz + (j - 1) * s;
                    let wx = min(px + r + 1, x0 + s) - max(px - r, x0);
                    let wz = min(pz + r + 1, z0 + s) - max(pz - r, z0);
                    if (wx > 0 && wz > 0) {
                        let q = u32(j) * 34u + u32(i);
                        atomicAdd(&cover[q], u32(wx * wz));
                        atomicMax(&canopy[q], packed);
                    }
                }
            }
        }
    }
    workgroupBarrier();
    // A sample is canopy where trees cover a third of it (a forest from afar, single trees up
    // close), as high as the highest.
    for (var k = t; k < LOD_SAMPLES; k += 256u) {
        let p = atomicLoad(&canopy[k]);
        if (p != 0u && atomicLoad(&cover[k]) * 3u >= u32(s * s)) {
            let o = (tile * LOD_SAMPLES + k) * 2u;
            let leaves = p & 65535u;
            outp[o] = ((u32(i32(p >> 16u) - 32768)) & 65535u) | (leaves << 16u);
            outp[o + 1u] = (outp[o + 1u] & 65535u) | (leaves << 16u);
        }
    }
}
