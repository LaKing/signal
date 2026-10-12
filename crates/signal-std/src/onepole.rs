//! `std.onepole`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::svf::{CUTOFF, CUTOFF_MAX, effective_cutoff, flush, integrator_gain};
use crate::{at, parameter, signal_port};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.onepole",
    doc: "TPT one-pole filter: low- and high-pass outputs, G = g/(1+g), g = tan(π·cutoff_hz/sr).",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("lp", Direction::Out, PortKind::Audio, None, None),
        signal_port("hp", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[parameter(
        "cutoff_hz",
        PortKind::Frequency,
        Some("Hz"),
        1.0,
        CUTOFF_MAX,
        CUTOFF,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.onepole`: a 1-pole filter in topology-preserving transform (TPT) form
/// (`docs/dsp-conventions.md`, "TPT filters").
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `lp`, `hp` (signal out, audio): low- and high-pass, `lp + hp = in`.
///
/// **Parameters**
/// - `cutoff_hz` (frequency, Hz, range `[1, 0.49·sr]`, default 1000, combine
///   `add`). Declared `[1, 0.49·(2³² − 1)]` as `std.svf`'s; `process` limits
///   the effective value to `0.49·sr`.
///
/// **Feedthrough:** direct (`in[n]` reaches both outputs on frame *n*).
///
/// **Formula** (x = `in`, s\[−1\] = 0, sr the sample rate):
///
/// ```text
/// fc = min(max(cutoff_hz[n], 1), 0.49·sr)
/// g  = tan(π·fc/sr),   G = g/(1 + g)
/// v  = (x[n] − s)·G
/// lp = v + s,   s ← lp + v
/// hp = x[n] − lp
/// ```
///
/// `G` is recomputed only on a frame where `fc` differs from the frame before.
/// When `|s|` falls below 1e-9 it is set to exactly 0
/// (`docs/dsp-conventions.md`, "Denormals"). The response is the bilinear
/// transform, prewarped at `fc`, of `lp = 1/(1 + s)`, `hp = s/(1 + s)` with
/// `s` normalised to `fc`. `tan` is `libm::tan`. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct OnePole {
    sample_rate: f64,
    s: f64,
    /// The `fc` that `big_g` was computed for (NaN: none yet).
    cutoff: f64,
    big_g: f64,
}

impl DirectPrimitive for OnePole {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.cutoff = f64::NAN;
        self.reset();
    }

    fn reset(&mut self) {
        self.s = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let cutoff = inputs.parameter(0);
        let mut signals = outputs.signals();
        let (Some(lp), Some(hp)) = (signals.next(), signals.next()) else {
            return;
        };
        for (n, (lp, hp)) in lp.iter_mut().zip(hp.iter_mut()).enumerate() {
            let fc = effective_cutoff(at(cutoff, n, CUTOFF), self.sample_rate);
            if fc != self.cutoff {
                self.cutoff = fc;
                let g = integrator_gain(fc, self.sample_rate);
                self.big_g = g / (1.0 + g);
            }
            let xn = at(x, n, 0.0);
            let v = (xn - self.s) * self.big_g;
            *lp = v + self.s;
            self.s = flush(*lp + v);
            *hp = xn - *lp;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svf::response::{
        Complex, FREQUENCIES, amplitude, db, modulated, normalised_s, sine,
    };
    use crate::testing::{Timeline, context, render};

    /// `[lp, hp]` for `input` with per-frame `cutoff`.
    fn filter(input: &[f64], cutoff: &[f64], sample_rate: f64) -> Vec<Vec<f64>> {
        let mut onepole = OnePole::default();
        onepole.prepare(&context(sample_rate, 512, 0));
        render(
            &mut onepole,
            &Timeline {
                signals: &[input],
                parameters: &[cutoff],
                triggers: &[],
            },
            input.len(),
            512,
        )
        .signals
    }

    /// AC3a: the magnitude of both outputs, measured from sine sweeps at 10
    /// frequencies, matches the analytic TPT response within 0.1 dB.
    #[test]
    fn magnitude_response_matches_the_analytic_tpt_response() {
        let sample_rate = 48_000.0;
        for fc in [1000.0, 6000.0] {
            for f in FREQUENCIES {
                let input = sine(f, sample_rate);
                let frames = input.len();
                let outputs = filter(&input, &vec![fc; frames], sample_rate);
                let s = normalised_s(f, fc, sample_rate);
                let denominator = Complex::new(1.0, 0.0) + s;
                let expected = [Complex::new(1.0, 0.0) / denominator, s / denominator];
                for ((name, samples), h) in ["lp", "hp"].iter().zip(&outputs).zip(expected) {
                    let measured = db(amplitude(&samples[frames / 2..], f, sample_rate));
                    let analytic = db(h.abs());
                    assert!(
                        (measured - analytic).abs() < 0.1,
                        "fc {fc}, {f} Hz, {name}: measured {measured} dB, analytic {analytic} dB"
                    );
                }
            }
        }
    }

    /// AC3b: with the cutoff swept between 20 Hz and 0.49·sr at audio rate,
    /// both outputs stay finite and bounded.
    #[test]
    fn stable_under_audio_rate_cutoff_modulation() {
        for sample_rate in [44_100.0, 96_000.0, 192_000.0] {
            let frames = sample_rate as usize;
            let (cutoff, input) = modulated(sample_rate, frames);
            let outputs = filter(&input, &cutoff, sample_rate);
            for (name, samples) in ["lp", "hp"].iter().zip(&outputs) {
                // `f64::max` skips NaN, so check every sample, not only the peak.
                assert!(samples.iter().all(|y| y.is_finite()), "non-finite sample");
                let peak = samples.iter().fold(0.0f64, |m, y| m.max(y.abs()));
                assert!(
                    peak.is_finite() && peak < 4.0,
                    "{sample_rate} Hz, {name}: peak {peak}"
                );
            }
        }
    }

    /// AC3c: the update is the formula, `cutoff_hz` is limited to
    /// `[1, 0.49·sr]`.
    #[test]
    fn follows_the_formula_and_limits_the_cutoff() {
        let sample_rate = 48_000.0;
        let frames = 400;
        let input: Vec<f64> = (0..frames)
            .map(|n| ((n * 13) % 17) as f64 / 8.0 - 1.0)
            .collect();
        let cutoff: Vec<f64> = (0..frames)
            .map(|n| [300.0, 0.25, 40_000.0, 7000.0][n / 100])
            .collect();
        let outputs = filter(&input, &cutoff, sample_rate);
        let mut s = 0.0;
        for n in 0..frames {
            let fc = cutoff[n].clamp(1.0, 0.49 * sample_rate);
            let g = libm::tan(std::f64::consts::PI * fc / sample_rate);
            let big_g = g / (1.0 + g);
            let v = (input[n] - s) * big_g;
            let lp = v + s;
            s = flush(lp + v);
            assert_eq!(
                [outputs[0][n], outputs[1][n]],
                [lp, input[n] - lp],
                "frame {n}"
            );
        }
    }
}
