//! The converter's filter (ADR 0052 §4): a Kaiser-windowed sinc, tabulated
//! at [`PHASES`] fractional positions per source frame, with linear
//! interpolation between neighbouring phases. Designed in `f64` and stored
//! in `f32` when a route is made ([`SincTable::new`], control thread); the
//! audio thread only reads the table ([`SincTable::coefficients`]).

use std::f64::consts::PI;

/// Phases per source frame (ADR 0052 §4).
pub const PHASES: usize = 512;

/// Bits of a 32-bit fraction that select the phase (`PHASES` = 2⁹).
const PHASE_BITS: u32 = 9;

/// The most taps a route may use (ADR 0052 §4).
pub const MAX_TAPS: usize = 256;

/// The passband edge when the lower rate allows it: 20 kHz.
const PASSBAND_HZ: f64 = 20_000.0;

/// The stopband attenuation the tap count is chosen for, in dB. 105 dB
/// gives the ADR's fixed counts: 96 taps at equal rates (transition
/// 20–24 kHz at 48 kHz), 160 taps between 44.1 and 48 kHz (20–22.05 kHz).
const DESIGN_ATTENUATION_DB: f64 = 105.0;

/// The window is never designed for more than this (an `f32` table's
/// coefficients carry about 150 dB).
const MAX_ATTENUATION_DB: f64 = 140.0;

/// What a converter's filter is: its length, cutoff and window, all in
/// source frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterSpec {
    /// Taps (source frames under the filter), even, at most [`MAX_TAPS`].
    pub taps: usize,
    /// The −6 dB cutoff in cycles per source frame (0 < cutoff < 0.5).
    pub cutoff: f64,
    /// The Kaiser window's β.
    pub beta: f64,
    /// The passband edge in cycles per source frame.
    pub passband: f64,
    /// The stopband edge in cycles per source frame.
    pub stopband: f64,
    /// The stopband attenuation Kaiser's formula predicts for this length
    /// and transition, in dB (an estimate; `docs/benchmarks.md` has the
    /// measurement).
    pub predicted_attenuation_db: f64,
}

impl FilterSpec {
    /// The filter for converting `source_rate` to `dest_rate` (ADR 0052 §4):
    /// passband to 20 kHz (or 20/44.1 of the lower rate, if that is less),
    /// stopband from the lower rate's Nyquist frequency, so nothing beyond
    /// it aliases or images into the output. The taps are the fewest
    /// multiple of 16 that reach [`DESIGN_ATTENUATION_DB`] by Kaiser's
    /// formula, at least 96 at equal nominal rates and 160 otherwise, at
    /// most [`MAX_TAPS`].
    pub fn for_rates(source_rate: u32, dest_rate: u32) -> FilterSpec {
        let source = f64::from(source_rate.max(1));
        let low = f64::from(source_rate.min(dest_rate).max(1));
        let pass_hz = PASSBAND_HZ.min(low * PASSBAND_HZ / 44_100.0);
        let stop_hz = low / 2.0;
        let passband = pass_hz / source;
        let stopband = stop_hz / source;
        let transition = 2.0 * PI * (stopband - passband);
        let least = if source_rate == dest_rate { 96 } else { 160 };
        let needed = ((DESIGN_ATTENUATION_DB - 8.0) / (2.285 * transition)).ceil() as usize + 1;
        let taps = needed
            .div_ceil(16)
            .saturating_mul(16)
            .clamp(least, MAX_TAPS);
        let predicted = (8.0 + 2.285 * transition * (taps as f64 - 1.0)).min(MAX_ATTENUATION_DB);
        FilterSpec {
            taps,
            cutoff: (passband + stopband) / 2.0,
            beta: kaiser_beta(predicted),
            passband,
            stopband,
            predicted_attenuation_db: predicted,
        }
    }

    /// Whether its taps reach the design attenuation (105 dB by Kaiser's
    /// formula): not when [`MAX_TAPS`] cut them short, which a high
    /// source rate over a low destination does (192 → 44.1 kHz needs
    /// about 630); [`Decimator`](super::Decimator) halves the rate first.
    pub fn reaches_design(&self) -> bool {
        self.predicted_attenuation_db >= DESIGN_ATTENUATION_DB
    }

    /// Half the taps: how far ahead of the read position the converter
    /// reads, in source frames. Being symmetric about the read position, the
    /// filter's group delay is this lookahead.
    pub fn half(&self) -> usize {
        self.taps / 2
    }

    /// The filter's continuous impulse response at `t` source frames from
    /// its centre (0 outside ±taps/2), in `f64`: the reference the table
    /// approximates.
    pub fn response(&self, t: f64) -> f64 {
        let half = self.taps as f64 / 2.0;
        if t.abs() > half {
            return 0.0;
        }
        let x = 2.0 * self.cutoff * t;
        let sinc = if x.abs() < 1e-12 {
            1.0
        } else {
            (PI * x).sin() / (PI * x)
        };
        let r = t / half;
        let window = bessel_i0(self.beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(self.beta);
        2.0 * self.cutoff * sinc * window
    }
}

/// Kaiser's β for a stopband attenuation of `a` dB.
pub(crate) fn kaiser_beta(a: f64) -> f64 {
    if a > 50.0 {
        0.1102 * (a - 8.7)
    } else if a >= 21.0 {
        0.5842 * (a - 21.0).powf(0.4) + 0.07886 * (a - 21.0)
    } else {
        0.0
    }
}

/// The modified Bessel function of the first kind, order 0 (power series).
pub(crate) fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1.0;
    while term > sum * 1e-21 && k < 500.0 {
        term *= quarter / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// The tabulated filter: for each of `PHASES + 1` fractions `p / PHASES` of
/// a source frame, the `taps` coefficients, each row normalised to a sum of
/// 1 (unity gain at DC at every phase), and the difference to the next row
/// for the linear interpolation between phases.
#[derive(Debug)]
pub struct SincTable {
    spec: FilterSpec,
    /// `PHASES` rows of `taps` coefficients.
    rows: Vec<f32>,
    /// `PHASES` rows: row `p + 1` minus row `p`.
    deltas: Vec<f32>,
}

impl SincTable {
    /// Designs the table (control thread: allocates and computes).
    ///
    /// Row `p`, coefficient `k`, weighs source frame `i − taps/2 + 1 + k`
    /// for a read position `i + p / PHASES`: it is the response at
    /// `p / PHASES + taps/2 − 1 − k`.
    pub fn new(spec: FilterSpec) -> SincTable {
        let taps = spec.taps.clamp(2, MAX_TAPS) & !1;
        let spec = FilterSpec { taps, ..spec };
        let half = (taps / 2) as f64;
        let row = |p: usize| -> Vec<f64> {
            let frac = p as f64 / PHASES as f64;
            let mut row: Vec<f64> = (0..taps)
                .map(|k| spec.response(frac + half - 1.0 - k as f64))
                .collect();
            let sum: f64 = row.iter().sum();
            if sum.abs() > 1e-9 {
                row.iter_mut().for_each(|c| *c /= sum);
            }
            row
        };
        let all: Vec<Vec<f64>> = (0..=PHASES).map(row).collect();
        let mut rows = Vec::with_capacity(PHASES * taps);
        let mut deltas = Vec::with_capacity(PHASES * taps);
        for pair in all.windows(2) {
            if let [now, next] = pair {
                rows.extend(now.iter().map(|&c| c as f32));
                deltas.extend(now.iter().zip(next).map(|(&a, &b)| (b - a) as f32));
            }
        }
        SincTable { spec, rows, deltas }
    }

    /// What the table was designed from.
    pub fn spec(&self) -> &FilterSpec {
        &self.spec
    }

    /// The taps.
    pub fn taps(&self) -> usize {
        self.spec.taps
    }

    /// Writes into `out` (`taps` long) the coefficients for the fractional
    /// position `frac` (a 32-bit fraction of a source frame): row
    /// `frac >> 23` plus the remaining 23 bits' share of the difference to
    /// the next row. Real-time safe: reads the table, writes `out`.
    pub fn coefficients(&self, frac: u32, out: &mut [f32]) {
        let taps = self.spec.taps;
        let phase = (frac >> (32 - PHASE_BITS)) as usize;
        let rest = frac & ((1u32 << (32 - PHASE_BITS)) - 1);
        let a = rest as f32 * (1.0 / (1u32 << (32 - PHASE_BITS)) as f32);
        let at = phase * taps;
        let (Some(row), Some(delta)) =
            (self.rows.get(at..at + taps), self.deltas.get(at..at + taps))
        else {
            return;
        };
        for ((o, &c), &d) in out.iter_mut().zip(row).zip(delta) {
            *o = c + a * d;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ADR's fixed tap counts come out of the design rule. Equal rates
    /// at 44.1 kHz keep the 20 kHz passband, so their transition is the
    /// 44.1 ↔ 48 kHz one and so are their taps.
    #[test]
    fn tap_counts_as_in_the_adr() {
        assert_eq!(FilterSpec::for_rates(48_000, 48_000).taps, 96);
        assert_eq!(FilterSpec::for_rates(96_000, 96_000).taps, 96);
        assert_eq!(FilterSpec::for_rates(44_100, 44_100).taps, 160);
        assert_eq!(FilterSpec::for_rates(44_100, 48_000).taps, 160);
        assert_eq!(FilterSpec::for_rates(48_000, 44_100).taps, 160);
        assert!(FilterSpec::for_rates(192_000, 44_100).taps <= MAX_TAPS);
    }

    /// Every phase sums to 1 and the table matches the continuous response
    /// at its rows.
    #[test]
    fn rows_are_normalised() {
        let table = SincTable::new(FilterSpec::for_rates(48_000, 48_000));
        let mut row = vec![0.0f32; table.taps()];
        for frac in [0u32, 1 << 23, 0x8000_0000, u32::MAX] {
            table.coefficients(frac, &mut row);
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "{frac:#x}: {sum}");
        }
        // At frac 0 the centre tap (k = taps/2 − 1) is the response at 0.
        table.coefficients(0, &mut row);
        let centre = row[table.taps() / 2 - 1];
        assert!(centre > 0.9 && centre < 1.0, "{centre}");
    }

    #[test]
    fn bessel_i0_known_values() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-15);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-12);
        assert!((bessel_i0(10.0) / 2_815.716_628_466_254 - 1.0).abs() < 1e-12);
    }
}
