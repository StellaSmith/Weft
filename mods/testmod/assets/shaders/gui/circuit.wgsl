// Machine screens: a dark board with traces and pulses running along them.
fn fill(p: GuiPixel) -> vec4<f32> {
    let px = p.uv * p.size / max(p.size.y / 120.0, 1.0);
    let cell = floor(px / 12.0);
    let f = fract(px / 12.0);
    let h = fract(sin(dot(cell, vec2(12.9898, 78.233))) * 43758.5453);
    var trace = 0.0;
    if h < 0.5 {
        trace = step(abs(f.y - 0.5), 0.06);
    } else {
        trace = step(abs(f.x - 0.5), 0.06);
    }
    let pulse = smoothstep(0.9, 1.0, fract(h * 7.0 - p.time * 0.4 + (cell.x + cell.y) * 0.05));
    let edge = min(min(p.uv.x, 1.0 - p.uv.x) * p.size.x, min(p.uv.y, 1.0 - p.uv.y) * p.size.y);
    let border = step(edge, 3.0);
    let base = vec3(0.012, 0.02, 0.03);
    let line = vec3(0.05, 0.16, 0.12) * trace + vec3(0.3, 1.0, 0.6) * trace * pulse;
    return vec4(base + line + vec3(0.25) * border, 0.97);
}
