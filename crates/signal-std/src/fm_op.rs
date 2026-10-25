//! `std.fm_op`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::line::finite_or_zero;
use crate::sine_osc::FREQ_MAX;
use crate::{BIPOLAR, at, fract, parameter, signal_port, sine_of_cycles};

/// Where the frequency comes from, by `mode` (ADR 0043: its value set).
const MODE_VALUES: &[signal_core::ValueLabel] =
    &[crate::named(0.0, "ratio"), crate::named(1.0, "fixed")];

const FREQ: f64 = 440.0;
const RATIO: f64 = 1.0;
const RATIO_MAX: f64 = 64.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.fm_op",
    doc: "FM operator (phase modulation, as in the DX7): a sine on an f64 phase accumulator, at ratio·freq or a fixed frequency, with a phase-modulation input in cycles, self-feedback through the mean of its last two outputs, and an output level.",
    ports: &[
        signal_port("pm", Direction::In, PortKind::Phase, Some("cycles"), None),
        signal_port("out", Direction::Out, PortKind::Audio, None, BIPOLAR),
    ],
    parameters: &[
        parameter(
            "freq",
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            FREQ_MAX,
            FREQ,
            Combine::Add,
        ),
        crate::choice(
            parameter(
                "mode",
                PortKind::Generic,
                None,
                0.0,
                1.0,
                0.0,
                Combine::Replace,
            ),
            MODE_VALUES,
        ),
        parameter(
            "ratio",
            PortKind::Generic,
            None,
            0.0,
            RATIO_MAX,
            RATIO,
            Combine::Add,
        ),
        parameter(
            "fixed_hz",
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            FREQ_MAX,
            FREQ,
            Combine::Add,
        ),
        parameter(
            "feedback",
            PortKind::Generic,
            Some("cycles"),
            0.0,
            1.0,
            0.0,
            Combine::Add,
        ),
        parameter(
            "level",
            PortKind::Generic,
            None,
            0.0,
            1.0,
            1.0,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::GENERATOR),
};

/// `std.fm_op`: one operator of a phase-modulation ("FM") synthesiser
/// (J. Chowning, "The synthesis of complex audio spectra by means of
/// frequency modulation", JAES 21, 1973; the Yamaha DX7's operator, whose
/// modulation is in fact of the phase).
///
/// Operators connect `out` → `pm`: a modulator's `level` is then the peak
/// phase deviation of the carrier in cycles, and the modulation index in
/// radians is `I = 2π·level`. A carrier at `fc` modulated by a sine at `fm`
/// has components at `fc ± n·fm` of amplitude `|Jₙ(I)|` (Bessel functions
/// of the first kind). Algorithms (stacks, parallel carriers) are wired in
/// the patch; there is no envelope here (use `std.adsr` on `level`).
///
/// **Ports**
/// - `pm` (signal in, phase, cycles, unbounded): phase offset added for this
///   frame only, as `std.osc`'s; unconnected it reads 0.
/// - `out` (signal out, audio, `[−1, 1]`).
///
/// **Parameters**
/// - `freq` (frequency, Hz, range `[0, 100 000]` (ADR 0043), default 440, combine
///   `add`): the note frequency the ratio applies to.
/// - `mode` (generic, no unit, choice, range `[0, 1]`, default 0, combine
///   `replace`): `round_half_up(mode)` selects 0 ratio (`f = ratio·freq`) or
///   1 fixed (`f = fixed_hz`), on every frame.
/// - `ratio` (generic, no unit, range `[0, 64]`, default 1, combine `add`).
/// - `fixed_hz` (frequency, Hz, range `[0, 100 000]` (ADR 0043), default 440,
///   combine `add`).
/// - `feedback` (generic, cycles, range `[0, 1]`, default 0, combine `add`):
///   the phase offset, in cycles, per unit of the operator's own sine. 1 is
///   a self-modulation index of 2π.
/// - `level` (generic, no unit, range `[0, 1]`, default 1, combine `add`):
///   the output gain.
///
/// The effective frequency is limited in `process` to `[0, sr/2]`.
///
/// **Feedthrough:** direct (`pm[n]` reaches `out[n]`).
///
/// **Formula** (φ in cycles, f64, φ\[0\] = 0, y\[−1\] = y\[−2\] = 0):
///
/// ```text
/// f     = min(max(ratio[n]·freq[n] (mode 0) or fixed_hz[n] (mode 1), 0), sr/2)
/// u     = fract(φ[n] + pm[n] + feedback[n]·(y[n−1] + y[n−2])/2)
///                                            (pm[n] = 0 if not finite)
/// y[n]  = sin(2π·u)
/// out   = level[n]·y[n]
/// φ[n+1] = fract(φ[n] + f/sr)
/// ```
///
/// **Feedback.** A one-frame self-feedback `sin(2π(φ + β·y[n−1]))` turns to
/// period-2 oscillation ("hunting") once the loop gain `2πβ` per frame is
/// above about 1. Averaging the last two outputs, as the DX7 does, puts a
/// zero at sr/2 in the loop and removes that oscillation, so the waveform
/// moves smoothly from a sine towards a sawtooth as `feedback` rises (at
/// the top of the range it turns noisy, as a DX7's does). The feedback
/// reads the unscaled `y`, so `level` does not change the timbre.
///
/// **Band limit.** The operator itself is a sine, but phase modulation
/// spreads its spectrum without bound: by Carson's rule about 98 % of the
/// power lies within `fc ± (I + 1)·fm`, and components beyond sr/2 alias.
/// Nothing here removes that: keep `I·fm` well below sr/2 for clean high
/// notes (lower modulator levels at high pitch, as a DX7's keyboard scaling
/// does), or run at a higher rate. Feedback adds harmonics the same way.
///
/// `fract(x) = x − ⌊x⌋`, `sin` is `libm::sin`. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct FmOp {
    sample_rate: f64,
    phase: f64,
    /// `y[n−1]`, `y[n−2]`.
    previous: [f64; 2],
}

impl DirectPrimitive for FmOp {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.previous = [0.0; 2];
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let pm = inputs.signal(0);
        let freq = inputs.parameter(0);
        let mode = inputs.parameter(1);
        let ratio = inputs.parameter(2);
        let fixed = inputs.parameter(3);
        let feedback = inputs.parameter(4);
        let level = inputs.parameter(5);
        let nyquist = self.sample_rate / 2.0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let f = if crate::chosen(MODE_VALUES, signal_core::Snap::Nearest, at(mode, n, 0.0))
                >= 1.0
            {
                at(fixed, n, FREQ)
            } else {
                at(ratio, n, RATIO) * at(freq, n, FREQ)
            };
            let f = f.max(0.0).min(nyquist);
            let [y1, y2] = self.previous;
            let fb = at(feedback, n, 0.0) * (y1 + y2) / 2.0;
            let u = fract(self.phase + finite_or_zero(at(pm, n, 0.0)) + fb);
            let y = sine_of_cycles(u);
            self.previous = [y, y1];
            *out = at(level, n, 1.0) * y;
            self.phase = fract(self.phase + f / self.sample_rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svf::response::amplitude;
    use crate::testing::{Timeline, context, render};

    /// `out` for parameters `[freq, mode, ratio, fixed_hz, feedback, level]`
    /// (constant) and the `pm` input.
    fn op(pm: &[f64], p: [f64; 6], sr: f64) -> Vec<f64> {
        let n = pm.len();
        let columns: Vec<Vec<f64>> = p.iter().map(|&v| vec![v; n]).collect();
        let parameters: Vec<&[f64]> = columns.iter().map(Vec::as_slice).collect();
        let mut fm = FmOp::default();
        fm.prepare(&context(sr, 512, 0));
        render(
            &mut fm,
            &Timeline {
                signals: &[pm],
                parameters: &parameters,
                triggers: &[],
            },
            n,
            512,
        )
        .signals
        .remove(0)
    }

    /// Two operators, modulator → carrier: carrier 1000 Hz (ratio 2.5 of
    /// 400 Hz), modulator 100 Hz (fixed), modulator level 0.25 cycles, so
    /// `I = π/2`. Over one second (whole periods) the component at
    /// `1000 + n·100` Hz has amplitude `|Jₙ(π/2)|` within 1e-9 for
    /// n = −8…8, and the bins between them hold less than 1e-9.
    #[test]
    fn two_operators_give_bessel_sidebands() {
        let sr = 48_000.0;
        let frames = sr as usize;
        let modulator = op(&vec![0.0; frames], [400.0, 1.0, 1.0, 100.0, 0.0, 0.25], sr);
        let carrier = op(&modulator, [400.0, 0.0, 2.5, 0.0, 0.0, 1.0], sr);
        let index = std::f64::consts::PI / 2.0;
        for n in -8i32..=8 {
            let f = 1000.0 + 100.0 * n as f64;
            let measured = amplitude(&carrier, f, sr);
            let expected = libm::jn(n, index).abs();
            assert!(
                (measured - expected).abs() < 1e-9,
                "{f} Hz: measured {measured}, |J{n}| = {expected}"
            );
        }
        for f in [950.0, 1050.0, 1234.0, 3000.0] {
            assert!(amplitude(&carrier, f, sr) < 1e-9, "{f} Hz");
        }
    }

    /// The formula, written out again, with every parameter changing.
    #[test]
    fn follows_the_formula() {
        let sr = 48_000.0;
        let frames = 3000;
        let pm: Vec<f64> = (0..frames).map(|n| libm::sin(n as f64 * 0.01)).collect();
        let columns: Vec<Vec<f64>> = vec![
            (0..frames).map(|n| 200.0 + n as f64).collect(),
            (0..frames).map(|n| ((n / 500) % 2) as f64).collect(),
            (0..frames).map(|n| 0.5 + (n % 7) as f64).collect(),
            vec![30_000.0; frames],
            (0..frames).map(|n| (n % 11) as f64 / 10.0).collect(),
            (0..frames).map(|n| (n % 3) as f64 / 2.0).collect(),
        ];
        let parameters: Vec<&[f64]> = columns.iter().map(Vec::as_slice).collect();
        let mut fm = FmOp::default();
        fm.prepare(&context(sr, 512, 0));
        let out = render(
            &mut fm,
            &Timeline {
                signals: &[&pm],
                parameters: &parameters,
                triggers: &[],
            },
            frames,
            100,
        )
        .signals
        .remove(0);
        let (mut phase, mut y1, mut y2) = (0.0, 0.0, 0.0);
        for n in 0..frames {
            let f = if columns[1][n] >= 0.5 {
                columns[3][n]
            } else {
                columns[2][n] * columns[0][n]
            };
            let f = f.clamp(0.0, sr / 2.0);
            let u = fract(phase + pm[n] + columns[4][n] * (y1 + y2) / 2.0);
            let y = libm::sin(std::f64::consts::TAU * u);
            assert_eq!(out[n], columns[5][n] * y, "frame {n}");
            y2 = y1;
            y1 = y;
            phase = fract(phase + f / sr);
        }
    }

    /// Self-feedback without hunting: at feedback 0.15 a 220 Hz operator's
    /// harmonics fall monotonically (sawtooth-like), the second is at least
    /// 5 % of the fundamental, and every output stays within ±1 at every
    /// feedback up to 1, at 44.1 and 192 kHz.
    #[test]
    fn feedback_brightens_without_running_away() {
        let sr = 48_000.0;
        let frames = sr as usize;
        let out = op(&vec![0.0; frames], [220.0, 0.0, 1.0, 0.0, 0.15, 1.0], sr);
        let h: Vec<f64> = (1..=6)
            .map(|k| amplitude(&out, 220.0 * k as f64, sr))
            .collect();
        assert!(h.windows(2).all(|p| p[1] < p[0]), "{h:?}");
        assert!(h[1] > 0.05 * h[0], "{h:?}");
        for sr in [44_100.0, 192_000.0] {
            for feedback in [0.3, 0.6, 1.0] {
                let out = op(
                    &vec![0.0; 20_000],
                    [3000.0, 0.0, 1.0, 0.0, feedback, 1.0],
                    sr,
                );
                assert!(out.iter().all(|y| y.is_finite() && y.abs() <= 1.0));
            }
        }
    }

    /// The frequency is limited to sr/2, and the fixed mode ignores freq and
    /// ratio.
    #[test]
    fn frequency_limit_and_fixed_mode() {
        let pm = vec![0.0; 64];
        let sr = 48_000.0;
        assert_eq!(
            op(&pm, [30_000.0, 0.0, 1.0, 0.0, 0.0, 1.0], sr),
            op(&pm, [24_000.0, 0.0, 1.0, 0.0, 0.0, 1.0], sr)
        );
        assert_eq!(
            op(&pm, [123.0, 1.0, 7.0, 500.0, 0.0, 1.0], sr),
            op(&pm, [250.0, 0.0, 2.0, 0.0, 0.0, 1.0], sr)
        );
    }
}
