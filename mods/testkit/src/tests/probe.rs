//! Color classes for pixel probes. Generous on purpose: they tell one material from another
//! (grass, sky, water, lava, a texture's main color), not shades.

pub type C = [u8; 3];

fn i(c: C) -> [i32; 3] {
    c.map(|v| v as i32)
}

pub fn lum(c: C) -> f32 {
    crate::runner::lum(c.map(|v| v as f32))
}

/// Saturated red (roses, red lamps, gizmo boxes).
pub fn red(c: C) -> bool {
    let [r, g, b] = i(c);
    r > g + 70 && r > b + 70
}

/// Blue-green (diamond, the player's shirt, glow crystals, circuit lines).
pub fn cyan(c: C) -> bool {
    let [r, g, b] = i(c);
    g > r + 40 && b > r + 40
}

/// Clearly blue: water, the player's trousers.
pub fn blue(c: C) -> bool {
    let [r, g, b] = i(c);
    b > r + 50 && b > g + 15
}

/// Wood and dirt.
pub fn brown(c: C) -> bool {
    let [r, g, b] = i(c);
    r > g + 8 && g > b + 5 && (50..200).contains(&r)
}

/// Gold, light gizmos, yellow text.
pub fn yellow(c: C) -> bool {
    let [r, g, b] = i(c);
    r > 170 && g > 140 && b < r - 70
}

/// Lava, fire.
pub fn orange(c: C) -> bool {
    let [r, g, b] = i(c);
    r > 180 && g > 60 && g < r - 50 && b < 80
}

/// The flat world's grass in daylight.
pub fn grass(c: C) -> bool {
    let [r, g, b] = i(c);
    g > r + 10 && g > b + 25 && (70..200).contains(&g)
}

/// Bright and nearly gray or white (glass, snow, skin highlights not included).
pub fn light(c: C) -> bool {
    c.iter().all(|&v| v > 170)
}

pub fn dark(c: C) -> bool {
    lum(c) < 40.0
}

/// Something reddish seen through water: water alone has much less red than green, a pink pig
/// under it about as much.
pub fn under_water(c: C) -> bool {
    let [r, g, b] = i(c);
    r + 12 > g && b > r + 40
}

/// The test mod's GUI circuit lines.
pub fn teal(c: C) -> bool {
    let [r, g, b] = i(c);
    g > r + 30 && b > r + 20
}

/// Skin of the default player model.
pub fn skin(c: C) -> bool {
    let [r, g, b] = i(c);
    r > 140 && r > g + 25 && g > b + 10
}
