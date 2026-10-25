//! `std.svf`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{FLUSH_THRESHOLD, at, parameter, signal_port};

/// The upper end of `cutoff_hz`'s declared range: 0.49 times the highest
/// sample rate the engine accepts (any non-zero `u32`), so the executor's clamp
/// to the range never cuts a cutoff below `0.49·sr`. The effective limit is
/// applied in `process`. `std.onepole` uses it too.
/// ADR 0043: a filter's cutoff range ends at 100 kHz; `process` limits it
/// to `0.49·sr` at lower rates.
pub(crate) const CUTOFF_MAX: f64 = 100_000.0;
pub(crate) const CUTOFF: f64 = 1000.0;
/// The specified default, a readable 0.7071 rather than 1/√2.
#[allow(clippy::approx_constant)]
const Q: f64 = 0.7071;

/// `cutoff_hz` limited to `[1, 0.49·sr]`.
pub(crate) fn effective_cutoff(cutoff_hz: f64, sample_rate: f64) -> f64 {
    cutoff_hz.max(1.0).min(0.49 * sample_rate)
}

/// `g = tan(π·fc/sr)`, the prewarped integrator gain of a TPT filter.
pub(crate) fn integrator_gain(cutoff_hz: f64, sample_rate: f64) -> f64 {
    libm::tan(std::f64::consts::PI * cutoff_hz / sample_rate)
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.svf",
    doc: "Zavalishin TPT state-variable filter: low-, band- and high-pass outputs, g = tan(π·cutoff_hz/sr), k = 1/q.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("lp", Direction::Out, PortKind::Audio, None, None),
        signal_port("bp", Direction::Out, PortKind::Audio, None, None),
        signal_port("hp", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "cutoff_hz",
            PortKind::Frequency,
            Some("Hz"),
            1.0,
            CUTOFF_MAX,
            CUTOFF,
            Combine::Add,
        ),
        parameter("q", PortKind::Generic, None, 0.5, 40.0, Q, Combine::Add),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::FILTER),
};

/// `std.svf`: a 2-pole state-variable filter in Zavalishin's
/// topology-preserving transform (TPT) form: trapezoidal integrators, no
/// unit delay in the feedback loop, stable under audio-rate modulation
/// (`docs/dsp-conventions.md`, "TPT filters").
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `lp`, `bp`, `hp` (signal out, audio): low-, band- and high-pass. `bp` has
///   gain `q` at the cutoff (`lp + k·bp + hp = in`).
///
/// **Parameters**
/// - `cutoff_hz` (frequency, Hz, range `[1, 0.49·sr]`, default 1000, combine
///   `add`). Declared `[1, 100 000]` (ADR 0043; before, `[1, 0.49·(2³² − 1)]`): `0.49·sr` at the highest sample
///   rate the engine accepts; `process` limits the effective value to
///   `0.49·sr` of the rate it was prepared at.
/// - `q` (generic, no unit, range `[0.5, 40]`, default 0.7071, combine `add`).
///
/// **Feedthrough:** direct (`in[n]` reaches every output on frame *n*).
///
/// **Formula** (x = `in`, s₁\[−1\] = s₂\[−1\] = 0, sr the sample rate):
///
/// ```text
/// fc = min(max(cutoff_hz[n], 1), 0.49·sr)
/// g  = tan(π·fc/sr),   k = 1/q[n],   d = 1/(1 + k·g + g²)
/// hp = (x[n] − (k + g)·s₁ − s₂)·d
/// v₁ = g·hp,   bp = v₁ + s₁,   s₁ ← bp + v₁
/// v₂ = g·bp,   lp = v₂ + s₂,   s₂ ← lp + v₂
/// ```
///
/// `g`, `k` and `d` are recomputed only on a frame where `fc` or `q` differs
/// from the frame before (always on the first frame after `prepare`). Each
/// state `s₁`, `s₂` whose magnitude falls below 1e-9 is set to exactly 0
/// (`docs/dsp-conventions.md`, "Denormals"). The response is the bilinear
/// transform, prewarped at `fc`, of `lp = 1/(s² + ks + 1)`,
/// `bp = s/(s² + ks + 1)`, `hp = s²/(s² + ks + 1)` with `s` normalised to
/// `fc`. `tan` is `libm::tan`. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Svf {
    sample_rate: f64,
    s1: f64,
    s2: f64,
    /// The `fc` and `q` the coefficients were computed for (NaN: none yet).
    cutoff: f64,
    q: f64,
    g: f64,
    k: f64,
    d: f64,
}

impl DirectPrimitive for Svf {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.cutoff = f64::NAN;
        self.q = f64::NAN;
        self.reset();
    }

    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let cutoff = inputs.parameter(0);
        let q = inputs.parameter(1);
        let mut signals = outputs.signals();
        let (Some(lp), Some(bp), Some(hp)) = (signals.next(), signals.next(), signals.next())
        else {
            return;
        };
        let frames = lp.iter_mut().zip(bp.iter_mut()).zip(hp.iter_mut());
        for (n, ((lp, bp), hp)) in frames.enumerate() {
            let fc = effective_cutoff(at(cutoff, n, CUTOFF), self.sample_rate);
            let q = at(q, n, Q);
            if fc != self.cutoff || q != self.q {
                self.cutoff = fc;
                self.q = q;
                self.g = integrator_gain(fc, self.sample_rate);
                self.k = 1.0 / q;
                self.d = 1.0 / (1.0 + self.k * self.g + self.g * self.g);
            }
            *hp = (at(x, n, 0.0) - (self.k + self.g) * self.s1 - self.s2) * self.d;
            let v1 = self.g * *hp;
            *bp = v1 + self.s1;
            self.s1 = flush(*bp + v1);
            let v2 = self.g * *bp;
            *lp = v2 + self.s2;
            self.s2 = flush(*lp + v2);
        }
    }
}

/// `x`, or exactly 0 when `|x|` is below 1e-9.
pub(crate) fn flush(x: f64) -> f64 {
    if x.abs() < FLUSH_THRESHOLD { 0.0 } else { x }
}

/// Shared by the `std.svf` and `std.onepole` tests: sine sweeps against the
/// analytic TPT response, and audio-rate cutoff modulation.
#[cfg(test)]
pub(crate) mod response {
    use std::ops::{Add, Div, Mul};

    /// `s` of the analog prototype normalised to `fc` that the bilinear
    /// transform prewarped at `fc` maps the frequency `f` to:
    /// `j·tan(π·f/sr)/tan(π·fc/sr)`.
    pub(crate) fn normalised_s(f: f64, fc: f64, sample_rate: f64) -> Complex {
        let pi = std::f64::consts::PI;
        Complex::new(
            0.0,
            libm::tan(pi * f / sample_rate) / libm::tan(pi * fc / sample_rate),
        )
    }

    /// The 10 test frequencies at 48 kHz, whole numbers of Hz so that one
    /// second holds a whole number of periods.
    pub(crate) const FREQUENCIES: [f64; 10] = [
        50.0, 120.0, 300.0, 700.0, 1000.0, 1500.0, 3000.0, 7000.0, 12_000.0, 20_000.0,
    ];

    /// The amplitude of the `f` Hz component of `samples` (a whole number of
    /// periods): `2/N·|Σ y[n]·e^(−j2πfn/sr)|`.
    pub(crate) fn amplitude(samples: &[f64], f: f64, sample_rate: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &y) in samples.iter().enumerate() {
            let angle = std::f64::consts::TAU * f * n as f64 / sample_rate;
            re += y * libm::cos(angle);
            im -= y * libm::sin(angle);
        }
        2.0 / samples.len() as f64 * libm::sqrt(re * re + im * im)
    }

    /// One second of a unit sine at `f`, after one second of settling.
    pub(crate) fn sine(f: f64, sample_rate: f64) -> Vec<f64> {
        (0..2 * sample_rate as usize)
            .map(|n| libm::sin(std::f64::consts::TAU * f * n as f64 / sample_rate))
            .collect()
    }

    pub(crate) fn db(x: f64) -> f64 {
        20.0 * libm::log10(x)
    }

    /// A cutoff swept exponentially between 20 Hz and 0.49·sr and back by a
    /// 1 kHz sine (audio rate), with a ±1 square input whose period is 7
    /// frames, for the stability tests.
    pub(crate) fn modulated(sample_rate: f64, frames: usize) -> (Vec<f64>, Vec<f64>) {
        let (low, high) = (20.0, 0.49 * sample_rate);
        let cutoff = (0..frames)
            .map(|n| {
                let lfo = libm::sin(std::f64::consts::TAU * 1000.0 * n as f64 / sample_rate);
                low * libm::pow(high / low, 0.5 + 0.5 * lfo)
            })
            .collect();
        let input = (0..frames)
            .map(|n| if n % 7 < 4 { 1.0 } else { -1.0 })
            .collect();
        (cutoff, input)
    }

    /// A minimal complex number for the analytic responses.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Complex {
        pub re: f64,
        pub im: f64,
    }

    impl Complex {
        pub(crate) const fn new(re: f64, im: f64) -> Complex {
            Complex { re, im }
        }

        pub(crate) fn abs(self) -> f64 {
            libm::sqrt(self.re * self.re + self.im * self.im)
        }
    }

    impl Add for Complex {
        type Output = Complex;
        fn add(self, o: Complex) -> Complex {
            Complex::new(self.re + o.re, self.im + o.im)
        }
    }

    impl Mul for Complex {
        type Output = Complex;
        fn mul(self, o: Complex) -> Complex {
            Complex::new(
                self.re * o.re - self.im * o.im,
                self.re * o.im + self.im * o.re,
            )
        }
    }

    impl Div for Complex {
        type Output = Complex;
        fn div(self, o: Complex) -> Complex {
            let d = o.re * o.re + o.im * o.im;
            Complex::new(
                (self.re * o.re + self.im * o.im) / d,
                (self.im * o.re - self.re * o.im) / d,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::response::{Complex, FREQUENCIES, amplitude, db, modulated, normalised_s, sine};
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// `[lp, bp, hp]` for `input` with per-frame `cutoff` and `q`.
    fn filter(input: &[f64], cutoff: &[f64], q: &[f64], sample_rate: f64) -> Vec<Vec<f64>> {
        let mut svf = Svf::default();
        svf.prepare(&context(sample_rate, 512, 0));
        render(
            &mut svf,
            &Timeline {
                signals: &[input],
                parameters: &[cutoff, q],
                triggers: &[],
            },
            input.len(),
            512,
        )
        .signals
    }

    /// AC3a: the magnitude of every output, measured from sine sweeps at 10
    /// frequencies, matches the analytic TPT response within 0.1 dB.
    #[test]
    fn magnitude_response_matches_the_analytic_tpt_response() {
        let sample_rate = 48_000.0;
        for (fc, q) in [(1000.0, Q), (2500.0, 8.0)] {
            let k = 1.0 / q;
            for f in FREQUENCIES {
                let input = sine(f, sample_rate);
                let frames = input.len();
                let outputs = filter(&input, &vec![fc; frames], &vec![q; frames], sample_rate);
                let s = normalised_s(f, fc, sample_rate);
                let one = Complex::new(1.0, 0.0);
                let denominator = s * s + Complex::new(k, 0.0) * s + one;
                let expected = [one / denominator, s / denominator, s * s / denominator];
                for ((name, samples), h) in ["lp", "bp", "hp"].iter().zip(&outputs).zip(expected) {
                    let measured = db(amplitude(&samples[frames / 2..], f, sample_rate));
                    let analytic = db(h.abs());
                    assert!(
                        (measured - analytic).abs() < 0.1,
                        "fc {fc}, q {q}, {f} Hz, {name}: measured {measured} dB, analytic {analytic} dB"
                    );
                }
            }
        }
    }

    /// AC3b: with the cutoff swept between 20 Hz and 0.49·sr at audio rate,
    /// every output stays finite and bounded, at the lowest and highest q.
    #[test]
    fn stable_under_audio_rate_cutoff_modulation() {
        for sample_rate in [44_100.0, 96_000.0, 192_000.0] {
            let frames = sample_rate as usize;
            let (cutoff, input) = modulated(sample_rate, frames);
            for q in [0.5, Q, 40.0] {
                let outputs = filter(&input, &cutoff, &vec![q; frames], sample_rate);
                for (name, samples) in ["lp", "bp", "hp"].iter().zip(&outputs) {
                    // `f64::max` skips NaN, so check every sample, not only the peak.
                    assert!(samples.iter().all(|y| y.is_finite()), "non-finite sample");
                    let peak = samples.iter().fold(0.0f64, |m, y| m.max(y.abs()));
                    assert!(
                        peak.is_finite() && peak < 200.0,
                        "{sample_rate} Hz, q {q}, {name}: peak {peak}"
                    );
                }
            }
        }
    }

    /// AC3c: `cutoff_hz` is limited to `[1, 0.49·sr]`.
    #[test]
    fn cutoff_is_limited_to_1_hz_and_0_49_sr() {
        let input: Vec<f64> = (0..256)
            .map(|n| ((n * 37) % 11) as f64 / 5.0 - 1.0)
            .collect();
        let q = vec![Q; input.len()];
        let at = |fc: f64| filter(&input, &vec![fc; input.len()], &q, 48_000.0);
        assert_eq!(at(40_000.0), at(0.49 * 48_000.0));
        assert_eq!(at(0.25), at(1.0));
        assert_ne!(at(0.49 * 48_000.0), at(0.48 * 48_000.0));
    }

    /// AC3c: the update is the formula; the coefficients are those of the
    /// current `cutoff_hz` and `q`, recomputed when they change.
    #[test]
    fn follows_the_formula_with_changing_coefficients() {
        let sample_rate = 48_000.0;
        let frames = 600;
        let input: Vec<f64> = (0..frames)
            .map(|n| ((n * 13) % 17) as f64 / 8.0 - 1.0)
            .collect();
        let cutoff: Vec<f64> = (0..frames)
            .map(|n| [300.0, 300.0, 5000.0][(n / 100) % 3])
            .collect();
        let q: Vec<f64> = (0..frames).map(|n| if n < 350 { Q } else { 4.0 }).collect();
        let outputs = filter(&input, &cutoff, &q, sample_rate);
        let (mut s1, mut s2) = (0.0, 0.0);
        for n in 0..frames {
            let g = libm::tan(std::f64::consts::PI * cutoff[n] / sample_rate);
            let k = 1.0 / q[n];
            let d = 1.0 / (1.0 + k * g + g * g);
            let hp = (input[n] - (k + g) * s1 - s2) * d;
            let v1 = g * hp;
            let bp = v1 + s1;
            s1 = flush(bp + v1);
            let v2 = g * bp;
            let lp = v2 + s2;
            s2 = flush(lp + v2);
            assert_eq!(
                [outputs[0][n], outputs[1][n], outputs[2][n]],
                [lp, bp, hp],
                "frame {n}"
            );
        }
    }

    /// An impulse decays to exactly 0 (the states are flushed below 1e-9).
    #[test]
    fn an_impulse_decays_to_exactly_zero() {
        let mut input = vec![0.0; 48_000];
        input[0] = 1.0;
        let frames = input.len();
        let outputs = filter(&input, &vec![200.0; frames], &vec![10.0; frames], 48_000.0);
        for samples in &outputs {
            assert_eq!(*samples.last().unwrap(), 0.0);
            assert!(samples.iter().all(|y| !y.is_subnormal()));
        }
    }
}
