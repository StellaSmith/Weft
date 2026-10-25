pub mod cube;
pub mod light;

use glam::IVec3;

pub use cube::Cube;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum GameMode {
    #[default]
    Survival,
    Creative,
    /// Flies through blocks, cannot touch anything, unseen by mobs and survival players.
    Spectator,
}

impl GameMode {
    pub const ALL: [GameMode; 3] = [GameMode::Survival, GameMode::Creative, GameMode::Spectator];

    pub fn name(self) -> &'static str {
        match self {
            GameMode::Survival => "survival",
            GameMode::Creative => "creative",
            GameMode::Spectator => "spectator",
        }
    }

    pub fn parse(s: &str) -> Option<GameMode> {
        GameMode::ALL.into_iter().find(|m| m.name() == s)
    }

    pub fn from_u8(v: u8) -> GameMode {
        GameMode::ALL.get(v as usize).copied().unwrap_or_default()
    }

    /// Takes no damage and needs no food.
    pub fn invulnerable(self) -> bool {
        self != GameMode::Survival
    }
}

#[inline]
pub fn cube_of(p: IVec3) -> IVec3 {
    p >> 4
}

#[inline]
pub fn local_index(p: IVec3) -> usize {
    cube::index((p.x & 15) as usize, (p.y & 15) as usize, (p.z & 15) as usize)
}

pub const DIRS: [IVec3; 6] = [IVec3::NEG_X, IVec3::X, IVec3::NEG_Y, IVec3::Y, IVec3::NEG_Z, IVec3::Z];

/// SplitMix64, used wherever we need cheap deterministic randomness.
#[inline]
pub fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

pub fn hash_pos(seed: u64, p: IVec3) -> u64 {
    mix(seed ^ mix((p.x as u32 as u64) | ((p.z as u32 as u64) << 32)) ^ mix(p.y as u32 as u64 ^ 0xA5A5))
}

#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    #[inline]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        mix(self.0)
    }

    pub fn float(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}
