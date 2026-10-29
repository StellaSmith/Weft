//! 16^3 block storage. Palette indices are packed into u64 words with 4, 8 or 16 bits per block
//! (always a divisor of 64, so an index never straddles two words).

pub const SIZE: usize = 16;
pub const VOLUME: usize = SIZE * SIZE * SIZE;

#[inline]
pub const fn index(x: usize, y: usize, z: usize) -> usize {
    (y * SIZE + z) * SIZE + x
}

/// The block at `index` `i`, within its cube.
#[inline]
pub const fn offset(i: usize) -> glam::IVec3 {
    glam::IVec3::new((i & 15) as i32, (i >> 8) as i32, ((i >> 4) & 15) as i32)
}

/// The cube at `i` in 0..27 of the 3 x 3 x 3 around a cube (13 is itself), as an offset.
#[inline]
pub const fn around(i: i32) -> glam::IVec3 {
    glam::IVec3::new(i % 3 - 1, i / 9 - 1, i / 3 % 3 - 1)
}

/// Every bit of `around_mask`.
pub const ALL_AROUND: u32 = (1 << 27) - 1;

/// The cubes whose meshes read the block at local `(x, y, z)`: itself and the neighbors it
/// borders (a mesh reads one block into each neighbor), bit `(dy + 1) * 9 + (dz + 1) * 3 + dx + 1`.
pub fn around_mask(x: i32, y: i32, z: i32) -> u32 {
    let sides = |v: i32| [v == 0, true, v == 15];
    let (sx, sy, sz) = (sides(x), sides(y), sides(z));
    let mut mask = 0;
    for dy in 0..3 {
        for dz in 0..3 {
            for dx in 0..3 {
                if sx[dx] && sy[dy] && sz[dz] {
                    mask |= 1 << (dy * 9 + dz * 3 + dx);
                }
            }
        }
    }
    mask
}

#[derive(Clone)]
enum Blocks {
    Uniform(u32),
    Packed { palette: Vec<u32>, bits: u32, words: Box<[u64]> },
}

#[derive(Clone)]
enum Light {
    Uniform(u32),
    Full(Box<[u32; VOLUME]>),
}

/// Light word: sky level in bits 0..8, block light red/green/blue in 8..16, 16..24, 24..32.
#[derive(Clone)]
pub struct Cube {
    blocks: Blocks,
    light: Light,
    pub biomes: Option<Box<[u16; 256]>>,
    /// World time it was simulated to when it was last unloaded (0: never), for growth to catch
    /// up on.
    pub ticked: u64,
    /// The last area edit of its dimension it got (`server::area`), so a saved cube gets the
    /// ones made while it was away, once.
    pub edits: u32,
}

impl Cube {
    pub fn uniform(state: u32, light: u32) -> Cube {
        Cube { blocks: Blocks::Uniform(state), light: Light::Uniform(light), biomes: None, ticked: 0, edits: 0 }
    }

    pub fn from_states(states: &[u32; VOLUME]) -> Cube {
        let mut c = Cube::uniform(states[0], 0);
        if states.iter().all(|&s| s == states[0]) {
            return c;
        }
        let mut palette: Vec<u32> = Vec::new();
        let mut last = u32::MAX;
        let mut last_i = 0u32;
        let mut indices = vec![0u16; VOLUME];
        for (i, &s) in states.iter().enumerate() {
            if s != last {
                last = s;
                last_i = match palette.iter().position(|&p| p == s) {
                    Some(p) => p as u32,
                    None => {
                        palette.push(s);
                        palette.len() as u32 - 1
                    }
                };
            }
            indices[i] = last_i as u16;
        }
        let bits = bits_for(palette.len());
        let mut words = vec![0u64; VOLUME * bits as usize / 64].into_boxed_slice();
        for (i, &p) in indices.iter().enumerate() {
            put(&mut words, bits, i, p as u32);
        }
        c.blocks = Blocks::Packed { palette, bits, words };
        c
    }

    #[inline]
    pub fn get(&self, i: usize) -> u32 {
        match &self.blocks {
            Blocks::Uniform(s) => *s,
            Blocks::Packed { palette, bits, words } => palette[take(words, *bits, i) as usize],
        }
    }

    /// Returns the previous state.
    pub fn set(&mut self, i: usize, state: u32) -> u32 {
        match &mut self.blocks {
            Blocks::Uniform(s) => {
                let old = *s;
                if old == state {
                    return old;
                }
                let palette = vec![old, state];
                let mut words = vec![0u64; VOLUME * 4 / 64].into_boxed_slice();
                put(&mut words, 4, i, 1);
                self.blocks = Blocks::Packed { palette, bits: 4, words };
                old
            }
            Blocks::Packed { palette, bits, words } => {
                let old = palette[take(words, *bits, i) as usize];
                if old == state {
                    return old;
                }
                let p = match palette.iter().position(|&p| p == state) {
                    Some(p) => p,
                    None => {
                        palette.push(state);
                        let needed = bits_for(palette.len());
                        if needed > *bits {
                            let mut grown = vec![0u64; VOLUME * needed as usize / 64].into_boxed_slice();
                            for j in 0..VOLUME {
                                put(&mut grown, needed, j, take(words, *bits, j));
                            }
                            *words = grown;
                            *bits = needed;
                        }
                        palette.len() - 1
                    }
                };
                put(words, *bits, i, p as u32);
                old
            }
        }
    }

    pub fn decode(&self, out: &mut [u32; VOLUME]) {
        match &self.blocks {
            Blocks::Uniform(s) => out.fill(*s),
            Blocks::Packed { palette, bits, words } => {
                let per = 64 / *bits as usize;
                let mask = (1u64 << *bits) - 1;
                for (w, &word) in words.iter().enumerate() {
                    for k in 0..per {
                        out[w * per + k] = palette[((word >> (k as u32 * *bits)) & mask) as usize];
                    }
                }
            }
        }
    }

    /// How this differs from `old` for meshes: `None` when every block and light value is the
    /// same, else the cubes around whose meshes read a changed one (`around_mask`), itself
    /// included.
    pub fn changed_around(&self, old: &Cube) -> Option<u32> {
        let same_blocks = match (&self.blocks, &old.blocks) {
            (Blocks::Uniform(a), Blocks::Uniform(b)) => a == b,
            (Blocks::Packed { palette: p, bits: b, words: w }, Blocks::Packed { palette: op, bits: ob, words: ow }) => p == op && b == ob && w == ow,
            _ => false,
        };
        let same_light = match (&self.light, &old.light) {
            (Light::Uniform(a), Light::Uniform(b)) => a == b,
            (Light::Full(a), Light::Full(b)) => a == b,
            _ => false,
        };
        if same_blocks && same_light {
            return None;
        }
        if let (Blocks::Uniform(_), Blocks::Uniform(_), Light::Uniform(_), Light::Uniform(_)) = (&self.blocks, &old.blocks, &self.light, &old.light) {
            return Some(ALL_AROUND);
        }
        // Something changed; inside blocks are read by this cube alone, so only the outer
        // layer decides which neighbors care.
        let mut mask = 1 << 13;
        for y in 0..16 {
            for z in 0..16 {
                let inner = (1..15).contains(&y) && (1..15).contains(&z);
                let step = if inner { 15 } else { 1 };
                for x in (0..16).step_by(step) {
                    let i = index(x, y, z);
                    if (!same_blocks && self.get(i) != old.get(i)) || (!same_light && self.light(i) != old.light(i)) {
                        mask |= around_mask(x as i32, y as i32, z as i32);
                    }
                }
                if mask == ALL_AROUND {
                    return Some(mask);
                }
            }
        }
        Some(mask)
    }

    /// `Some(state)` when every block is the same.
    pub fn uniform_state(&self) -> Option<u32> {
        match &self.blocks {
            Blocks::Uniform(s) => Some(*s),
            _ => None,
        }
    }

    /// States that may occur in the cube (can contain stale entries after edits).
    pub fn palette(&self) -> &[u32] {
        match &self.blocks {
            Blocks::Uniform(s) => std::slice::from_ref(s),
            Blocks::Packed { palette, .. } => palette,
        }
    }

    /// Drops unused palette entries and shrinks the bit width; collapses to uniform if possible.
    pub fn compact(&mut self) {
        if let Blocks::Packed { .. } = self.blocks {
            let mut states = Box::new([0u32; VOLUME]);
            self.decode(&mut states);
            let light = std::mem::replace(&mut self.light, Light::Uniform(0));
            let (biomes, ticked, edits) = (self.biomes.take(), self.ticked, self.edits);
            *self = Cube::from_states(&states);
            self.light = light;
            (self.biomes, self.ticked, self.edits) = (biomes, ticked, edits);
        }
        self.mask_light(!0);
    }

    /// Keeps only the light bits in `keep`; light the same all through becomes uniform again.
    pub fn mask_light(&mut self, keep: u32) {
        match &mut self.light {
            Light::Uniform(v) => *v &= keep,
            Light::Full(l) => {
                if keep != !0 {
                    l.iter_mut().for_each(|v| *v &= keep);
                }
                if l.iter().all(|&v| v == l[0]) {
                    self.light = Light::Uniform(l[0]);
                }
            }
        }
    }

    #[inline]
    pub fn light(&self, i: usize) -> u32 {
        match &self.light {
            Light::Uniform(v) => *v,
            Light::Full(l) => l[i],
        }
    }

    #[inline]
    pub fn set_light(&mut self, i: usize, v: u32) {
        match &mut self.light {
            Light::Uniform(u) => {
                if *u == v {
                    return;
                }
                let mut l = Box::new([*u; VOLUME]);
                l[i] = v;
                self.light = Light::Full(l);
            }
            Light::Full(l) => l[i] = v,
        }
    }

    pub fn fill_light(&mut self, v: u32) {
        self.light = Light::Uniform(v);
    }

    /// Sets every light value; the same all through becomes uniform.
    pub fn set_light_all(&mut self, l: Box<[u32; VOLUME]>) {
        self.light = if l.iter().all(|&v| v == l[0]) { Light::Uniform(l[0]) } else { Light::Full(l) };
    }

    /// Every light value, when they are not all the same.
    pub fn light_all(&self) -> Option<&[u32; VOLUME]> {
        match &self.light {
            Light::Uniform(_) => None,
            Light::Full(l) => Some(l),
        }
    }

    pub fn uniform_light(&self) -> Option<u32> {
        match &self.light {
            Light::Uniform(v) => Some(*v),
            Light::Full(_) => None,
        }
    }

    pub fn biome(&self, x: usize, z: usize) -> u16 {
        self.biomes.as_ref().map_or(0, |b| b[z * 16 + x])
    }

    // ------------------------------------------------------------ serialization

    /// Layout: palette len u16, palette as u32 (already translated by the caller), bits u8, words,
    /// light tag + data, biome tag + data. Compression is the caller's job.
    pub fn write(&self, out: &mut Vec<u8>, map: impl Fn(u32) -> u32) {
        match &self.blocks {
            Blocks::Uniform(s) => {
                out.extend_from_slice(&1u16.to_le_bytes());
                out.extend_from_slice(&map(*s).to_le_bytes());
                out.push(0);
            }
            Blocks::Packed { palette, bits, words } => {
                out.extend_from_slice(&(palette.len() as u16).to_le_bytes());
                for &p in palette {
                    out.extend_from_slice(&map(p).to_le_bytes());
                }
                out.push(*bits as u8);
                for w in words.iter() {
                    out.extend_from_slice(&w.to_le_bytes());
                }
            }
        }
        match &self.light {
            Light::Uniform(v) => {
                out.push(0);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Light::Full(l) => {
                out.push(1);
                for v in l.iter() {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        // Bit 0: biomes follow, bit 1: the simulated time, bit 2: the last area edit.
        out.push(self.biomes.is_some() as u8 | ((self.ticked != 0) as u8) << 1 | ((self.edits != 0) as u8) << 2);
        if let Some(b) = &self.biomes {
            for v in b.iter() {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        if self.ticked != 0 {
            out.extend_from_slice(&self.ticked.to_le_bytes());
        }
        if self.edits != 0 {
            out.extend_from_slice(&self.edits.to_le_bytes());
        }
    }

    pub fn read(data: &mut &[u8], map: impl Fn(u32) -> u32) -> Option<Cube> {
        let n = u16::from_le_bytes(take_bytes(data, 2)?.try_into().ok()?) as usize;
        let mut palette = Vec::with_capacity(n);
        for _ in 0..n {
            palette.push(map(u32::from_le_bytes(take_bytes(data, 4)?.try_into().ok()?)));
        }
        let bits = take_bytes(data, 1)?[0] as u32;
        let blocks = if bits == 0 {
            Blocks::Uniform(*palette.first()?)
        } else {
            if !matches!(bits, 4 | 8 | 16) || palette.len() > 1 << bits {
                return None;
            }
            let count = VOLUME * bits as usize / 64;
            let raw = take_bytes(data, count * 8)?;
            let words: Box<[u64]> = raw.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
            Blocks::Packed { palette, bits, words }
        };
        let light = match take_bytes(data, 1)?[0] {
            0 => Light::Uniform(u32::from_le_bytes(take_bytes(data, 4)?.try_into().ok()?)),
            _ => {
                let raw = take_bytes(data, VOLUME * 4)?;
                let mut l = Box::new([0u32; VOLUME]);
                for (d, c) in l.iter_mut().zip(raw.chunks_exact(4)) {
                    *d = u32::from_le_bytes(c.try_into().unwrap());
                }
                Light::Full(l)
            }
        };
        let flags = take_bytes(data, 1)?[0];
        let biomes = if flags & 1 != 0 {
            let mut b = Box::new([0u16; 256]);
            for (d, c) in b.iter_mut().zip(take_bytes(data, 512)?.chunks_exact(2)) {
                *d = u16::from_le_bytes([c[0], c[1]]);
            }
            Some(b)
        } else {
            None
        };
        let ticked = if flags & 2 != 0 { u64::from_le_bytes(take_bytes(data, 8)?.try_into().ok()?) } else { 0 };
        let edits = if flags & 4 != 0 { u32::from_le_bytes(take_bytes(data, 4)?.try_into().ok()?) } else { 0 };
        Some(Cube { blocks, light, biomes, ticked, edits })
    }
}

fn take_bytes<'a>(data: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if data.len() < n {
        return None;
    }
    let (a, b) = data.split_at(n);
    *data = b;
    Some(a)
}

fn bits_for(n: usize) -> u32 {
    match n {
        0..=16 => 4,
        17..=256 => 8,
        _ => 16,
    }
}

/// Values per word as a shift: widths are powers of two (`bits_for`), so a read is shifts and
/// masks instead of a division (most of what reading a block cost).
#[inline]
fn per_shift(bits: u32) -> u32 {
    6 - bits.trailing_zeros()
}

#[inline]
fn take(words: &[u64], bits: u32, i: usize) -> u32 {
    let ps = per_shift(bits);
    let w = words[i >> ps];
    ((w >> ((i as u32 & ((1 << ps) - 1)) * bits)) & ((1u64 << bits) - 1)) as u32
}

#[inline]
fn put(words: &mut [u64], bits: u32, i: usize, v: u32) {
    let ps = per_shift(bits);
    let shift = (i as u32 & ((1 << ps) - 1)) * bits;
    let mask = ((1u64 << bits) - 1) << shift;
    let w = &mut words[i >> ps];
    *w = (*w & !mask) | ((v as u64) << shift);
}

/// Bit of the face pair (`a`, `b`) in a connectivity mask; faces in `DIRS` order.
pub const fn face_pair(a: usize, b: usize) -> u16 {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    // Pairs (0,1)..(4,5) numbered in order: 5 + 4 + 3 + 2 + 1 = 15 bits.
    1 << (a * (11 - a) / 2 + b - a - 1)
}

pub const ALL_CONNECTED: u16 = 0x7FFF;

/// Faces reachable from face `from` through the inside of a cube with connectivity `conn`.
pub fn exits(conn: u16, from: usize) -> u8 {
    (0..6).filter(|&f| f != from && conn & face_pair(from, f) != 0).fold(0, |m, f| m | 1 << f)
}

impl Cube {
    /// Which faces see each other through blocks `open` lets through: a `face_pair` bit for every
    /// pair joined by a path of open blocks. Used to load only what can be seen.
    pub fn connectivity(&self, open: impl Fn(u32) -> bool) -> u16 {
        if let Some(s) = self.uniform_state() {
            return if open(s) { ALL_CONNECTED } else { 0 };
        }
        let mut states = [0u32; VOLUME];
        self.decode(&mut states);
        let mut free = [0u64; VOLUME / 64];
        for (i, &s) in states.iter().enumerate() {
            if open(s) {
                free[i / 64] |= 1 << (i % 64);
            }
        }
        let mut conn = 0u16;
        let mut stack: Vec<u16> = Vec::new();
        for start in 0..VOLUME {
            if free[start / 64] & 1 << (start % 64) == 0 {
                continue;
            }
            free[start / 64] &= !(1 << (start % 64));
            stack.push(start as u16);
            let mut faces = 0u8;
            while let Some(i) = stack.pop() {
                let i = i as usize;
                let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
                let edge = [x == 0, x == 15, y == 0, y == 15, z == 0, z == 15];
                for (f, &on) in edge.iter().enumerate() {
                    if on {
                        faces |= 1 << f;
                    }
                }
                let mut visit = |n: usize| {
                    if free[n / 64] & 1 << (n % 64) != 0 {
                        free[n / 64] &= !(1 << (n % 64));
                        stack.push(n as u16);
                    }
                };
                if x > 0 {
                    visit(i - 1);
                }
                if x < 15 {
                    visit(i + 1);
                }
                if y > 0 {
                    visit(i - 256);
                }
                if y < 15 {
                    visit(i + 256);
                }
                if z > 0 {
                    visit(i - 16);
                }
                if z < 15 {
                    visit(i + 16);
                }
            }
            for a in 0..6 {
                for b in a + 1..6 {
                    if faces & (1 << a) != 0 && faces & (1 << b) != 0 {
                        conn |= face_pair(a, b);
                    }
                }
            }
            if conn == ALL_CONNECTED {
                break;
            }
        }
        conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_grows_palette() {
        let mut c = Cube::uniform(0, 0);
        for i in 0..300 {
            c.set(i * 13 % VOLUME, i as u32 + 1);
        }
        for i in 0..300 {
            assert_eq!(c.get(i * 13 % VOLUME), i as u32 + 1);
        }
        let mut buf = Vec::new();
        c.write(&mut buf, |s| s);
        let back = Cube::read(&mut &buf[..], |s| s).unwrap();
        for i in 0..VOLUME {
            assert_eq!(back.get(i), c.get(i));
        }
    }

    #[test]
    fn connectivity() {
        let mut bits = std::collections::HashSet::new();
        for a in 0..6 {
            for b in a + 1..6 {
                assert!(bits.insert(face_pair(a, b)), "pair bits must be distinct");
                assert_eq!(face_pair(a, b), face_pair(b, a));
            }
        }
        assert_eq!(bits.iter().fold(0, |m, b| m | b), ALL_CONNECTED);
        // Solid cube with a straight air tunnel along x: only -X and +X see each other.
        let mut c = Cube::uniform(1, 0);
        for x in 0..16 {
            c.set(index(x, 8, 8), 0);
        }
        let conn = c.connectivity(|s| s == 0);
        assert_eq!(conn, face_pair(0, 1));
        assert_eq!(exits(conn, 0), 0b10);
        assert_eq!(Cube::uniform(0, 0).connectivity(|s| s == 0), ALL_CONNECTED);
        assert_eq!(Cube::uniform(1, 0).connectivity(|s| s == 0), 0);
    }

    #[test]
    fn compact_collapses() {
        let mut c = Cube::uniform(5, 0);
        c.set(10, 7);
        c.set(10, 5);
        c.compact();
        assert_eq!(c.uniform_state(), Some(5));
    }
}
