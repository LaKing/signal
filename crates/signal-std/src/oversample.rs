//! Oversampled `tanh`: the resampling of `std.tanh_shaper_os8`
//! ([`TanhShaperOs8`](crate::TanhShaperOs8)), promoted from the M3
//! "ADAA versus oversampling" experiment (`experiments/m3-tanh-aliasing/`,
//! verdict 2026-10-23). It is public so the experiment's other `lab.*`
//! candidates (×2, ×4, ×8 without ADAA) are built from the same filters.
//!
//! `u = drive·in` is computed at the frame rate and raised to *R* = 2^*S*
//! times the rate by *S* cascaded ×2 stages, shaped there, and brought back
//! by the same stages in reverse. Every stage is a linear-phase FIR
//! (Kaiser-windowed sinc), used as a polyphase interpolator (gain 2) on the
//! way up and as a decimator on the way down:
//!
//! - stage 1 (sr ↔ 2·sr), [`STAGE1`]: passband to 0.40·sr, stopband from
//!   0.50·sr, so nothing above the frame rate's Nyquist frequency folds back
//!   into it; this stage sets the passband (19.2 kHz at 48 kHz, 38.4 kHz at
//!   96 kHz) and most of the delay;
//! - stages 2 and 3 (2·sr ↔ 4·sr, 4·sr ↔ 8·sr), [`STAGE2`], [`STAGE3`]:
//!   half-band filters (every other tap 0), stopband from the band whose
//!   content would fold onto 0 … 0.5·sr at the next rate down.
//!
//! The coefficients are designed when an [`Oversampled`] is made (in
//! `prepare`), in normalized frequency, so they are the same at every sample
//! rate ([`design`]). The work per frame is fixed: no branch depends on the
//! signal except first-order ADAA's ill-conditioning switch.

use libm::{sin, sqrt, tanh};

use crate::tanh_shaper::adaa;

/// The longest filter, in taps.
pub const MAX_TAPS: usize = 160;

/// A ×2 stage's linear-phase low-pass FIR, as designed by [`design`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageSpec {
    /// Taps *N* (odd); its delay is (*N* − 1)/2 samples at the stage's rate.
    pub taps: usize,
    /// Cut-off (−6 dB) in cycles per sample at the stage's higher rate.
    pub cutoff: f64,
    /// The Kaiser window's β.
    pub beta: f64,
    /// Half-band: the taps at even offsets from the centre are set to 0.
    pub half_band: bool,
}

/// Stage 1 (sr ↔ 2·sr): passband 0 … 0.40·sr, stopband 0.50·sr … sr, i.e.
/// 0.20 and 0.25 cycles per sample at 2·sr. β = 9.0 gives about 90 dB of
/// stopband (Kaiser's formula A ≈ β/0.1102 + 8.7); *N* = 131 meets that
/// transition width of 0.05 (Kaiser: *N* − 1 ≥ (A − 7.95)/(14.36·Δf)).
pub const STAGE1: StageSpec = StageSpec {
    taps: 131,
    cutoff: 0.225,
    beta: 9.0,
    half_band: false,
};

/// Stage 2 (2·sr ↔ 4·sr): half-band, passband to 0.5·sr (0.125 cycles per
/// sample at 4·sr), stopband from 1.5·sr (0.375), whose content would fold
/// onto 0 … 0.5·sr at 2·sr. *N* = 27.
pub const STAGE2: StageSpec = StageSpec {
    taps: 27,
    cutoff: 0.25,
    beta: 9.0,
    half_band: true,
};

/// Stage 3 (4·sr ↔ 8·sr): half-band, passband to 0.5·sr (0.0625 cycles per
/// sample at 8·sr), stopband from 3.5·sr (0.4375). *N* = 19.
pub const STAGE3: StageSpec = StageSpec {
    taps: 19,
    cutoff: 0.25,
    beta: 9.0,
    half_band: true,
};

/// The stages, in order from the frame rate up.
pub const STAGES: [StageSpec; 3] = [STAGE1, STAGE2, STAGE3];

/// The delay of an oversampled shaper in frames: stage *s* (1-based) delays
/// (*N*ₛ − 1)/2 samples at 2^*s*·sr each way, and first-order ADAA half a
/// sample at 2^*S*·sr. `latency_frames(3, true)` = 73.8125 is
/// `std.tanh_shaper_os8`'s.
pub fn latency_frames(stages: usize, adaa: bool) -> f64 {
    let stages = stages.min(STAGES.len());
    // Up and down: (N − 1)/2 samples each, at 2^s times the frame rate.
    let filters: f64 = STAGES
        .iter()
        .take(stages)
        .zip([2.0, 4.0, 8.0])
        .map(|(spec, rate)| (spec.taps - 1) as f64 / rate)
        .sum();
    let half_sample = if adaa {
        0.5 / f64::from(1_u32 << stages)
    } else {
        0.0
    };
    filters + half_sample
}

/// The zeroth-order modified Bessel function I₀, by its series (fixed 40
/// terms: enough for x ≤ β = 9 to f64 precision).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..=40_u32 {
        let k = f64::from(k);
        term *= q / (k * k);
        sum += term;
    }
    sum
}

/// Writes `spec`'s taps into `taps` (its first `spec.taps` entries; the rest
/// 0): h\[n\] = 2·f_c·sinc(2·f_c·(n − M))·w\[n\], M = (N − 1)/2, w the Kaiser
/// window of β, scaled so Σ h = 1 (unit gain at DC). Pure arithmetic with
/// `libm`: the same coefficients on every platform and at every rate.
pub fn design(spec: StageSpec, taps: &mut [f64; MAX_TAPS]) {
    *taps = [0.0; MAX_TAPS];
    let n = spec.taps.min(MAX_TAPS);
    let m = (n as f64 - 1.0) / 2.0;
    let i0_beta = bessel_i0(spec.beta);
    let mut sum = 0.0;
    for (i, h) in taps.iter_mut().take(n).enumerate() {
        let offset = i as f64 - m;
        let x = 2.0 * spec.cutoff * offset;
        let sinc = if offset == 0.0 {
            1.0
        } else {
            sin(core::f64::consts::PI * x) / (core::f64::consts::PI * x)
        };
        let r = if m > 0.0 { offset / m } else { 0.0 };
        let window = bessel_i0(spec.beta * sqrt((1.0 - r * r).max(0.0))) / i0_beta;
        let even_offset = offset != 0.0 && (offset as i64) % 2 == 0;
        *h = if spec.half_band && even_offset {
            0.0
        } else {
            2.0 * spec.cutoff * sinc * window
        };
        sum += *h;
    }
    for h in taps.iter_mut().take(n) {
        *h /= sum;
    }
}

/// A delay line of up to [`MAX_TAPS`] values, newest first: written twice,
/// so the window of its last `len` values is always one contiguous slice.
#[derive(Clone, Debug)]
struct Line {
    buffer: [f64; 2 * MAX_TAPS],
    len: usize,
    position: usize,
}

impl Line {
    fn new(len: usize) -> Line {
        Line {
            buffer: [0.0; 2 * MAX_TAPS],
            len: len.clamp(1, MAX_TAPS),
            position: 0,
        }
    }

    fn clear(&mut self) {
        self.buffer = [0.0; 2 * MAX_TAPS];
        self.position = 0;
    }

    fn push(&mut self, x: f64) {
        self.position = if self.position == 0 {
            self.len - 1
        } else {
            self.position - 1
        };
        for at in [self.position, self.position + self.len] {
            if let Some(slot) = self.buffer.get_mut(at) {
                *slot = x;
            }
        }
    }

    /// The last `len` values, newest first.
    fn window(&self) -> &[f64] {
        self.buffer
            .get(self.position..self.position + self.len)
            .unwrap_or(&[])
    }
}

/// Σ aᵢ·bᵢ over the shorter of the two, in four interleaved partial sums
/// (a fixed order, so the same bits in every run and partition).
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    let (a, b) = (a.get(..n).unwrap_or(&[]), b.get(..n).unwrap_or(&[]));
    let (a4, a_rest) = a.as_chunks::<4>();
    let (b4, b_rest) = b.as_chunks::<4>();
    let mut sums = [0.0; 4];
    for (x, y) in a4.iter().zip(b4) {
        for ((s, x), y) in sums.iter_mut().zip(x).zip(y) {
            *s += x * y;
        }
    }
    let rest: f64 = a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum();
    let [s0, s1, s2, s3] = sums;
    ((s0 + s1) + (s2 + s3)) + rest
}

/// One polyphase branch of a filter: the taps h\[2j + p\] (j = 0, 1, …),
/// trimmed to those from the first to the last that is not 0 (a half-band
/// filter's odd branch is one tap).
#[derive(Clone, Debug)]
struct Branch {
    taps: [f64; MAX_TAPS / 2 + 1],
    /// The j of the first kept tap.
    start: usize,
    len: usize,
}

impl Branch {
    /// Allocates a temporary: `prepare` only.
    fn new(h: &[f64], p: usize) -> Branch {
        let all: Vec<f64> = h.iter().skip(p).step_by(2).copied().collect();
        let start = all.iter().position(|h| *h != 0.0).unwrap_or(0);
        let end = all.iter().rposition(|h| *h != 0.0).map_or(start, |e| e + 1);
        let mut taps = [0.0; MAX_TAPS / 2 + 1];
        for (t, h) in taps.iter_mut().zip(all.get(start..end).unwrap_or(&[])) {
            *t = *h;
        }
        Branch {
            taps,
            start,
            len: end - start,
        }
    }

    /// Σⱼ h\[2j + p\]·line\[j\], `line` newest first.
    fn apply(&self, line: &Line) -> f64 {
        let window = line.window();
        let window = window.get(self.start..).unwrap_or(&[]);
        dot(self.taps.get(..self.len).unwrap_or(&[]), window)
    }
}

/// One ×2 stage: an interpolator (low rate → high) and a decimator (high →
/// low) with the same taps, both polyphase: every product is computed at the
/// low rate, and a tap that is 0 is never multiplied.
#[derive(Clone, Debug)]
struct Stage {
    /// h\[2j\], h\[2j + 1\].
    branches: [Branch; 2],
    /// The interpolator's input x\[m\], newest first: ⌈N/2⌉ values.
    up: Line,
    /// The decimator's even input v\[2m\], v\[2m − 2\], …
    down_even: Line,
    /// The decimator's odd input v\[2m − 1\], v\[2m − 3\], …
    down_odd: Line,
}

impl Stage {
    /// Allocates (a temporary while splitting the taps): `prepare` only.
    fn new(spec: StageSpec) -> Stage {
        let mut taps = [0.0; MAX_TAPS];
        design(spec, &mut taps);
        let n = spec.taps.min(MAX_TAPS);
        let h = taps.get(..n).unwrap_or(&[]);
        let half = n.div_ceil(2);
        Stage {
            branches: [Branch::new(h, 0), Branch::new(h, 1)],
            up: Line::new(half),
            down_even: Line::new(half),
            down_odd: Line::new(half),
        }
    }

    fn clear(&mut self) {
        self.up.clear();
        self.down_even.clear();
        self.down_odd.clear();
    }

    /// The two high-rate samples v\[2m\], v\[2m + 1\] for low-rate x\[m\]:
    /// v\[2m + p\] = 2·Σⱼ h\[2j + p\]·x\[m − j\].
    fn interpolate(&mut self, x: f64) -> [f64; 2] {
        self.up.push(x);
        let [even, odd] = &self.branches;
        [2.0 * even.apply(&self.up), 2.0 * odd.apply(&self.up)]
    }

    /// The low-rate y\[m\] = Σᵢ h\[i\]·v\[2m − i\] for the high-rate pair v\[2m\],
    /// v\[2m + 1\] (v\[2m + 1\] is kept for the next frame):
    /// Σⱼ h\[2j\]·v\[2m − 2j\] + Σⱼ h\[2j + 1\]·v\[2m − 1 − 2j\].
    fn decimate(&mut self, pair: [f64; 2]) -> f64 {
        let [v_even, v_odd] = pair;
        self.down_even.push(v_even);
        let [even, odd] = &self.branches;
        let y = even.apply(&self.down_even) + odd.apply(&self.down_odd);
        self.down_odd.push(v_odd);
        y
    }
}

/// The highest oversampling factor.
const MAX_FACTOR: usize = 8;

/// `tanh` of `u` raised to 2^`stages` times the rate, shaped by plain
/// `tanh` or first-order ADAA there (`std.tanh_shaper`'s formula), and
/// brought back.
#[derive(Clone, Debug)]
pub struct Oversampled {
    stages: [Stage; 3],
    active: usize,
    adaa: bool,
    /// The last shaped input at the high rate (first-order ADAA's u\[n−1\]).
    previous: f64,
}

impl Oversampled {
    /// 2^`stages` times oversampling (1 ≤ `stages` ≤ 3), with first-order
    /// ADAA at the high rate if `adaa`. Allocates while it designs the
    /// filters: call it in `prepare` only.
    pub fn new(stages: usize, adaa: bool) -> Oversampled {
        Oversampled {
            stages: STAGES.map(Stage::new),
            active: stages.clamp(1, STAGES.len()),
            adaa,
            previous: 0.0,
        }
    }

    /// Back to silence.
    pub fn clear(&mut self) {
        for stage in &mut self.stages {
            stage.clear();
        }
        self.previous = 0.0;
    }

    /// One frame: the shaped, delayed output for `u` = drive·in. Fixed work.
    pub fn tick(&mut self, u: f64) -> f64 {
        let mut samples = [0.0; MAX_FACTOR];
        let mut count = 1;
        if let Some(first) = samples.first_mut() {
            *first = u;
        }
        let active = self.active;
        for stage in self.stages.iter_mut().take(active) {
            let mut next = [0.0; MAX_FACTOR];
            for (x, pair) in samples
                .iter()
                .take(count)
                .zip(next.as_chunks_mut::<2>().0.iter_mut())
            {
                *pair = stage.interpolate(*x);
            }
            samples = next;
            count *= 2;
        }
        for v in samples.iter_mut().take(count) {
            let u = *v;
            *v = if self.adaa {
                adaa(u, self.previous)
            } else {
                tanh(u)
            };
            self.previous = u;
        }
        for stage in self.stages.iter_mut().take(active).rev() {
            let mut next = [0.0; MAX_FACTOR];
            for (y, pair) in next
                .iter_mut()
                .zip(samples.as_chunks::<2>().0.iter().take(count / 2))
            {
                *y = stage.decimate(*pair);
            }
            samples = next;
            count /= 2;
        }
        samples.first().copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_delays_match_the_filters() {
        assert_eq!(latency_frames(1, true), 65.25);
        assert_eq!(latency_frames(2, true), 71.625);
        assert_eq!(latency_frames(3, true), 73.8125);
        assert_eq!(latency_frames(3, false), 73.75);
    }

    #[test]
    fn the_filters_have_unit_dc_gain_and_are_symmetric() {
        for spec in STAGES {
            let mut taps = [0.0; MAX_TAPS];
            design(spec, &mut taps);
            let n = spec.taps;
            assert!((taps[..n].iter().sum::<f64>() - 1.0).abs() < 1e-14);
            for i in 0..n {
                assert_eq!(taps[i], taps[n - 1 - i]);
            }
            assert!(taps[n..].iter().all(|h| *h == 0.0));
        }
    }

    /// At a low drive the shaper is nearly linear: a slow sine comes out
    /// with the documented delay.
    #[test]
    fn a_slow_sine_comes_out_delayed_by_the_documented_frames() {
        for (stages, adaa) in [(1, true), (2, true), (3, true), (3, false)] {
            let mut shaper = Oversampled::new(stages, adaa);
            let delay = latency_frames(stages, adaa);
            let period = 400.0;
            let amplitude = 0.01;
            let mut worst: f64 = 0.0;
            for n in 0..4_000 {
                let t = n as f64;
                let y = shaper.tick(amplitude * libm::sin(core::f64::consts::TAU * t / period));
                if n > 500 {
                    let expected =
                        tanh(amplitude * libm::sin(core::f64::consts::TAU * (t - delay) / period));
                    worst = worst.max((y - expected).abs());
                }
            }
            assert!(worst < 1e-4 * amplitude, "{stages} {adaa}: {worst}");
        }
    }
}
