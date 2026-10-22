//! Bitmap font from a `.hex` file (Unifont format). Glyphs are packed into one R8 atlas; advance
//! widths come from the inked columns so Latin and Cyrillic look proportional.

use rustc_hash::FxHashMap;

pub const CELL: u32 = 16;
const ATLAS_COLS: u32 = 64;

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
    fallback: Glyph,
}

impl Font {
    pub fn parse_hex(src: &str) -> Font {
        let mut bitmaps: Vec<(char, u32, Vec<u8>)> = Vec::new();
        for line in src.lines() {
            if line.starts_with('#') {
                continue;
            }
            let Some((cp, hex)) = line.split_once(':') else { continue };
            let Some(ch) = u32::from_str_radix(cp, 16).ok().and_then(char::from_u32) else { continue };
            let width = match hex.len() {
                32 => 8,
                64 => 16,
                _ => continue,
            };
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
            bitmaps.push((ch, width, bytes));
        }
        let rows = (bitmaps.len() as u32 + 1).div_ceil(ATLAS_COLS);
        let atlas_width = ATLAS_COLS * CELL;
        let atlas_height = rows * CELL;
        let mut atlas = vec![0u8; (atlas_width * atlas_height) as usize];
        let mut glyphs = FxHashMap::default();
        for (n, (ch, width, bytes)) in bitmaps.iter().enumerate() {
            let cell = n as u32 + 1;
            let (cx, cy) = ((cell % ATLAS_COLS) * CELL, (cell / ATLAS_COLS) * CELL);
            let bpr = (*width / 8) as usize;
            let (mut min_x, mut max_x) = (u32::MAX, 0);
            for y in 0..16u32 {
                for x in 0..*width {
                    let byte = bytes[y as usize * bpr + (x / 8) as usize];
                    if byte & (0x80 >> (x % 8)) != 0 {
                        atlas[((cy + y) * atlas_width + cx + x) as usize] = 255;
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                    }
                }
            }
            let (left, advance) = if min_x == u32::MAX {
                (0, if *ch == ' ' { 4 } else { *width as u8 / 2 })
            } else {
                (min_x as u8, (max_x - min_x + 2) as u8)
            };
            glyphs.insert(*ch, Glyph { cell, left, advance });
        }
        // Cell 0 stays empty and is used for a visible box when a glyph is missing.
        for i in 0..CELL {
            for j in [2, CELL - 3] {
                atlas[(j * atlas_width + i.clamp(2, CELL - 3)) as usize] = 255;
                atlas[(i.clamp(2, CELL - 3) * atlas_width + j) as usize] = 255;
            }
        }
        Font { glyphs, atlas_width, atlas_height, atlas, line_height: 16.0, fallback: Glyph { cell: 0, left: 2, advance: 13 } }
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
        let (cx, cy) = ((g.cell % ATLAS_COLS) * CELL, (g.cell / ATLAS_COLS) * CELL);
        let x0 = (cx + g.left as u32) as f32 / self.atlas_width as f32;
        let x1 = (cx + g.left as u32 + g.advance as u32 - 1) as f32 / self.atlas_width as f32;
        [x0, cy as f32 / self.atlas_height as f32, x1, (cy + CELL) as f32 / self.atlas_height as f32]
    }
}
