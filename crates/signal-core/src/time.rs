//! Time: frames of the master clock and integer rate divisors (ADR 0003, 0005).

use std::num::NonZeroU32;
use std::ops::{Add, AddAssign, Sub};

use serde::Serialize;

/// A point in time: the number of frames since project start, at the project's
/// master rate (ADR 0003). This is the only time the core knows.
///
/// Offsets are plain `u64` frame counts: `Frame + u64`, `Frame - u64` and
/// `Frame - Frame` (the offset between two frames). The operators follow `u64`
/// arithmetic, so subtracting past frame 0 overflows; use [`Frame::checked_sub`]
/// where that can happen. Ordering is chronological.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Frame(pub u64);

impl Frame {
    /// The first frame of a project.
    pub const ZERO: Frame = Frame(0);

    /// `self + frames`, or `None` on overflow.
    pub const fn checked_add(self, frames: u64) -> Option<Frame> {
        match self.0.checked_add(frames) {
            Some(n) => Some(Frame(n)),
            None => None,
        }
    }

    /// `self - frames`, or `None` if that would be before frame 0.
    pub const fn checked_sub(self, frames: u64) -> Option<Frame> {
        match self.0.checked_sub(frames) {
            Some(n) => Some(Frame(n)),
            None => None,
        }
    }
}

impl Add<u64> for Frame {
    type Output = Frame;

    fn add(self, frames: u64) -> Frame {
        Frame(self.0 + frames)
    }
}

impl AddAssign<u64> for Frame {
    fn add_assign(&mut self, frames: u64) {
        self.0 += frames;
    }
}

impl Sub<u64> for Frame {
    type Output = Frame;

    fn sub(self, frames: u64) -> Frame {
        Frame(self.0 - frames)
    }
}

impl Sub<Frame> for Frame {
    type Output = u64;

    /// The number of frames from `earlier` to `self`.
    fn sub(self, earlier: Frame) -> u64 {
        self.0 - earlier.0
    }
}

/// A port's rate as a non-zero integer divisor of the master rate (ADR 0005):
/// a port with divisor *d* ticks on every *d*-th master frame, the frames
/// that are multiples of *d* (`docs/semantics.md` §16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RateDivisor(NonZeroU32);

impl RateDivisor {
    /// Divisor 1: the port runs at the master rate.
    pub const ONE: RateDivisor = RateDivisor(NonZeroU32::MIN);

    /// The divisor `d`, or `None` if `d` is 0.
    pub const fn new(d: u32) -> Option<RateDivisor> {
        match NonZeroU32::new(d) {
            Some(d) => Some(RateDivisor(d)),
            None => None,
        }
    }

    /// The divisor as an integer (always ≥ 1).
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl Default for RateDivisor {
    fn default() -> Self {
        RateDivisor::ONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_offsets_and_order() {
        let a = Frame(10);
        assert_eq!(a + 5, Frame(15));
        assert_eq!(a - 4, Frame(6));
        assert_eq!(Frame(15) - a, 5);
        let mut b = a;
        b += 3;
        assert_eq!(b, Frame(13));
        assert!(Frame(1) < Frame(2));
        assert_eq!(Frame::ZERO.checked_sub(1), None);
        assert_eq!(Frame(u64::MAX).checked_add(1), None);
        assert_eq!(Frame(3).checked_sub(3), Some(Frame::ZERO));
    }

    #[test]
    fn rate_divisor_is_non_zero() {
        assert_eq!(RateDivisor::new(0), None);
        assert_eq!(RateDivisor::new(4).map(RateDivisor::get), Some(4));
        assert_eq!(RateDivisor::ONE.get(), 1);
        assert_eq!(RateDivisor::default(), RateDivisor::ONE);
    }
}
