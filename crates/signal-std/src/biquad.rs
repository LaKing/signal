//! `std.biquad`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::line::{STATE_BOUND, finite_or_zero};
use crate::svf::{CUTOFF, CUTOFF_MAX, effective_cutoff, flush, integrator_gain};
use crate::{SIGNAL_BOUND, at, parameter, signal_port};

/// The filter types `mode` selects (ADR 0043: its value set).
const MODE_VALUES: &[signal_core::ValueLabel] = &[
    crate::named(0.0, "low-pass"),
    crate::named(1.0, "high-pass"),
    crate::named(2.0, "band-pass"),
    crate::named(3.0, "notch"),
    crate::named(4.0, "peak"),
    crate::named(5.0, "low shelf"),
    crate::named(6.0, "high shelf"),
    crate::named(7.0, "all-pass"),
];

/// The default `mode`: low-pass.
const LOWPASS: f64 = 0.0;
/// The specified default, a readable 0.7071 rather than 1/√2.
#[allow(clippy::approx_constant)]
const Q: f64 = 0.7071;
/// The largest boost or cut of `gain_db`, in dB.
const GAIN_DB_MAX: f64 = 48.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.biquad",
    doc: "RBJ cookbook biquad (low-pass, high-pass, band-pass, notch, peak, low shelf, high shelf, all-pass by `mode`), realised as a TPT state-variable filter with output mixing (Simper): the cookbook's transfer functions, stable under audio-rate modulation.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        crate::choice(
            parameter(
                "mode",
                PortKind::Generic,
                None,
                0.0,
                7.0,
                LOWPASS,
                Combine::Replace,
            ),
            MODE_VALUES,
        ),
        parameter(
            "cutoff_hz",
            PortKind::Frequency,
            Some("Hz"),
            1.0,
            CUTOFF_MAX,
            CUTOFF,
            Combine::Add,
        ),
        parameter("q", PortKind::Generic, None, 0.1, 40.0, Q, Combine::Add),
        parameter(
            "gain_db",
            PortKind::Generic,
            Some("dB"),
            -GAIN_DB_MAX,
            GAIN_DB_MAX,
            0.0,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::FILTER),
};

/// `std.biquad`: the eight second-order responses of R. Bristow-Johnson's
/// *Cookbook formulae for audio EQ biquad filter coefficients* (the "Audio EQ
/// Cookbook"), selected by `mode`.
///
/// **Realisation.** Every cookbook response is the bilinear transform,
/// prewarped at the centre frequency, of a second-order analog prototype
/// `H(s) = N(s)/(s² + k·s + 1)`. Instead of the cookbook's transposed direct
/// form II (TDF-II), the filter runs that prototype as a TPT state-variable
/// filter whose output is a mix of its input, band-pass and low-pass
/// (A. Simper, *Linear Trap Integrated SVF*, Cytomic technical paper SvfLinearTrapOptimised2,
/// 2013, which derives the mixes below for each cookbook type). Both forms
/// give the same transfer function for fixed coefficients (the unit test
/// `matches_a_tdf2_reference_with_cookbook_coefficients` runs the cookbook's
/// TDF-II beside it and agrees within 1e-9), but only this one is stable
/// under audio-rate modulation of `cutoff_hz` and `q`. A direct form's state
/// is a pair of past output and input combinations that belong to the old
/// coefficients: a fast sweep of a frozen-stable TDF-II can still grow
/// without bound (time-varying instability). The SVF's states are the
/// charges of two trapezoidal integrators, meaningful for any coefficients,
/// as in `std.svf` (`docs/dsp-conventions.md`, "TPT filters"). Coefficient
/// smoothing was rejected: it lowers how fast the coefficients move but
/// proves nothing, and it changes the response of a modulated filter.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `out` (signal out, audio).
///
/// **Parameters**
/// - `mode` (generic, no unit, choice, range `[0, 7]`, default 0, combine
///   `replace`): `round_half_up(mode)` selects, on every frame, 0 low-pass,
///   1 high-pass, 2 band-pass (constant 0 dB peak gain), 3 notch, 4 peak
///   (bell), 5 low shelf, 6 high shelf, 7 all-pass.
/// - `cutoff_hz` (frequency, Hz, range `[1, 0.49·sr]`, default 1000, combine
///   `add`): the cookbook's f0 (cutoff, centre or shelf midpoint). Declared
///   `[1, 100 000]` (ADR 0043) like `std.svf`'s; `process` limits it to `0.49·sr`.
/// - `q` (generic, no unit, range `[0.1, 40]`, default 0.7071, combine
///   `add`): the cookbook's Q (for the shelves, its Q form of the slope;
///   0.7071 is the steepest shelf without a bump).
/// - `gain_db` (generic, dB, range `[−48, 48]`, default 0, combine `add`):
///   the boost or cut of the peak and the shelves; the other modes ignore it.
///
/// **Feedthrough:** direct (`in[n]` reaches `out[n]`).
///
/// **Formula** (x = `in`, s₁\[−1\] = s₂\[−1\] = 0, sr the sample rate):
///
/// ```text
/// fc = min(max(cutoff_hz[n], 1), 0.49·sr),   m = round_half_up(mode[n])
/// A  = 10^(gain_db[n]/40),   t = tan(π·fc/sr),   k = 1/q[n]
/// g  = t/√A (low shelf),  t·√A (high shelf),  t (otherwise)
/// k  = 1/(q[n]·A) for the peak
/// (m₀, m₁, m₂) by mode:
///   0 low-pass   (0, 0, 1)            4 peak        (1, k·(A² − 1), 0)
///   1 high-pass  (1, −k, −1)          5 low shelf   (1, k·(A − 1), A² − 1)
///   2 band-pass  (0, k, 0)            6 high shelf  (A², k·(1 − A)·A, 1 − A²)
///   3 notch      (1, −k, 0)           7 all-pass    (1, −2k, 0)
/// a₁ = 1/(1 + g·(g + k)),   a₂ = g·a₁,   a₃ = g·a₂
/// v₀ = x[n] (0 if not finite),   v₃ = v₀ − s₂
/// v₁ = a₁·s₁ + a₂·v₃                  (band-pass, s/(s² + ks + 1))
/// v₂ = s₂ + a₂·s₁ + a₃·v₃             (low-pass,  1/(s² + ks + 1))
/// s₁ ← 2·v₁ − s₁,   s₂ ← 2·v₂ − s₂
/// out = clamp(m₀·v₀ + m₁·v₁ + m₂·v₂, −10⁶, 10⁶)
/// ```
///
/// The analog prototypes, `s` normalised to `fc`, are the cookbook's:
/// low-pass `1/D`, high-pass `s²/D`, band-pass `(s/Q)/D`, notch
/// `(s² + 1)/D`, all-pass `(s² − s/Q + 1)/D` with `D = s² + s/Q + 1`; peak
/// `(s² + s·A/Q + 1)/(s² + s/(A·Q) + 1)`; low shelf
/// `A·(s² + (√A/Q)·s + A)/(A·s² + (√A/Q)·s + 1)`; high shelf
/// `A·(A·s² + (√A/Q)·s + 1)/(s² + (√A/Q)·s + A)`.
///
/// The coefficients (`g`, `k`, `a₁…a₃`, `m₀…m₂`, one `tan`, one `pow` and a
/// square root) are recomputed only on a frame where `m`, `fc`, `q` or
/// `gain_db` differs from the frame before (always on the first frame after
/// `prepare`). Each state whose magnitude falls below 1e-9 is set to exactly
/// 0 (`docs/dsp-conventions.md`, "Denormals"), and each is clamped to
/// ±10¹²: for fixed coefficients the poles lie in the left half-plane
/// (`g > 0`, `k > 0`), so the states stay bounded and the clamp never acts;
/// it only keeps them finite under modulation. The output is clamped to
/// ±10⁶ ([`SIGNAL_BOUND`](crate::SIGNAL_BOUND)): the peak and the shelves
/// gain up to `A² = 10^(48/20) ≈ 251`, and a modulated resonance can ring
/// above the input. `tan`, `pow` and `sqrt` are `libm`'s. Constant work per
/// frame.
#[derive(Clone, Debug, Default)]
pub struct Biquad {
    sample_rate: f64,
    s1: f64,
    s2: f64,
    /// The mode, `fc`, `q` and `gain_db` the coefficients were computed for
    /// (NaN: none yet).
    key: [f64; 4],
    /// `a₁, a₂, a₃`.
    a: [f64; 3],
    /// `m₀, m₁, m₂`.
    m: [f64; 3],
}

/// `(g, k, [m₀, m₁, m₂])` of mode `mode` at the prewarped `t = tan(π·fc/sr)`.
fn coefficients(mode: u8, t: f64, q: f64, gain_db: f64) -> (f64, f64, [f64; 3]) {
    let k = 1.0 / q;
    let a = libm::pow(10.0, gain_db / 40.0);
    match mode {
        0 => (t, k, [0.0, 0.0, 1.0]),
        1 => (t, k, [1.0, -k, -1.0]),
        2 => (t, k, [0.0, k, 0.0]),
        3 => (t, k, [1.0, -k, 0.0]),
        4 => {
            let k = 1.0 / (q * a);
            (t, k, [1.0, k * (a * a - 1.0), 0.0])
        }
        5 => (t / libm::sqrt(a), k, [1.0, k * (a - 1.0), a * a - 1.0]),
        6 => (
            t * libm::sqrt(a),
            k,
            [a * a, k * (1.0 - a) * a, 1.0 - a * a],
        ),
        _ => (t, k, [1.0, -2.0 * k, 0.0]),
    }
}

impl DirectPrimitive for Biquad {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.key = [f64::NAN; 4];
        self.reset();
    }

    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let mode = inputs.parameter(0);
        let cutoff = inputs.parameter(1);
        let q = inputs.parameter(2);
        let gain = inputs.parameter(3);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let m = crate::chosen(
                MODE_VALUES,
                signal_core::Snap::Nearest,
                at(mode, n, LOWPASS),
            );
            let fc = effective_cutoff(at(cutoff, n, CUTOFF), self.sample_rate);
            let key = [m, fc, at(q, n, Q), at(gain, n, 0.0)];
            if key != self.key {
                self.key = key;
                let t = integrator_gain(fc, self.sample_rate);
                let (g, k, mix) = coefficients(m as u8, t, key[2], key[3]);
                let a1 = 1.0 / (1.0 + g * (g + k));
                let a2 = g * a1;
                self.a = [a1, a2, g * a2];
                self.m = mix;
            }
            let [a1, a2, a3] = self.a;
            let v0 = finite_or_zero(at(x, n, 0.0));
            let v3 = v0 - self.s2;
            let v1 = a1 * self.s1 + a2 * v3;
            let v2 = self.s2 + a2 * self.s1 + a3 * v3;
            self.s1 = flush((2.0 * v1 - self.s1).clamp(-STATE_BOUND, STATE_BOUND));
            self.s2 = flush((2.0 * v2 - self.s2).clamp(-STATE_BOUND, STATE_BOUND));
            let [m0, m1, m2] = self.m;
            *out = (m0 * v0 + m1 * v1 + m2 * v2).clamp(-SIGNAL_BOUND, SIGNAL_BOUND);
        }
    }
}

#[cfg(test)]
mod tests {
    // The mode is both the parameter value and the index of its name.
    #![allow(clippy::needless_range_loop)]

    use super::*;
    use crate::svf::response::{amplitude, db, modulated, sine};
    use crate::testing::{Timeline, context, render};

    const MODES: [&str; 8] = [
        "lowpass",
        "highpass",
        "bandpass",
        "notch",
        "peak",
        "lowshelf",
        "highshelf",
        "allpass",
    ];

    /// `out` for `input` with per-frame parameters `[mode, cutoff, q, gain]`.
    fn filter(input: &[f64], parameters: [&[f64]; 4], sample_rate: f64) -> Vec<f64> {
        let mut biquad = Biquad::default();
        biquad.prepare(&context(sample_rate, 512, 0));
        render(
            &mut biquad,
            &Timeline {
                signals: &[input],
                parameters: &parameters,
                triggers: &[],
            },
            input.len(),
            512,
        )
        .signals
        .remove(0)
    }

    fn constant(input: &[f64], mode: f64, fc: f64, q: f64, gain_db: f64, rate: f64) -> Vec<f64> {
        let n = input.len();
        filter(
            input,
            [&vec![mode; n], &vec![fc; n], &vec![q; n], &vec![gain_db; n]],
            rate,
        )
    }

    /// The cookbook's normalised `[b₀, b₁, b₂, a₁, a₂]` (a₀ = 1), written out
    /// from the Audio EQ Cookbook independently of the SVF mixes.
    fn cookbook(mode: usize, fc: f64, q: f64, gain_db: f64, sr: f64) -> [f64; 5] {
        let w0 = std::f64::consts::TAU * fc / sr;
        let (cs, sn) = (libm::cos(w0), libm::sin(w0));
        let alpha = sn / (2.0 * q);
        let a = libm::pow(10.0, gain_db / 40.0);
        let sa = 2.0 * libm::sqrt(a) * alpha;
        let [b0, b1, b2, a0, a1, a2] = match mode {
            0 => [
                (1.0 - cs) / 2.0,
                1.0 - cs,
                (1.0 - cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ],
            1 => [
                (1.0 + cs) / 2.0,
                -(1.0 + cs),
                (1.0 + cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ],
            2 => [alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha],
            3 => [1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha],
            4 => [
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ],
            5 => [
                a * ((a + 1.0) - (a - 1.0) * cs + sa),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - sa),
                (a + 1.0) + (a - 1.0) * cs + sa,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - sa,
            ],
            6 => [
                a * ((a + 1.0) + (a - 1.0) * cs + sa),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - sa),
                (a + 1.0) - (a - 1.0) * cs + sa,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - sa,
            ],
            _ => [
                1.0 - alpha,
                -2.0 * cs,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ],
        };
        [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
    }

    /// `|H(e^{jω})|` of cookbook coefficients at `f` Hz.
    fn cookbook_magnitude(c: [f64; 5], f: f64, sr: f64) -> f64 {
        let w = std::f64::consts::TAU * f / sr;
        let poly = |p0: f64, p1: f64, p2: f64| {
            let re = p0 + p1 * libm::cos(w) + p2 * libm::cos(2.0 * w);
            let im = -p1 * libm::sin(w) - p2 * libm::sin(2.0 * w);
            libm::sqrt(re * re + im * im)
        };
        poly(c[0], c[1], c[2]) / poly(1.0, c[3], c[4])
    }

    /// The magnitude of every mode, measured from sine sweeps at 10
    /// frequencies, matches the cookbook's analytic `|H(e^{jω})|` within
    /// 0.1 dB wherever it is above −80 dB (a notch's null is measured only
    /// to be below −60 dB).
    #[test]
    fn magnitude_response_matches_the_cookbook() {
        let sr = 48_000.0;
        let frequencies = [
            50.0, 120.0, 300.0, 700.0, 1000.0, 1500.0, 3000.0, 7000.0, 12_000.0, 20_000.0,
        ];
        for (mode, name) in MODES.iter().enumerate() {
            for (fc, q, gain_db) in [(1000.0, Q, 12.0), (3000.0, 4.0, -18.0), (700.0, 0.3, 6.0)] {
                let c = cookbook(mode, fc, q, gain_db, sr);
                for f in frequencies {
                    let input = sine(f, sr);
                    let out = constant(&input, mode as f64, fc, q, gain_db, sr);
                    let measured = db(amplitude(&out[out.len() / 2..], f, sr));
                    let analytic = db(cookbook_magnitude(c, f, sr));
                    let at = format!("{name}, fc {fc}, q {q}, {gain_db} dB, {f} Hz");
                    if analytic > -80.0 {
                        assert!(
                            (measured - analytic).abs() < 0.1,
                            "{at}: measured {measured} dB, analytic {analytic} dB"
                        );
                    } else {
                        assert!(measured < -60.0, "{at}: measured {measured} dB");
                    }
                }
            }
        }
    }

    /// For fixed coefficients the output equals the cookbook's TDF-II
    /// (`y = b₀x + z₁`, `z₁ ← b₁x − a₁y + z₂`, `z₂ ← b₂x − a₂y`) within
    /// 1e-9 of the signal's scale, sample by sample, for every mode at 44.1
    /// and 192 kHz: the same filter in a modulation-stable structure.
    #[test]
    fn matches_a_tdf2_reference_with_cookbook_coefficients() {
        for sr in [44_100.0, 192_000.0] {
            let input: Vec<f64> = (0..4000)
                .map(|n| libm::sin(n as f64 * 0.37) + 0.5 * ((n * 7919) % 13) as f64 / 13.0)
                .collect();
            for mode in 0..8 {
                for (fc, q, gain_db) in [(440.0, Q, 9.0), (5000.0, 10.0, -24.0)] {
                    let c = cookbook(mode, fc, q, gain_db, sr);
                    let out = constant(&input, mode as f64, fc, q, gain_db, sr);
                    let (mut z1, mut z2) = (0.0, 0.0);
                    for (n, &x) in input.iter().enumerate() {
                        let y = c[0] * x + z1;
                        z1 = c[1] * x - c[3] * y + z2;
                        z2 = c[2] * x - c[4] * y;
                        assert!(
                            (out[n] - y).abs() <= 1e-9 * (1.0 + y.abs()),
                            "{sr} Hz, {}, frame {n}: {} vs TDF-II {y}",
                            MODES[mode],
                            out[n]
                        );
                    }
                }
            }
        }
    }

    /// With the cutoff swept between 20 Hz and 0.49·sr by a 1 kHz sine and q
    /// at its ends, every mode stays finite and bounded at 44.1, 96 and
    /// 192 kHz, at the largest boost: within `A²` (the largest gain, 251)
    /// times a modest ringing allowance, far below the clamp.
    #[test]
    fn stable_under_audio_rate_cutoff_modulation() {
        for sr in [44_100.0, 96_000.0, 192_000.0] {
            let frames = sr as usize / 2;
            let (cutoff, input) = modulated(sr, frames);
            for mode in 0..8 {
                for q in [0.1, Q, 40.0] {
                    for gain_db in [-48.0, 48.0] {
                        let out = filter(
                            &input,
                            [
                                &vec![mode as f64; frames],
                                &cutoff,
                                &vec![q; frames],
                                &vec![gain_db; frames],
                            ],
                            sr,
                        );
                        assert!(out.iter().all(|y| y.is_finite()), "non-finite sample");
                        let peak = out.iter().fold(0.0f64, |m, y| m.max(y.abs()));
                        assert!(
                            peak < 251.2 * 200.0,
                            "{sr} Hz, {}, q {q}, {gain_db} dB: peak {peak}",
                            MODES[mode]
                        );
                    }
                }
            }
        }
    }

    /// The mode, q and gain also modulated at audio rate (every mode within
    /// a few frames) leave the output finite and bounded.
    #[test]
    fn stable_with_every_parameter_modulated() {
        let sr = 48_000.0;
        let frames = 48_000;
        let (cutoff, input) = modulated(sr, frames);
        let mode: Vec<f64> = (0..frames).map(|n| ((n / 3) % 8) as f64).collect();
        let q: Vec<f64> = (0..frames)
            .map(|n| 0.1 + 39.9 * (0.5 + 0.5 * libm::sin(n as f64 * 0.11)))
            .collect();
        let gain: Vec<f64> = (0..frames)
            .map(|n| 48.0 * libm::sin(n as f64 * 0.013))
            .collect();
        let out = filter(&input, [&mode, &cutoff, &q, &gain], sr);
        assert!(out.iter().all(|y| y.is_finite() && y.abs() < 1e5));
    }

    #[test]
    fn cutoff_is_limited_to_1_hz_and_0_49_sr() {
        let input: Vec<f64> = (0..256)
            .map(|n| ((n * 37) % 11) as f64 / 5.0 - 1.0)
            .collect();
        let at = |fc: f64| constant(&input, 4.0, fc, 2.0, 6.0, 48_000.0);
        assert_eq!(at(40_000.0), at(0.49 * 48_000.0));
        assert_eq!(at(0.25), at(1.0));
        assert_ne!(at(0.49 * 48_000.0), at(0.48 * 48_000.0));
    }

    /// A non-finite input in a direct call is taken as 0.
    #[test]
    fn a_non_finite_input_is_taken_as_0() {
        let input: Vec<f64> = (0..64).map(|n| ((n * 7) % 5) as f64 / 2.0 - 1.0).collect();
        let mut poisoned = input.clone();
        let mut zeroed = input.clone();
        for (n, bad) in [(5, f64::NAN), (9, f64::INFINITY)] {
            poisoned[n] = bad;
            zeroed[n] = 0.0;
        }
        let got = constant(&poisoned, 1.0, 2000.0, 3.0, 0.0, 48_000.0);
        assert_eq!(got, constant(&zeroed, 1.0, 2000.0, 3.0, 0.0, 48_000.0));
    }

    /// An impulse decays to exactly 0 in every mode (the states are flushed).
    #[test]
    fn an_impulse_decays_to_exactly_zero() {
        let mut input = vec![0.0; 48_000];
        input[0] = 1.0;
        for mode in 0..8 {
            let out = constant(&input, mode as f64, 200.0, 10.0, 12.0, 48_000.0);
            assert_eq!(*out.last().unwrap(), 0.0, "{}", MODES[mode]);
            assert!(out.iter().all(|y| !y.is_subnormal()));
        }
    }
}
