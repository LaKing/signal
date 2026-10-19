//! The circular history and the Hermite read shared by the M2.10 delay-based
//! primitives (`std.comb`, `std.allpass_delay`, `std.fdn_reverb`). It is the
//! read of `std.delay` (`crate::delay`), written once more here so that
//! `std.delay` and its golden renders stay untouched.

/// Frames a line holds beyond its longest delay, for the interpolator.
pub(crate) const GUARD: usize = 4;

/// Internal state (a recirculating delay line, a reverb's lines) is clamped
/// to this magnitude. It never acts on a stable loop with an input within
/// the engine's bound: it only keeps the state finite whatever happens, as
/// under audio-rate modulation, where the fixed-setting loop-gain argument
/// does not apply.
pub(crate) const STATE_BOUND: f64 = 1e12;

/// A circular history of samples, allocated in `prepare`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Line {
    /// The history; `buffer[newest]` is the latest pushed sample.
    buffer: Vec<f64>,
    newest: usize,
}

impl Line {
    /// Allocates room for a delay of up to `frames` frames and writes every
    /// element (the prefault, as `std.buffer*`'s): an opaque zero the
    /// optimiser cannot turn into a lazily zeroed allocation, so the pages
    /// are touched here, off the audio thread. Call in `prepare` only.
    pub(crate) fn allocate(&mut self, frames: usize) {
        let zero = std::hint::black_box(0.0);
        self.buffer.clear();
        self.buffer.resize(frames + GUARD, zero);
        self.newest = 0;
    }

    /// Zeroes the history (one pass over the buffer).
    pub(crate) fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.newest = 0;
    }

    /// Appends `x` as the newest sample.
    pub(crate) fn push(&mut self, x: f64) {
        let len = self.buffer.len().max(1);
        self.newest = (self.newest + 1) % len;
        if let Some(slot) = self.buffer.get_mut(self.newest) {
            *slot = x;
        }
    }

    /// The sample `back` frames before the newest one (0: the newest),
    /// `back` limited to the buffer.
    pub(crate) fn sample(&self, back: usize) -> f64 {
        let len = self.buffer.len();
        let back = back.min(len.saturating_sub(1));
        let index = (self.newest + len - back) % len.max(1);
        self.buffer.get(index).copied().unwrap_or(0.0)
    }

    /// The history `back` frames before the newest sample, `back ≥ 0`,
    /// read with `std.delay`'s 4-point, 3rd-order Hermite interpolator:
    /// exact at a whole `back`; for `0 < back < 1` the point newer than the
    /// newest sample is the newest sample (`std.delay`'s causal fallback).
    pub(crate) fn read(&self, back: f64) -> f64 {
        let whole = libm::floor(back);
        let frac = back - whole;
        let (offset, t) = if frac == 0.0 {
            (whole as usize, 0.0)
        } else {
            (whole as usize + 1, 1.0 - frac)
        };
        let ym1 = self.sample(offset + 1);
        let y0 = self.sample(offset);
        let y1 = self.sample(offset.saturating_sub(1));
        let y2 = self.sample(offset.saturating_sub(2));
        hermite(ym1, y0, y1, y2, t)
    }
}

impl Line {
    /// [`Line::read`] limited to the two samples it interpolates between:
    /// the Hermite value clamped to `[min(y₀, y₁), max(y₀, y₁)]` (at a whole
    /// `back`, exactly `y₀`). So `|read_bounded(back)|` never exceeds the
    /// largest magnitude in the history, at any delay and under any
    /// modulation of it: the read the feedback loops of `std.comb` and
    /// `std.allpass_delay` use, which makes them contractive. (The plain
    /// Hermite read overshoots by up to its weights' ℓ1 norm, 1.25, on a
    /// single frame, so a modulated delay could pump a loop of gain 0.999
    /// past 1.)
    pub(crate) fn read_bounded(&self, back: f64) -> f64 {
        let whole = libm::floor(back);
        let frac = back - whole;
        if frac == 0.0 {
            return self.sample(whole as usize);
        }
        let offset = whole as usize + 1;
        let ym1 = self.sample(offset + 1);
        let y0 = self.sample(offset);
        let y1 = self.sample(offset.saturating_sub(1));
        let y2 = self.sample(offset.saturating_sub(2));
        hermite(ym1, y0, y1, y2, 1.0 - frac).clamp(y0.min(y1), y0.max(y1))
    }
}

/// The Catmull-Rom (4-point, 3rd-order Hermite) interpolation between `y0`
/// (t = 0) and `y1` (t = 1).
pub(crate) fn hermite(ym1: f64, y0: f64, y1: f64, y2: f64, t: f64) -> f64 {
    let c1 = 0.5 * (y1 - ym1);
    let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
    let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
    ((c3 * t + c2) * t + c1) * t + y0
}

/// `x` if finite, else 0: a non-finite input (possible only in a direct
/// call; through the engine every input is finite, `docs/semantics.md` §7)
/// cannot poison a recursive state.
pub(crate) fn finite_or_zero(x: f64) -> f64 {
    if x.is_finite() { x } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loop-gain argument of `std.comb`, `std.allpass_delay` and
    /// `std.fdn_reverb` rests on this: at every fractional position `t` and
    /// every frequency, the interpolator's magnitude response is at most 1,
    /// for the 4-point Hermite weights and for the causal fallback (the
    /// newest point's weight folded into the one before).
    #[test]
    fn the_interpolator_never_amplifies() {
        let mut worst = 0.0f64;
        for ti in 0..=200 {
            let t = ti as f64 / 200.0;
            let (t2, t3) = (t * t, t * t * t);
            let w = [
                (-t3 + 2.0 * t2 - t) / 2.0,
                (3.0 * t3 - 5.0 * t2 + 2.0) / 2.0,
                (-3.0 * t3 + 4.0 * t2 + t) / 2.0,
                (t3 - t2) / 2.0,
            ];
            let fallback = [w[0], w[1], w[2] + w[3], 0.0];
            for weights in [w, fallback] {
                for wi in 0..=400 {
                    let omega = std::f64::consts::PI * wi as f64 / 400.0;
                    let (mut re, mut im) = (0.0, 0.0);
                    for (k, c) in weights.iter().enumerate() {
                        re += c * libm::cos(omega * k as f64);
                        im -= c * libm::sin(omega * k as f64);
                    }
                    worst = worst.max(libm::sqrt(re * re + im * im));
                }
            }
        }
        assert!(worst <= 1.0 + 1e-12, "peak gain {worst}");
    }

    /// The bounded read stays between its two neighbours where Hermite
    /// overshoots: next to a step it would give 1.0625 times the step.
    #[test]
    fn the_bounded_read_never_overshoots() {
        let mut line = Line::default();
        line.allocate(16);
        for x in [0.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
            line.push(x);
        }
        // Between the last 0 and the first 1, and between 1s next to the step.
        assert!(line.read(1.5) > 1.0);
        assert_eq!(line.read_bounded(1.5), 1.0);
        for back in [0.25, 0.5, 1.25, 1.5, 2.5, 2.75, 3.5] {
            let v = line.read_bounded(back);
            assert!((0.0..=1.0).contains(&v), "{back}: {v}");
        }
    }

    #[test]
    fn whole_reads_are_exact_and_fractional_reads_interpolate() {
        let mut line = Line::default();
        line.allocate(16);
        for x in 1..=10 {
            line.push(x as f64);
        }
        assert_eq!(line.read(0.0), 10.0);
        assert_eq!(line.read(3.0), 7.0);
        // A ramp is reproduced exactly by Catmull-Rom.
        assert_eq!(line.read(3.5), 6.5);
        assert_eq!(line.read_bounded(3.5), 6.5);
        assert_eq!(line.read_bounded(3.0), 7.0);
        line.clear();
        assert_eq!(line.read(2.0), 0.0);
    }
}
