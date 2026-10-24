//! Which way a block faces. Any block may face any of the six directions or only some of them;
//! a direction it cannot face makes no state (three allowed directions are three states).
//!
//! ```ignore
//! // A furnace: four directions, each lit or not (eight states), front towards the player.
//! r.block("furnace").facing_variants(Facings::HORIZONTAL, Toward::Player, 2, |dir, lit, s| ...);
//! ```

/// A direction, in face order (`-X +X -Y +Y -Z +Z`), so `face()` is the face index blocks and
/// support rules use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    West,
    East,
    Down,
    Up,
    North,
    South,
}

impl Dir {
    pub const ALL: [Dir; 6] = [Dir::West, Dir::East, Dir::Down, Dir::Up, Dir::North, Dir::South];

    pub fn face(self) -> u32 {
        self as u32
    }
    pub fn from_face(face: u32) -> Dir {
        Dir::ALL[(face % 6) as usize]
    }
    pub fn opposite(self) -> Dir {
        Dir::from_face(self.face() ^ 1)
    }
    pub fn offset(self) -> [i32; 3] {
        [[-1, 0, 0], [1, 0, 0], [0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1]][self as usize]
    }
    pub fn horizontal(self) -> bool {
        !matches!(self, Dir::Down | Dir::Up)
    }
    /// The direction closest to `v` among `allowed`.
    pub fn nearest(v: [f32; 3], allowed: Facings) -> Dir {
        let score = |d: Dir| {
            let o = d.offset();
            o[0] as f32 * v[0] + o[1] as f32 * v[1] + o[2] as f32 * v[2]
        };
        allowed.dirs().max_by(|a, b| score(*a).total_cmp(&score(*b))).unwrap_or(Dir::Up)
    }
}

/// The directions a block may face (a bit per face index).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Facings(pub u8);

impl Facings {
    pub const ALL: Facings = Facings(0b11_1111);
    pub const HORIZONTAL: Facings = Facings(0b11_0011);
    pub const VERTICAL: Facings = Facings(0b00_1100);

    pub const fn only(dirs: &[Dir]) -> Facings {
        let mut bits = 0;
        let mut i = 0;
        while i < dirs.len() {
            bits |= 1 << dirs[i] as u8;
            i += 1;
        }
        Facings(bits)
    }
    pub fn allows(self, d: Dir) -> bool {
        self.0 & (1 << d as u8) != 0
    }
    /// How many states one variant takes.
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }
    pub fn dirs(self) -> impl Iterator<Item = Dir> {
        Dir::ALL.into_iter().filter(move |&d| self.allows(d))
    }
    /// State index of facing `d` in variant `variant` (variants are blocks of `count()` states).
    pub fn index(self, d: Dir, variant: u32) -> u32 {
        variant * self.count() + (self.0 & ((1u8 << d as u8) - 1)).count_ones()
    }
    /// The facing and variant of state index `i`.
    pub fn parse(self, i: u32) -> (Dir, u32) {
        let n = self.count().max(1);
        (self.dirs().nth((i % n) as usize).unwrap_or(Dir::Up), i / n)
    }
}

/// How placing chooses the facing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Toward {
    /// The front looks at the player (furnaces, chests, dispensers).
    Player,
    /// Out of the face that was clicked (torches and ladders on walls, logs along it).
    Face,
}

impl Toward {
    /// The facing for a placement: clicked face (0..6) and look direction.
    /// A clicked face the block cannot face (a ladder on a floor) falls back to the player's
    /// look, front towards the player.
    pub fn choose(self, allowed: Facings, face: u32, look: [f32; 3]) -> Dir {
        if self == Toward::Face && allowed.allows(Dir::from_face(face)) {
            return Dir::from_face(face);
        }
        let back = [-look[0], -look[1], -look[2]];
        // Horizontal-only blocks go by the horizontal part of the look alone.
        if allowed.0 & Facings::VERTICAL.0 == 0 { Dir::nearest([back[0], 0.0, back[2]], allowed) } else { Dir::nearest(back, allowed) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_allowed_directions_make_states() {
        let f = Facings::only(&[Dir::Up, Dir::North, Dir::East]);
        assert_eq!(f.count(), 3);
        assert_eq!([Dir::East, Dir::Up, Dir::North].map(|d| f.index(d, 0)), [0, 1, 2]);
        assert_eq!(f.parse(4), (Dir::Up, 1));
        assert_eq!(Facings::HORIZONTAL.index(Dir::South, 1), 7);
        assert_eq!(Toward::Player.choose(Facings::HORIZONTAL, 3, [0.0, -0.9, -0.4]), Dir::South);
        assert_eq!(Toward::Face.choose(f, 3, [0.0, -1.0, 0.0]), Dir::Up);
    }
}
