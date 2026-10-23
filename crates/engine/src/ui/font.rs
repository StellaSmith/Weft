//! Bitmap font: an atlas image described by a `.font` file for the scripts it covers, Unifont
//! (`.hex`) for everything else. Glyphs are packed into one R8 atlas; advance widths come from the
//! inked columns so text looks proportional.

use rustc_hash::FxHashMap;

pub const CELL: u32 = 16;
const ATLAS_COLS: u32 = 64;
/// Cells are spaced with an empty pixel all around: with fractional text scales a quad's edge can
/// sample just outside its glyph, which must hit blank space and not the neighbouring glyph.
const STRIDE: u32 = CELL + 2;

fn cell_origin(cell: u32) -> (u32, u32) {
    ((cell % ATLAS_COLS) * STRIDE + 1, (cell / ATLAS_COLS) * STRIDE + 1)
}

#[derive(Clone, Copy)]
pub struct Glyph {
    /// Atlas cell.
    pub cell: u32,
    /// First inked column and advance in font pixels.
    pub left: u8,
    pub advance: u8,
}

pub struct Font {
    pub glyphs: FxHashMap<char, Glyph>,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub atlas: Vec<u8>,
    pub line_height: f32,
    /// Font pixels per pixel of the font's design (2 for the 8 pixel atlas): shadows and text
    /// placement step by this so glyph pixels stay whole.
    pub pixel: f32,
    /// Fixed cell width for monospaced text.
    pub mono: f32,
    fallback: Glyph,
}

impl Font {
    pub fn parse_hex(src: &str) -> Font {
        Self::new(src, None)
    }

    /// Unifont for everything, with the glyphs of a bitmap atlas (`.font` description and its
    /// image) drawn over it. Atlas pixels become `pixel` font pixels so both fill the 16 pixel cell.
    pub fn new(hex: &str, bitmap: Option<(&str, &crate::assets::Image)>) -> Font {
        // (char, bitmap rows of up to 16 pixels, first inked column and advance; `None` = from ink)
        let mut bitmaps: Vec<(char, [u16; 16], Option<(u8, u8)>)> = Vec::new();
        for line in hex.lines() {
            if line.starts_with('#') {
                continue;
            }
            let Some((cp, hex)) = line.split_once(':') else { continue };
            let Some(ch) = u32::from_str_radix(cp, 16).ok().and_then(char::from_u32) else { continue };
            let bpr = match hex.len() {
                32 => 1,
                64 => 2,
                _ => continue,
            };
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
            if bytes.len() != 16 * bpr {
                continue;
            }
            let mut rows = [0u16; 16];
            for (y, row) in rows.iter_mut().enumerate() {
                *row = if bpr == 1 { (bytes[y] as u16) << 8 } else { (bytes[y * 2] as u16) << 8 | bytes[y * 2 + 1] as u16 };
            }
            let metrics = (ch == ' ').then_some((0, 4)).or_else(|| rows.iter().all(|&r| r == 0).then_some((0, bpr as u8 * 4)));
            bitmaps.push((ch, rows, metrics));
        }

        let mut pixel = 1.0;
        let mut line_height = 16.0;
        if let Some((desc, img)) = bitmap {
            let mut cell = (8u32, 8u32);
            let mut grid: Vec<Vec<char>> = Vec::new();
            for line in desc.lines() {
                let Some((k, v)) = line.split_once(" = ") else { continue };
                match k.trim() {
                    "cell" => {
                        let n: Vec<u32> = v.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                        if let [w, h] = n[..] {
                            cell = (w, h);
                        }
                    }
                    "row" => grid.push(v.chars().collect()),
                    _ => {}
                }
            }
            let scale = (CELL / cell.1.max(1)).max(1);
            if cell.0 * scale <= CELL {
                pixel = scale as f32;
                line_height = ((cell.1 + 1) * scale) as f32;
                let mut by_char: FxHashMap<char, usize> = bitmaps.iter().enumerate().map(|(i, b)| (b.0, i)).collect();
                let mut put = |ch: char, rows: [u16; 16], metrics: Option<(u8, u8)>| match by_char.get(&ch) {
                    Some(&i) => bitmaps[i] = (ch, rows, metrics),
                    None => {
                        by_char.insert(ch, bitmaps.len());
                        bitmaps.push((ch, rows, metrics));
                    }
                };
                // Vanilla-like spacing: a space is four atlas pixels, glyphs keep one between them.
                put(' ', [0; 16], Some((0, (4 * scale) as u8)));
                for (gy, row) in grid.iter().enumerate() {
                    for (gx, &ch) in row.iter().enumerate() {
                        let mut rows = [0u16; 16];
                        let (mut min_x, mut max_x) = (u32::MAX, 0);
                        for y in 0..cell.1 {
                            for x in 0..cell.0 {
                                let (px, py) = (gx as u32 * cell.0 + x, gy as u32 * cell.1 + y);
                                if px >= img.width || py >= img.height || img.rgba[((py * img.width + px) * 4 + 3) as usize] < 128 {
                                    continue;
                                }
                                min_x = min_x.min(x);
                                max_x = max_x.max(x);
                                for sy in 0..scale {
                                    for sx in 0..scale {
                                        rows[(y * scale + sy) as usize] |= 0x8000 >> (x * scale + sx);
                                    }
                                }
                            }
                        }
                        if min_x != u32::MAX {
                            put(ch, rows, Some(((min_x * scale) as u8, ((max_x - min_x + 2) * scale) as u8)));
                        }
                    }
                }
            }
        }

        let rows = (bitmaps.len() as u32 + 1).div_ceil(ATLAS_COLS);
        let atlas_width = ATLAS_COLS * STRIDE;
        let atlas_height = rows * STRIDE;
        let mut atlas = vec![0u8; (atlas_width * atlas_height) as usize];
        let mut glyphs = FxHashMap::default();
        for (n, (ch, bits, metrics)) in bitmaps.iter().enumerate() {
            let cell = n as u32 + 1;
            let (cx, cy) = cell_origin(cell);
            let (mut min_x, mut max_x) = (u32::MAX, 0);
            for (y, &row) in bits.iter().enumerate() {
                for x in 0..16u32 {
                    if row & (0x8000 >> x) != 0 {
                        atlas[((cy + y as u32) * atlas_width + cx + x) as usize] = 255;
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                    }
                }
            }
            let (left, advance) = metrics.unwrap_or((min_x.min(255) as u8, (max_x + 2).saturating_sub(min_x).min(255) as u8));
            glyphs.insert(*ch, Glyph { cell, left, advance });
        }
        // Cell 0 stays empty and is used for a visible box when a glyph is missing.
        let (ox, oy) = cell_origin(0);
        for i in 0..CELL {
            for j in [2, CELL - 3] {
                atlas[((oy + j) * atlas_width + ox + i.clamp(2, CELL - 3)) as usize] = 255;
                atlas[((oy + i.clamp(2, CELL - 3)) * atlas_width + ox + j) as usize] = 255;
            }
        }
        // Cell width for fixed-width text: the widest digit.
        let mono = ('0'..='9').filter_map(|c| glyphs.get(&c)).map(|g: &Glyph| g.advance as f32).fold(0.0, f32::max).max(1.0);
        Font { glyphs, atlas_width, atlas_height, atlas, line_height, pixel, mono, fallback: Glyph { cell: 0, left: 2, advance: 13 } }
    }

    #[inline]
    pub fn glyph(&self, c: char) -> Glyph {
        self.glyphs.get(&c).copied().unwrap_or(self.fallback)
    }

    /// Advance of a string in font pixels (unscaled).
    pub fn width(&self, s: &str) -> f32 {
        s.chars().map(|c| self.glyph(c).advance as f32).sum()
    }

    pub fn uv(&self, g: Glyph) -> [f32; 4] {
        let (cx, cy) = cell_origin(g.cell);
        let x0 = (cx + g.left as u32) as f32 / self.atlas_width as f32;
        let x1 = (cx + g.left as u32 + g.advance as u32 - 1) as f32 / self.atlas_width as f32;
        [x0, cy as f32 / self.atlas_height as f32, x1, (cy + CELL) as f32 / self.atlas_height as f32]
    }
}
