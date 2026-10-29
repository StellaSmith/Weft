//! Hashing, a small random stream and the world's noise. The noise is 32-bit integers only
//! (`fixed`), so every platform and the GPU get the same bits. The engine's terrain and mods
//! share it, so a feature written in a mod sees the same fields the engine does.

#[inline]
pub fn hash(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    let mut h = seed ^ (x as u64).wrapping_mul(0x9E3779B97F4A7C15);
    h ^= (y as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
    h ^= (z as u64).wrapping_mul(0x165667B19E3779F9);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D049BB133111EB);
    h ^ (h >> 31)
}

/// SplitMix64's finalizer: a well mixed value from any input.
#[inline]
pub fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// A small deterministic random stream (SplitMix64); the engine's only source of randomness
/// besides `hash` of positions.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    #[inline]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        mix(self.0)
    }

    /// Uniform in [0, 1).
    pub fn float(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in 0..n (0 for n = 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

/// Where an FNV-1a hash starts.
pub const FNV: u64 = 0xcbf2_9ce4_8422_2325;

/// One more value into an FNV-1a hash (whole values hashed as one step, for fingerprints).
#[inline]
pub const fn fnv_step(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x100_0000_01b3)
}

/// FNV-1a of `bytes`: stable names to salts and fingerprints.
pub const fn fnv(bytes: &[u8]) -> u64 {
    let mut h = FNV;
    let mut i = 0;
    while i < bytes.len() {
        h = fnv_step(h, bytes[i] as u64);
        i += 1;
    }
    h
}

/// Uniform in [0, 1).
#[inline]
pub fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

pub mod fixed;
