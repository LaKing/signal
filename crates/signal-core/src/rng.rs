//! The vendored, seeded random number generators (ADR 0008).
//!
//! SplitMix64 derives seeds; Xoshiro256++ produces streams. Both follow the
//! public-domain reference code by Blackman & Vigna (<https://prng.di.unimi.it/>).
//! The `rand` crate is not used: its value streams are not guaranteed stable.

use crate::address::InstanceAddress;

/// FNV-1a 64-bit hash of `bytes` (offset basis `0xcbf29ce484222325`, prime
/// `0x100000001b3`). A non-cryptographic content hash: used to derive RNG
/// streams from addresses, and by render manifests. Equal to feeding `bytes`
/// to [`Fnv1a64`] in one or several pieces.
pub const fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hasher = Fnv1a64::new();
    hasher.update(bytes);
    hasher.finish()
}

/// The FNV-1a 64-bit hash of [`fnv1a64`], fed incrementally, for content too
/// large to hold in one buffer (render audio). Non-cryptographic: it detects
/// accidental change, not tampering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fnv1a64 {
    hash: u64,
}

impl Fnv1a64 {
    /// A hasher that has seen no bytes (the offset basis).
    pub const fn new() -> Fnv1a64 {
        Fnv1a64 {
            hash: 0xcbf2_9ce4_8422_2325,
        }
    }

    /// Feeds `bytes`, continuing from everything fed before.
    pub const fn update(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let [byte, tail @ ..] = rest {
            self.hash ^= *byte as u64;
            self.hash = self.hash.wrapping_mul(0x0000_0100_0000_01b3);
            rest = tail;
        }
    }

    /// The hash of every byte fed so far.
    pub const fn finish(&self) -> u64 {
        self.hash
    }
}

impl Default for Fnv1a64 {
    fn default() -> Fnv1a64 {
        Fnv1a64::new()
    }
}

/// SplitMix64 (Steele, Lea & Flood; reference `splitmix64.c`). Used to expand
/// one 64-bit seed into the Xoshiro256++ state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// A generator whose state is `seed`.
    pub const fn new(seed: u64) -> SplitMix64 {
        SplitMix64 { state: seed }
    }

    /// The next output.
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Xoshiro256++ 1.0 (Blackman & Vigna; reference `xoshiro256plusplus.c`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Xoshiro256PlusPlus {
    s: [u64; 4],
}

impl Xoshiro256PlusPlus {
    /// A generator with the given state. The state must not be all zero: that
    /// is a fixed point which yields only zeros. [`RngStream::derive`] never
    /// produces it.
    pub const fn from_state(s: [u64; 4]) -> Xoshiro256PlusPlus {
        Xoshiro256PlusPlus { s }
    }

    /// The next output.
    pub const fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = self.s;
        let result = s0.wrapping_add(s3).rotate_left(23).wrapping_add(s0);
        let t = s1 << 17;
        let s2 = s2 ^ s0;
        let s3 = s3 ^ s1;
        let s1 = s1 ^ s2;
        let s0 = s0 ^ s3;
        self.s = [s0, s1, s2 ^ t, s3.rotate_left(45)];
        result
    }
}

/// An instance's random stream: Xoshiro256++ seeded from the master seed and
/// the instance address (ADR 0008), so every instance gets its own reproducible
/// stream and renaming an instance changes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RngStream {
    inner: Xoshiro256PlusPlus,
}

/// 2^-53, the spacing of the 53-bit values produced by [`RngStream::next_f64`].
const TWO_POW_NEG_53: f64 = 1.0 / (1u64 << 53) as f64;

impl RngStream {
    /// The stream of `address` under `master_seed`: Xoshiro256++ seeded with four
    /// successive SplitMix64 outputs, starting from
    /// `master_seed ^ fnv1a64(address.as_str().as_bytes())`.
    ///
    /// The state is never all zero: SplitMix64's output function is a bijection
    /// and its four successive states are distinct, so at most one output is 0.
    pub fn derive(master_seed: u64, address: &InstanceAddress) -> RngStream {
        let mut seeder = SplitMix64::new(master_seed ^ fnv1a64(address.as_str().as_bytes()));
        let s = [
            seeder.next_u64(),
            seeder.next_u64(),
            seeder.next_u64(),
            seeder.next_u64(),
        ];
        RngStream {
            inner: Xoshiro256PlusPlus::from_state(s),
        }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.inner.next_u64()
    }

    /// The next value in `[0, 1)`: `(x >> 11) as f64 * 2^-53`, exact on every platform.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * TWO_POW_NEG_53
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden vectors. All of them were produced by compiling the public-domain
    // reference sources from https://prng.di.unimi.it/ (`splitmix64.c`,
    // `xoshiro256plusplus.c`), a textbook FNV-1a 64 loop and a `derive` written on
    // top of them, as one C program built with Apple clang 17 on arm64, and copying
    // its output. They were not generated by this Rust code. Cross-checks against
    // published values: SplitMix64(0) → 0xE220A8397B1DCDAF (issue M1-1);
    // Xoshiro256++ from state [1, 2, 3, 4] → 41943041, 58720359, 3588806011781223, …
    // (the vector used by the `rand_xoshiro` crate's tests); FNV-1a 64 of "a" and
    // "foobar" (the FNV authors' test suite).

    const SPLITMIX64_SEED_0: [u64; 5] = [
        0xE220A8397B1DCDAF,
        0x6E789E6AA1B965F4,
        0x06C45D188009454F,
        0xF88BB8A8724C81EC,
        0x1B39896A51A8749B,
    ];

    const SPLITMIX64_SEED_0123456789ABCDEF: [u64; 5] = [
        0x157A3807A48FAA9D,
        0xD573529B34A1D093,
        0x2F90B72E996DCCBE,
        0xA2D419334C4667EC,
        0x01404CE914938008,
    ];

    const XOSHIRO_STATE_1234: [u64; 10] = [
        0x0000000002800001,
        0x0000000003800067,
        0x000CC00003800067,
        0x000CC201994400B2,
        0x8012A2019AC433CD,
        0x8A69978ACDEE33BA,
        0xC271134733154ABD,
        0xAC2BA09179169E97,
        0xDBF3190A8F073FD8,
        0x9105F14AB2229220,
    ];

    /// Xoshiro256++ seeded with the first four SplitMix64(0) outputs.
    const XOSHIRO_SPLITMIX_0: [u64; 5] = [
        0x53175D61490B23DF,
        0x61DA6F3DC380D507,
        0x5C0FDF91EC9A7BFC,
        0x02EEBF8C3BBE5E1A,
        0x7ECA04EBAF4A5EEA,
    ];

    const FNV1A64: [(&str, u64); 4] = [
        ("", 0xCBF29CE484222325),
        ("a", 0xAF63DC4C8601EC8C),
        ("foobar", 0x85944171F73967E8),
        ("/drums/kick/amp_env", 0xF773CE61B4A2EECF),
    ];

    /// `derive(42, /drums/kick/amp_env)`: `next_u64` outputs.
    const DERIVE_42_AMP_ENV_U64: [u64; 5] = [
        0x2359FC88CEE9E5C4,
        0x4131B655D1D0CF97,
        0x5D0AFE99DFD3BEDB,
        0x39796A704D15D721,
        0x32414393706386FB,
    ];

    /// `derive(42, /drums/kick/amp_env)`: `next_f64` outputs as bit patterns.
    const DERIVE_42_AMP_ENV_F64_BITS: [u64; 3] =
        [0x3FC1ACFE446774F0, 0x3FD04C6D95747432, 0x3FD742BFA677F4EE];

    fn addr(s: &str) -> InstanceAddress {
        s.parse().unwrap()
    }

    #[test]
    fn splitmix64_golden() {
        let mut g = SplitMix64::new(0);
        assert_eq!(g.next_u64(), 0xE220A8397B1DCDAF);
        let mut g = SplitMix64::new(0);
        assert_eq!(SPLITMIX64_SEED_0.map(|_| g.next_u64()), SPLITMIX64_SEED_0);
        let mut g = SplitMix64::new(0x0123456789ABCDEF);
        assert_eq!(
            SPLITMIX64_SEED_0123456789ABCDEF.map(|_| g.next_u64()),
            SPLITMIX64_SEED_0123456789ABCDEF
        );
    }

    #[test]
    fn xoshiro256plusplus_golden() {
        let mut g = Xoshiro256PlusPlus::from_state([1, 2, 3, 4]);
        assert_eq!(XOSHIRO_STATE_1234.map(|_| g.next_u64()), XOSHIRO_STATE_1234);

        let mut seeder = SplitMix64::new(0);
        let state = [(); 4].map(|_| seeder.next_u64());
        let mut g = Xoshiro256PlusPlus::from_state(state);
        assert_eq!(XOSHIRO_SPLITMIX_0.map(|_| g.next_u64()), XOSHIRO_SPLITMIX_0);
    }

    #[test]
    fn fnv1a64_golden() {
        for (text, hash) in FNV1A64 {
            assert_eq!(fnv1a64(text.as_bytes()), hash, "fnv1a64({text:?})");
        }
    }

    #[test]
    fn fnv1a64_fed_in_pieces_equals_one_shot() {
        for (text, hash) in FNV1A64 {
            for split in 0..=text.len() {
                let (head, tail) = text.as_bytes().split_at(split);
                let mut hasher = Fnv1a64::new();
                hasher.update(head);
                hasher.update(tail);
                assert_eq!(hasher.finish(), hash, "{text:?} split at {split}");
            }
        }
        assert_eq!(Fnv1a64::default().finish(), fnv1a64(b""));
    }

    #[test]
    fn derive_golden() {
        let a = addr("/drums/kick/amp_env");
        let mut s = RngStream::derive(42, &a);
        assert_eq!(
            DERIVE_42_AMP_ENV_U64.map(|_| s.next_u64()),
            DERIVE_42_AMP_ENV_U64
        );
        let mut s = RngStream::derive(42, &a);
        assert_eq!(
            DERIVE_42_AMP_ENV_F64_BITS.map(|_| s.next_f64().to_bits()),
            DERIVE_42_AMP_ENV_F64_BITS
        );
    }

    #[test]
    fn derive_is_per_address_and_reproducible() {
        let take = |seed, a: &str| {
            let mut s = RngStream::derive(seed, &addr(a));
            [(); 8].map(|_| s.next_u64())
        };
        assert_eq!(take(7, "/a/b"), take(7, "/a/b"));
        assert_ne!(take(7, "/a/b"), take(7, "/a/c"));
        assert_ne!(take(7, "/a/b"), take(7, "/ab"));
        assert_ne!(take(7, "/a/b"), take(8, "/a/b"));
    }

    #[test]
    fn next_f64_is_in_unit_interval() {
        let mut s = RngStream::derive(1, &addr("/noise"));
        for _ in 0..10_000 {
            let x = s.next_f64();
            assert!((0.0..1.0).contains(&x));
        }
        // The largest possible output is 1 - 2^-53, still below 1.
        assert!(((u64::MAX >> 11) as f64 * TWO_POW_NEG_53) < 1.0);
    }
}
