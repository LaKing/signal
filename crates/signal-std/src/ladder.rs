//! `std.ladder`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::svf::{CUTOFF, CUTOFF_MAX, flush, integrator_gain};
use crate::{SIGNAL_BOUND, at, parameter, signal_port};

/// The feedback `k` at `resonance` 1. Above 4, where the linear ladder
/// starts to oscillate, so that the oscillation sustains at resonance 1
/// (from resonance 4/4.2 ≈ 0.952 on).
const K_MAX: f64 = 4.2;
/// The most Newton steps of the zero-delay-feedback solve in one frame.
const MAX_ITERATIONS: usize = 8;
/// The solve stops when a step moves `y` by at most this times `1 + |y|`.
const TOLERANCE: f64 = 1e-12;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.ladder",
    doc: "Zavalishin TPT four-pole transistor ladder with tanh at the input of the feedback loop, solved per frame (zero-delay feedback); 24 dB/oct lp and 12 dB/oct lp12, k = 4.2·resonance, bass-compensated.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("lp", Direction::Out, PortKind::Audio, None, None),
        signal_port("lp12", Direction::Out, PortKind::Audio, None, None),
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
        parameter(
            "resonance",
            PortKind::Generic,
            None,
            0.0,
            1.0,
            0.0,
            Combine::Add,
        ),
        parameter(
            "drive",
            PortKind::Generic,
            None,
            0.0,
            10.0,
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

/// `cutoff_hz` limited to `[1, 0.45·sr]`.
fn effective_cutoff(cutoff_hz: f64, sample_rate: f64) -> f64 {
    cutoff_hz.max(1.0).min(0.45 * sample_rate)
}

/// `std.ladder`: a four-pole transistor-ladder low-pass (the Moog filter) in
/// Zavalishin's topology-preserving transform (TPT) form, with a `tanh`
/// saturating the input of the feedback loop and the zero-delay feedback
/// solved on every frame.
///
/// **Model.** V. Zavalishin, *The Art of VA Filter Design*, revision 2.1.x
/// (chapter numbers differ in other revisions): §5.2 "Feedback and
/// resonance" and §5.3 "Digital model" for the four TPT one-pole stages in a
/// feedback loop of gain `k` (self-oscillation from `k = 4`) and the passband
/// compensation; §6.3 "Feedback loop saturation", §6.4 "Nonlinear zero-delay
/// feedback equation" and §6.5 "Iterative methods" for the `tanh` at the
/// loop's input and its solution by Newton–Raphson. Why this model: its
/// trapezoidal integrators make the linear response exactly the bilinear
/// transform of the analog ladder, prewarped at the cutoff, so the cutoff and
/// the self-oscillation stay in tune at every rate and under audio-rate
/// modulation without oversampling, as for `std.svf` (`docs/dsp-conventions.md`,
/// "TPT filters"). Huovilainen's model has a unit delay in the loop, which
/// needs oversampling and tuning corrections. One `tanh` (not one per stage)
/// gives a one-dimensional equation, increasing in `y` with a known bracket,
/// so the solve is bounded in time. The `tanh` is not antialiased: ADAA's
/// half frame of delay inside the zero-delay loop would detune it.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `lp` (signal out, audio): the fourth stage, 24 dB/oct.
/// - `lp12` (signal out, audio): the second stage, 12 dB/oct, with the same
///   feedback and compensation.
///
/// **Parameters**
/// - `cutoff_hz` (frequency, Hz, range `[1, 0.45·sr]`, default 1000, combine
///   `add`). Declared `[1, 100 000]` (ADR 0043) like `std.svf`'s; `process`
///   limits the effective value to `0.45·sr` of the rate it was prepared at.
/// - `resonance` (generic, no unit, range `[0, 1]`, default 0, combine
///   `add`): the feedback `k = 4.2·resonance`. The filter self-oscillates
///   at the cutoff from resonance 4/4.2 ≈ 0.952 on.
/// - `drive` (generic, no unit, range `[0, 10]`, default 0, combine `add`):
///   the input gain `1 + drive`, from unity (0) to ×11 (+20.8 dB, 10). There is
///   no make-up gain: at drive 0 a small signal passes at unity gain, more
///   drive is louder and saturates the `tanh` harder. (`std.tanh_shaper`'s
///   `drive` is the factor itself.)
///
/// **Feedthrough:** direct (`in[n]` reaches both outputs on frame *n*).
///
/// **Formula** (x = `in`, s₁…s₄\[−1\] = 0, sr the sample rate):
///
/// ```text
/// fc = min(max(cutoff_hz[n], 1), 0.45·sr)
/// g  = tan(π·fc/sr),   G = g/(1 + g),   G4 = G·G·G·G
/// k  = 4.2·resonance[n],   w = (1 + drive[n])·x[n]  (0 if not finite)
/// βᵢ = (1 − G)·sᵢ,   S = G·(G·(G·β₁ + β₂) + β₃) + β₄
/// solve y = G4·tanh(w − k·y) + S for y (y is y₄ before the stages run):
///   lo = S − G4,  hi = S + G4,  y = min(max((G4·w + S)/(1 + k·G4), lo), hi)
///   at most 8 times:
///     t = tanh(w − k·y),   F = y − G4·t − S
///     stop if F = 0;  if F > 0 then hi = y else lo = y
///     y' = y − F/(1 + k·G4·(1 − t²));  if not lo ≤ y' ≤ hi then y' = (lo + hi)/2
///     stop after y = y' if |y' − y| ≤ 1e-12·(1 + |y'|)
/// u = tanh(w − k·y)
/// stage i = 1…4, input u then yᵢ₋₁:  v = G·(input − sᵢ),  yᵢ = v + sᵢ,  sᵢ ← yᵢ + v
/// lp   = clamp((1 + k)·y₄, −10⁶, 10⁶)
/// lp12 = clamp((1 + k)·y₂, −10⁶, 10⁶)
/// ```
///
/// `F` increases with `y` (`F' = 1 + k·G4·(1 − t²) ≥ 1`) and changes sign in
/// `[S − G4, S + G4]` since `|tanh| ≤ 1`, so the bracketed Newton steps
/// converge. The bracket is closed: a Newton step that lands on one of its
/// ends is kept (the root may be there, as when `tanh` saturates and the
/// start was clamped to an end), and only a step outside it is replaced by
/// bisection. The stages always run on `u = tanh(…)`, so the output is
/// bounded (below) whether or not the solve converged within 8 steps.
/// `g`, `G` and `G4` are recomputed only on a frame where `fc` differs from
/// the frame before, and `k` only where `resonance` does (always on the
/// first frame after `prepare`). Each state `sᵢ` whose magnitude falls
/// below 1e-9 is set to exactly 0 (`docs/dsp-conventions.md`, "Denormals").
/// `tan` and `tanh` are `libm`'s.
///
/// **Response.** For a small input the `tanh` is the identity and the outputs
/// are the bilinear transform, prewarped at `fc`, of
/// `lp = (1 + k)/((1 + s)⁴ + k)` and `lp12 = (1 + k)·(1 + s)²/((1 + s)⁴ + k)`
/// with `s` normalised to `fc`.
///
/// **Bass compensation** is fixed and full: the factor `1 + k` restores the
/// passband (DC) gain to 1 at every resonance, which the feedback would
/// lower to `1/(1 + k)`. It is applied after the loop, so the `tanh` sees
/// the same small-signal level at every resonance.
///
/// **Bound.** `|u| ≤ 1`. A stage with input bounded by `M` keeps
/// `|sᵢ| ≤ max(1, g)·M` (the update is `sᵢ ← (1 − 2G)·sᵢ + 2G·input`, also
/// while `G` changes), so `|yᵢ| ≤ max(1, g)·M`. With `g ≤ tan(0.45π) ≈ 6.31`,
/// `|lp| ≤ 5.2·6.31⁴ ≈ 8.3·10³` and `|lp12| ≤ 5.2·6.31² ≈ 207` whatever the
/// input, drive, resonance or modulation, self-oscillation included: the
/// clamp to ±10⁶ ([`SIGNAL_BOUND`](crate::SIGNAL_BOUND), as the core types)
/// never acts on a finite state and only guarantees the bound.
///
/// A non-finite `w` (only possible in a direct call: through the engine
/// every input is finite, `docs/semantics.md` §7) is taken as 0, so it cannot
/// poison the states.
///
/// **Work per frame:** at most one `tan` (on a cutoff change) and nine
/// `tanh` (eight Newton steps and `u`); typically three to five (two to four
/// steps and `u`), see `docs/benchmarks.md`.
///
/// **Level.** The output is not limited to ±1, and a voice needs gain
/// staging after it. Measured (48 kHz, a 110 Hz saw of ±1, cutoff 300 Hz to
/// 4 kHz, the peak of the second half of 1 s): drive 0 peaks at 0.5–0.75 at
/// resonance 0, 1.2–1.5 at 0.5 and 1.5–2.2 at 1; drive 2 up to 3.7 and
/// drive 10 up to 5.2 (`(1 + k)` with `|u| ≤ 1`). Unity gain at drive 0 is a
/// small-signal property: at resonance 0 a ±1 sine in the passband comes out
/// at 0.76 (the `tanh`), at resonance 0.5 at 0.99.
#[derive(Clone, Debug, Default)]
pub struct Ladder {
    sample_rate: f64,
    /// s₁…s₄.
    s: [f64; 4],
    /// The `fc` and `resonance` the coefficients were computed for (NaN:
    /// none yet).
    cutoff: f64,
    resonance: f64,
    /// `G`, `G4` and `k`.
    gain: f64,
    gain4: f64,
    k: f64,
}

/// `u = tanh(w − k·y)` with `y` the solution of `y = G4·tanh(w − k·y) + S`
/// (the formula's solve), for the states `s`.
fn solve(w: f64, k: f64, gain: f64, gain4: f64, s: [f64; 4]) -> f64 {
    let [b1, b2, b3, b4] = s.map(|s| (1.0 - gain) * s);
    let sum = gain * (gain * (gain * b1 + b2) + b3) + b4;
    let (mut lo, mut hi) = (sum - gain4, sum + gain4);
    let mut y = ((gain4 * w + sum) / (1.0 + k * gain4)).max(lo).min(hi);
    for _ in 0..MAX_ITERATIONS {
        let t = libm::tanh(w - k * y);
        let f = y - gain4 * t - sum;
        if f == 0.0 {
            break;
        }
        if f > 0.0 {
            hi = y;
        } else {
            lo = y;
        }
        let mut next = y - f / (1.0 + k * gain4 * (1.0 - t * t));
        // The bracket is closed: the root may lie on either end. A Newton
        // step landing exactly on an end (as it does when the start was
        // clamped there and `f` is rounding noise) is the solution, not a
        // reason to bisect.
        if !(lo <= next && next <= hi) {
            next = (lo + hi) / 2.0;
        }
        let step = (next - y).abs();
        y = next;
        if step <= TOLERANCE * (1.0 + y.abs()) {
            break;
        }
    }
    libm::tanh(w - k * y)
}

impl DirectPrimitive for Ladder {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.cutoff = f64::NAN;
        self.resonance = f64::NAN;
        self.reset();
    }

    fn reset(&mut self) {
        self.s = [0.0; 4];
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let cutoff = inputs.parameter(0);
        let resonance = inputs.parameter(1);
        let drive = inputs.parameter(2);
        let mut signals = outputs.signals();
        let (Some(lp), Some(lp12)) = (signals.next(), signals.next()) else {
            return;
        };
        for (n, (lp, lp12)) in lp.iter_mut().zip(lp12.iter_mut()).enumerate() {
            [*lp, *lp12] = self.frame(
                at(x, n, 0.0),
                at(cutoff, n, CUTOFF),
                at(resonance, n, 0.0),
                at(drive, n, 0.0),
            );
        }
    }
}

impl Ladder {
    /// One frame of the formula: `[lp, lp12]` for input `x` and the
    /// effective `cutoff_hz`, `resonance` and `drive`.
    fn frame(&mut self, x: f64, cutoff: f64, resonance: f64, drive: f64) -> [f64; 2] {
        let fc = effective_cutoff(cutoff, self.sample_rate);
        if fc != self.cutoff {
            self.cutoff = fc;
            let g = integrator_gain(fc, self.sample_rate);
            self.gain = g / (1.0 + g);
            self.gain4 = self.gain * self.gain * self.gain * self.gain;
        }
        if resonance != self.resonance {
            self.resonance = resonance;
            self.k = K_MAX * resonance;
        }
        let w = (1.0 + drive) * x;
        let w = if w.is_finite() { w } else { 0.0 };
        let mut input = solve(w, self.k, self.gain, self.gain4, self.s);
        let mut taps = [0.0; 4];
        for (s, y) in self.s.iter_mut().zip(taps.iter_mut()) {
            let v = self.gain * (input - *s);
            *y = v + *s;
            *s = flush(*y + v);
            input = *y;
        }
        let [_, y2, _, y4] = taps;
        let compensation = 1.0 + self.k;
        [
            (compensation * y4).clamp(-SIGNAL_BOUND, SIGNAL_BOUND),
            (compensation * y2).clamp(-SIGNAL_BOUND, SIGNAL_BOUND),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svf::response::{
        Complex, FREQUENCIES, amplitude, db, modulated, normalised_s, sine,
    };
    use crate::testing::{Timeline, context, render};

    /// `[lp, lp12]` for `input` with per-frame `cutoff`, `resonance` and
    /// `drive`.
    fn filter(
        input: &[f64],
        cutoff: &[f64],
        resonance: &[f64],
        drive: &[f64],
        sample_rate: f64,
    ) -> Vec<Vec<f64>> {
        let mut ladder = Ladder::default();
        ladder.prepare(&context(sample_rate, 512, 0));
        render(
            &mut ladder,
            &Timeline {
                signals: &[input],
                parameters: &[cutoff, resonance, drive],
                triggers: &[],
            },
            input.len(),
            512,
        )
        .signals
    }

    /// [`filter`] with constant parameters.
    fn constant(input: &[f64], fc: f64, resonance: f64, drive: f64, rate: f64) -> Vec<Vec<f64>> {
        let n = input.len();
        filter(
            input,
            &vec![fc; n],
            &vec![resonance; n],
            &vec![drive; n],
            rate,
        )
    }

    fn impulse(frames: usize) -> Vec<f64> {
        let mut input = vec![0.0; frames];
        input[0] = 1.0;
        input
    }

    /// The A7 bound on `lp`: `(1 + 4.2)·tan(0.45π)⁴`.
    fn bound() -> f64 {
        (1.0 + K_MAX) * libm::pow(libm::tan(0.45 * std::f64::consts::PI), 4.0)
    }

    /// AC9: at low resonance a small input (amplitude 0.01, where `tanh` is
    /// the identity within 3·10⁻⁵ relative) gives the analytic four-pole
    /// response within 0.05 dB, measured from sine sweeps at 10 frequencies
    /// at every point where it is at least −100 dB; below that, the 1e-9
    /// state flush limits the measurement, which only has to be below −95 dB.
    #[test]
    fn magnitude_response_matches_the_analytic_4_pole_response() {
        let sample_rate = 48_000.0;
        let amplitude_in = 0.01;
        for (fc, resonance) in [(1000.0, 0.0), (1000.0, 0.3), (2500.0, 0.5)] {
            let k = Complex::new(K_MAX * resonance, 0.0);
            let one = Complex::new(1.0, 0.0);
            for f in FREQUENCIES {
                let input: Vec<f64> = sine(f, sample_rate)
                    .iter()
                    .map(|x| amplitude_in * x)
                    .collect();
                let frames = input.len();
                let outputs = constant(&input, fc, resonance, 0.0, sample_rate);
                let s = normalised_s(f, fc, sample_rate);
                let pole = (one + s) * (one + s);
                let denominator = pole * pole + k;
                let expected = [(one + k) / denominator, (one + k) * pole / denominator];
                for ((name, samples), h) in ["lp", "lp12"].iter().zip(&outputs).zip(expected) {
                    let measured =
                        db(amplitude(&samples[frames / 2..], f, sample_rate) / amplitude_in);
                    let analytic = db(h.abs());
                    let at = format!("fc {fc}, resonance {resonance}, {f} Hz, {name}");
                    if analytic >= -100.0 {
                        assert!(
                            (measured - analytic).abs() < 0.05,
                            "{at}: measured {measured} dB, analytic {analytic} dB"
                        );
                    } else {
                        assert!(measured < -95.0, "{at}: measured {measured} dB");
                    }
                }
            }
        }
    }

    /// AC10: at resonance 1 an impulse starts an oscillation that sustains at
    /// the cutoff, within 0.1 %, from 100 Hz to 0.45·sr (the limit) at 44.1, 96 and
    /// 192 kHz (measured from the zero crossings of the second half of 1 s,
    /// 2 s at 100 Hz).
    #[test]
    fn self_oscillates_at_the_cutoff() {
        for sample_rate in [44_100.0, 96_000.0, 192_000.0] {
            for fc in [100.0, 1000.0, 5000.0, 0.3 * sample_rate, 0.45 * sample_rate] {
                let seconds = if fc < 200.0 { 2.0 } else { 1.0 };
                let frames = (seconds * sample_rate) as usize;
                let lp = &constant(&impulse(frames), fc, 1.0, 0.0, sample_rate)[0];
                let measured =
                    signal_eval::zero_crossing_frequency(&lp[frames / 2..], sample_rate).unwrap();
                assert!(
                    (measured / fc - 1.0).abs() < 1e-3,
                    "{sample_rate} Hz, fc {fc}: oscillates at {measured} Hz"
                );
            }
        }
    }

    /// AC3: at cutoff 1 kHz and 48 kHz, an impulse decays to exactly 0
    /// within 1 s at resonance 0.9 (below the onset, k = 3.78), and at
    /// resonance 1 it starts an oscillation whose RMS over the last 0.1 s of
    /// the second is above 0.1.
    #[test]
    fn resonance_below_the_onset_decays_and_at_1_sustains() {
        let input = impulse(48_000);
        let decayed = constant(&input, 1000.0, 0.9, 0.0, 48_000.0);
        for samples in &decayed {
            assert_eq!(*samples.last().unwrap(), 0.0);
        }
        let sustained = &constant(&input, 1000.0, 1.0, 0.0, 48_000.0)[0];
        let tail = &sustained[sustained.len() - 4800..];
        let rms = libm::sqrt(tail.iter().map(|y| y * y).sum::<f64>() / tail.len() as f64);
        assert!(rms > 0.1, "rms {rms}");
    }

    /// AC4: a small constant input (10⁻⁴) settles to unity gain on both
    /// outputs at drive 0, at resonance 0 and 0.5 (the compensation keeps
    /// the passband at 1), and to ×11 at drive 10, the top of the range.
    #[test]
    fn drive_0_has_unity_gain() {
        let input = vec![1e-4; 48_000];
        for resonance in [0.0, 0.5] {
            for (drive, gain, tolerance) in [(0.0, 1.0, 1e-6), (10.0, 11.0, 1e-3)] {
                let outputs = constant(&input, 1000.0, resonance, drive, 48_000.0);
                for samples in &outputs {
                    let measured = samples.last().unwrap() / 1e-4;
                    assert!(
                        (measured - gain).abs() < tolerance,
                        "resonance {resonance}, drive {drive}: gain {measured}"
                    );
                }
            }
        }
    }

    /// AC5, AC11: with the cutoff swept between 20 Hz and 0.49·sr (above
    /// the 0.45·sr limit) by a 1 kHz sine, every output stays finite and
    /// within the A7 bound at every rate, resonance and drive tried,
    /// including resonance 1, drive 10 and a ±10⁶ input.
    #[test]
    fn stable_for_every_combination_under_audio_rate_modulation() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let frames = sample_rate as usize / 4;
            let (cutoff, square) = modulated(sample_rate, frames);
            for scale in [1.0, 1e6] {
                let input: Vec<f64> = square.iter().map(|x| scale * x).collect();
                for resonance in [0.0, 0.5, 0.95, 1.0] {
                    for drive in [0.0, 1.0, 10.0] {
                        let outputs = filter(
                            &input,
                            &cutoff,
                            &vec![resonance; frames],
                            &vec![drive; frames],
                            sample_rate,
                        );
                        for (name, samples) in ["lp", "lp12"].iter().zip(&outputs) {
                            // `f64::max` skips NaN, so check every sample.
                            assert!(samples.iter().all(|y| y.is_finite()), "non-finite sample");
                            let peak = samples.iter().fold(0.0f64, |m, y| m.max(y.abs()));
                            assert!(
                                peak <= bound(),
                                "{sample_rate} Hz, input ×{scale}, resonance {resonance}, \
                                 drive {drive}, {name}: peak {peak}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// AC5: `cutoff_hz` is limited to `[1, 0.45·sr]`.
    #[test]
    fn cutoff_is_limited_to_1_hz_and_0_45_sr() {
        let input: Vec<f64> = (0..256)
            .map(|n| ((n * 37) % 11) as f64 / 5.0 - 1.0)
            .collect();
        let at = |fc: f64| constant(&input, fc, 0.5, 1.0, 48_000.0);
        assert_eq!(at(40_000.0), at(0.45 * 48_000.0));
        assert_eq!(at(0.25), at(1.0));
        assert_ne!(at(0.45 * 48_000.0), at(0.44 * 48_000.0));
    }

    /// AC7: the update is the formula, written out again here; the
    /// coefficients are those of the current `cutoff_hz` and `resonance`,
    /// recomputed when they change.
    #[test]
    fn follows_the_formula_with_changing_coefficients() {
        let sample_rate = 48_000.0;
        let frames = 600;
        let input: Vec<f64> = (0..frames)
            .map(|n| ((n * 13) % 17) as f64 / 4.0 - 2.0)
            .collect();
        let cutoff: Vec<f64> = (0..frames)
            .map(|n| [300.0, 300.0, 5000.0][(n / 100) % 3])
            .collect();
        let resonance: Vec<f64> = (0..frames)
            .map(|n| if n < 350 { 0.2 } else { 1.0 })
            .collect();
        let drive: Vec<f64> = (0..frames).map(|n| (n % 5) as f64).collect();
        let outputs = filter(&input, &cutoff, &resonance, &drive, sample_rate);
        let mut s = [0.0f64; 4];
        for n in 0..frames {
            let g = libm::tan(std::f64::consts::PI * cutoff[n] / sample_rate);
            let big_g = g / (1.0 + g);
            let g4 = big_g * big_g * big_g * big_g;
            let k = 4.2 * resonance[n];
            let w = (1.0 + drive[n]) * input[n];
            let w = if w.is_finite() { w } else { 0.0 };
            let b: Vec<f64> = s.iter().map(|s| (1.0 - big_g) * s).collect();
            let sum = big_g * (big_g * (big_g * b[0] + b[1]) + b[2]) + b[3];
            let (mut lo, mut hi) = (sum - g4, sum + g4);
            let mut y = ((g4 * w + sum) / (1.0 + k * g4)).max(lo).min(hi);
            for _ in 0..8 {
                let t = libm::tanh(w - k * y);
                let f = y - g4 * t - sum;
                if f == 0.0 {
                    break;
                }
                if f > 0.0 {
                    hi = y;
                } else {
                    lo = y;
                }
                let mut next = y - f / (1.0 + k * g4 * (1.0 - t * t));
                if !(lo <= next && next <= hi) {
                    next = (lo + hi) / 2.0;
                }
                let step = (next - y).abs();
                y = next;
                if step <= 1e-12 * (1.0 + y.abs()) {
                    break;
                }
            }
            let mut stage = libm::tanh(w - k * y);
            let mut taps = [0.0; 4];
            for i in 0..4 {
                let v = big_g * (stage - s[i]);
                taps[i] = v + s[i];
                s[i] = crate::svf::flush(taps[i] + v);
                stage = taps[i];
            }
            assert_eq!(
                [outputs[0][n], outputs[1][n]],
                [(1.0 + k) * taps[3], (1.0 + k) * taps[1]],
                "frame {n}"
            );
        }
    }

    /// Review M2.11-2: the zero-delay feedback equation is solved on every
    /// frame. For a ±1 saw at cutoffs from 15 kHz to 0.45·sr, resonance 0.5,
    /// 0.8 and 1 and drive 0, 2 and 10, at 48 and 192 kHz, the `y₄` the stages
    /// produce satisfies `|y₄ − G4·tanh(w − k·y₄) − S| ≤ 10⁻⁹`, with `S` from
    /// the states before the frame. (A strict bracket, which bisected away a
    /// Newton step landing on its end, left residuals up to about 0.1 here.)
    #[test]
    fn solves_the_zero_delay_equation_on_every_frame() {
        for sample_rate in [48_000.0, 192_000.0] {
            for fc in [15_000.0, 18_000.0, 21_000.0, 0.45 * sample_rate] {
                for resonance in [0.5, 0.8, 1.0] {
                    for drive in [0.0, 2.0, 10.0] {
                        let mut ladder = Ladder::default();
                        ladder.prepare(&context(sample_rate, 512, 0));
                        let mut worst = 0.0f64;
                        for n in 0..(sample_rate as usize / 10) {
                            let x = 2.0 * (n as f64 * 110.0 / sample_rate).fract() - 1.0;
                            let s = ladder.s;
                            let [lp, _] = ladder.frame(x, fc, resonance, drive);
                            let (gain, gain4, k) = (ladder.gain, ladder.gain4, ladder.k);
                            let [b1, b2, b3, b4] = s.map(|s| (1.0 - gain) * s);
                            let sum = gain * (gain * (gain * b1 + b2) + b3) + b4;
                            let y4 = lp / (1.0 + k);
                            let w = (1.0 + drive) * x;
                            let residual = (y4 - gain4 * libm::tanh(w - k * y4) - sum).abs();
                            worst = worst.max(residual);
                        }
                        assert!(
                            worst <= 1e-9,
                            "{sample_rate} Hz, fc {fc}, resonance {resonance}, drive {drive}: \
                             residual {worst}"
                        );
                    }
                }
            }
        }
    }

    /// Review M2.11-2: a non-finite input in a direct call is taken as 0, so
    /// the states stay finite and the filter continues as if it were 0.
    #[test]
    fn a_non_finite_input_is_taken_as_0() {
        let input: Vec<f64> = (0..64).map(|n| ((n * 7) % 5) as f64 / 2.0 - 1.0).collect();
        let mut poisoned = input.clone();
        let mut zeroed = input.clone();
        for (n, bad) in [(5, f64::NAN), (9, f64::INFINITY), (20, f64::NEG_INFINITY)] {
            poisoned[n] = bad;
            zeroed[n] = 0.0;
        }
        let got = constant(&poisoned, 2000.0, 1.0, 3.0, 48_000.0);
        assert!(got.iter().flatten().all(|y| y.is_finite()));
        assert_eq!(got, constant(&zeroed, 2000.0, 1.0, 3.0, 48_000.0));
        // `drive` × a finite input that overflows is not finite either.
        let mut ladder = Ladder::default();
        ladder.prepare(&context(48_000.0, 512, 0));
        let out = ladder.frame(f64::MAX, 2000.0, 1.0, 10.0);
        assert_eq!(out, [0.0, 0.0]);
        assert_eq!(ladder.s, [0.0; 4]);
    }

    /// An impulse decays to exactly 0 (the states are flushed below 1e-9).
    #[test]
    fn an_impulse_decays_to_exactly_zero() {
        let outputs = constant(&impulse(48_000), 200.0, 0.5, 0.0, 48_000.0);
        for samples in &outputs {
            assert_eq!(*samples.last().unwrap(), 0.0);
            assert!(samples.iter().all(|y| !y.is_subnormal()));
        }
    }
}
