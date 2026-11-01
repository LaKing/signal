//! Shared by the clock adapter's tests (ADR 0052 phase 1): the fake-clock
//! harness ([`harness`]) and the converter measurements ([`spectrum`]).

#![allow(dead_code)]

pub mod harness;
pub mod spectrum;

/// A seeded generator (SplitMix64): the harness's jitter.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator from `seed`.
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [−1, 1).
    pub fn symmetric(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}
