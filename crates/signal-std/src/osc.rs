//! `std.osc`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::sine_osc::FREQ_MAX;
use crate::{BIPOLAR, at, fract, parameter, round_half_up, signal_port, sine_of_cycles};

/// The default `shape`: saw.
const SAW: f64 = 1.0;
const PULSE_WIDTH: f64 = 0.5;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.osc",
    doc: "Band-limited oscillator (sine, saw, square, triangle) with an f64 phase accumulator; saw and square corrected with polyBLEP, triangle with polyBLAMP.",
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
            440.0,
            Combine::Add,
        ),
        parameter(
            "shape",
            PortKind::Generic,
            None,
            0.0,
            3.0,
            SAW,
            Combine::Replace,
        ),
        parameter(
            "pulse_width",
            PortKind::Generic,
            None,
            0.01,
            0.99,
            PULSE_WIDTH,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.osc`: a band-limited oscillator with four shapes, on the phase
/// accumulator of [`SineOsc`](crate::SineOsc).
///
/// **Ports**
/// - `pm` (signal in, phase, cycles, unbounded): phase offset added to the
///   phase for this frame only; unconnected it reads 0.
/// - `out` (signal out, audio, `[−1, 1]`).
///
/// **Parameters**
/// - `freq` (frequency, Hz, range `[0, sr/2]`, default 440, combine `add`).
///   Declared `[0, (2³² − 1)/2]` as `std.sine_osc`'s; `process` limits the
///   effective value to `sr/2`.
/// - `shape` (generic, no unit, choice, range `[0, 3]`, default 1, combine
///   `replace`): `round_half_up(shape)` selects 0 sine, 1 saw, 2 square,
///   3 triangle, on every frame.
/// - `pulse_width` (generic, no unit, fraction of the period, range
///   `[0.01, 0.99]`, default 0.5, combine `add`): the part of the period the
///   square is +1. Only the square reads it.
///
/// **Feedthrough:** direct (`pm[n]` reaches `out[n]`).
///
/// **Formula** (φ in cycles, f64, φ\[0\] = 0, sr the sample rate):
///
/// ```text
/// f[n]     = min(max(freq[n], 0), sr/2)
/// Δ        = f[n] / sr                              phase step, in cycles
/// p        = fract(φ[n] + pm[n])
/// sine     = sin(2π·(φ[n] + pm[n]))                 = std.sine_osc, bit for bit
/// saw      = 2p − 1 − blep(p)
/// square   = (p < w ? 1 : −1) + blep(p) − blep(fract(p − w))       w = pulse_width[n]
/// triangle = tri(p) + 8Δ·(blamp(fract(p − ¾)) − blamp(fract(p − ¼)))
/// φ[n+1]   = fract(φ[n] + Δ)
///
/// tri(p)   = 4p (p < ¼),  2 − 4p (p < ¾),  4p − 4 (otherwise)
/// blep(t)  = 2x − x² − 1,  x = t/Δ          if t < Δ
///          = x² + 2x + 1,  x = (t − 1)/Δ    if t > 1 − Δ
///          = 0                              otherwise
/// blamp(t) = (1 − t/Δ)³ / 6                 if t < Δ
///          = ((t − 1)/Δ + 1)³ / 6           if t > 1 − Δ
///          = 0                              otherwise
/// ```
///
/// `blep` is the 2-point polyBLEP residual of an upward jump of 2: saw and
/// square jump by ±2. `blamp` is its integral, the 2-point polyBLAMP residual
/// of a slope change of 1 per frame; the triangle's slope changes by ∓8 per
/// cycle, 8Δ per frame, at its corners ¼ and ¾ (`docs/dsp-conventions.md`,
/// "Band-limited oscillators"). With Δ = 0 no correction applies. The
/// corrections assume the phase advances by Δ per frame; a changing `pm`
/// moves the discontinuities without correcting for its own steps.
///
/// **Residual aliasing.** A 2-point polyBLEP is a short polynomial
/// approximation of a band-limited step, not an ideal one: it attenuates the
/// aliases of the saw and square most at low frequencies and less as the
/// fundamental rises, and leaves the top octave below `sr/2` slightly rolled
/// off. Measured with `signal_eval::aliasing` on a 2003 Hz tone at 96 kHz (the
/// test `aliasing_is_below_the_naive_waveforms` prints the values), the
/// fundamental is 29.3 dB above the inharmonic energy for the saw (naive
/// 13.7 dB), 33.0 dB for the square (naive 16.8 dB) and 61.6 dB for the
/// triangle (naive 49.2 dB): about 16 dB better than naive for the saw and
/// square, 12 dB for the triangle, whose aliases already fall off one order
/// faster. Aliasing remains, growing with the fundamental; whether it is
/// audible is a listening verdict. Oversampling is not used.
///
/// `fract(x) = x − ⌊x⌋`. `sin` is `libm::sin`. Work per frame is constant.
#[derive(Clone, Debug, Default)]
pub struct Osc {
    sample_rate: f64,
    phase: f64,
}

/// The 2-point polyBLEP residual at phase `t` of an upward jump of 2 at phase
/// 0, for a phase step `dt` ≤ ½.
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

/// The 2-point polyBLAMP residual at phase `t` of a slope change of 1 per
/// frame at phase 0: the integral of [`blep`] for a unit jump.
fn blamp(t: f64, dt: f64) -> f64 {
    let x = if t < dt {
        1.0 - t / dt
    } else if t > 1.0 - dt {
        (t - 1.0) / dt + 1.0
    } else {
        return 0.0;
    };
    x * x * x / 6.0
}

/// The naive triangle: 0 at phase 0, rising to 1 at ¼, −1 at ¾.
fn tri(p: f64) -> f64 {
    if p < 0.25 {
        4.0 * p
    } else if p < 0.75 {
        2.0 - 4.0 * p
    } else {
        4.0 * p - 4.0
    }
}

impl DirectPrimitive for Osc {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.phase = 0.0;
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let pm = inputs.signal(0);
        let freq = inputs.parameter(0);
        let shape = inputs.parameter(1);
        let width = inputs.parameter(2);
        let nyquist = self.sample_rate / 2.0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let f = at(freq, n, 0.0).max(0.0).min(nyquist);
            let dt = f / self.sample_rate;
            let cycles = self.phase + at(pm, n, 0.0);
            let p = fract(cycles);
            *out = match round_half_up(at(shape, n, SAW)) as u8 {
                0 => sine_of_cycles(cycles),
                1 => 2.0 * p - 1.0 - blep(p, dt),
                2 => {
                    let w = at(width, n, PULSE_WIDTH);
                    let naive = if p < w { 1.0 } else { -1.0 };
                    naive + blep(p, dt) - blep(fract(p - w), dt)
                }
                _ => tri(p) + 8.0 * dt * (blamp(fract(p - 0.75), dt) - blamp(fract(p - 0.25), dt)),
            };
            self.phase = fract(self.phase + dt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SineOsc;
    use crate::testing::{Timeline, context, render};

    const SHAPES: [(&str, f64); 4] = [
        ("sine", 0.0),
        ("saw", 1.0),
        ("square", 2.0),
        ("triangle", 3.0),
    ];

    fn osc(
        sample_rate: f64,
        freq: &[f64],
        shape: &[f64],
        width: &[f64],
        pm: &[f64],
        block: usize,
    ) -> Vec<f64> {
        let mut osc = Osc::default();
        osc.prepare(&context(sample_rate, block, 0));
        let run = render(
            &mut osc,
            &Timeline {
                signals: &[pm],
                parameters: &[freq, shape, width],
                triggers: &[],
            },
            freq.len(),
            block,
        );
        run.signals.into_iter().next().unwrap()
    }

    fn tone(freq: f64, shape: f64, sample_rate: f64, frames: usize) -> Vec<f64> {
        let zeros = vec![0.0; frames];
        osc(
            sample_rate,
            &vec![freq; frames],
            &vec![shape; frames],
            &vec![PULSE_WIDTH; frames],
            &zeros,
            4096,
        )
    }

    /// The phase sequence φ[n] of a constant `freq`, computed as `process` does.
    fn phases(freq: f64, sample_rate: f64, frames: usize) -> Vec<f64> {
        let mut phase = 0.0;
        (0..frames)
            .map(|_| {
                let p = phase;
                phase = fract(phase + freq / sample_rate);
                p
            })
            .collect()
    }

    /// AC2b: the sine is `std.sine_osc` bit for bit, with a varying frequency
    /// (above `sr/2` too) and phase offset.
    #[test]
    fn sine_is_sine_osc_bit_for_bit() {
        let frames = 20_000;
        let sample_rate = 48_000.0;
        let freq: Vec<f64> = (0..frames).map(|n| 3.7 * (n % 9001) as f64).collect();
        let pm: Vec<f64> = (0..frames)
            .map(|n| ((n * 7) % 13) as f64 / 5.0 - 1.3)
            .collect();
        let shape = vec![0.0; frames];
        let width = vec![PULSE_WIDTH; frames];
        let out = osc(sample_rate, &freq, &shape, &width, &pm, 61);

        let mut sine = SineOsc::default();
        sine.prepare(&context(sample_rate, 61, 0));
        let reference = render(
            &mut sine,
            &Timeline {
                signals: &[&pm],
                parameters: &[&freq],
                triggers: &[],
            },
            frames,
            61,
        );
        assert_eq!(out, reference.signals[0]);
    }

    /// AC2a: a 1 kHz tone of every shape measures 1 kHz within 1e-9 relative
    /// by zero crossings, at 96 kHz (and at 48 and 192 kHz). Assumption: "1 kHz
    /// and 96 kHz" means a 1 kHz tone at a 96 kHz sample rate, as for
    /// `std.sine_osc`. The rates are those where a 1 kHz period is a whole
    /// number of frames, so every interpolated crossing sits at the same place
    /// relative to the frames around it; at 44.1 kHz the square's corrected
    /// edge would put the interpolated crossing at one of ten positions.
    #[test]
    fn frequency_by_zero_crossings_is_exact_within_1e_9() {
        for sample_rate in [48_000.0, 96_000.0, 192_000.0] {
            let frames = 2 * sample_rate as usize;
            for (name, shape) in SHAPES {
                let samples = tone(1000.0, shape, sample_rate, frames);
                let measured = signal_eval::zero_crossing_frequency(&samples, sample_rate).unwrap();
                let error = (measured - 1000.0).abs() / 1000.0;
                assert!(
                    error < 1e-9,
                    "{name} at {sample_rate} Hz: measured {measured} Hz, relative error {error:e}"
                );
            }
        }
    }

    /// AC2c: on a 2003 Hz tone at 96 kHz (one second: a whole number of
    /// periods), the inharmonic energy of the corrected saw and square is at
    /// least 10 dB further below the fundamental than the naive waveform's.
    /// The values are printed (`--nocapture`); metrics do not decide quality.
    #[test]
    fn aliasing_is_below_the_naive_waveforms() {
        let (f0, sample_rate, frames) = (2003.0, 96_000.0, 96_000);
        let phases = phases(f0, sample_rate, frames);
        let naive_saw: Vec<f64> = phases.iter().map(|&p| 2.0 * p - 1.0).collect();
        let naive_square: Vec<f64> = phases
            .iter()
            .map(|&p| if p < PULSE_WIDTH { 1.0 } else { -1.0 })
            .collect();
        let naive_triangle: Vec<f64> = phases.iter().map(|&p| tri(p)).collect();
        for (name, shape, naive, required) in [
            ("saw", 1.0, naive_saw, true),
            ("square", 2.0, naive_square, true),
            ("triangle", 3.0, naive_triangle, false),
        ] {
            let corrected = tone(f0, shape, sample_rate, frames);
            let naive = signal_eval::aliasing(&naive, sample_rate, f0).unwrap();
            let corrected = signal_eval::aliasing(&corrected, sample_rate, f0).unwrap();
            println!(
                "std.osc {name}, {f0} Hz at {sample_rate} Hz: fundamental/inharmonic \
                 naive {:.2} dB, corrected {:.2} dB, improvement {:.2} dB",
                naive.ratio_db,
                corrected.ratio_db,
                corrected.ratio_db - naive.ratio_db
            );
            if required {
                assert!(
                    corrected.ratio_db >= naive.ratio_db + 10.0,
                    "{name}: naive {naive:?}, corrected {corrected:?}"
                );
            }
        }
    }

    /// AC2d: away from the discontinuities the shapes are the naive formulas;
    /// next to them, the polyBLEP and polyBLAMP terms.
    #[test]
    fn shapes_follow_the_formulas() {
        // sr 1000, 125 Hz: Δ = 1/8 and φ[n] = n/8 exactly.
        let (sample_rate, frames) = (1000.0, 16);
        let saw = tone(125.0, 1.0, sample_rate, frames);
        let square = tone(125.0, 2.0, sample_rate, frames);
        let triangle = tone(125.0, 3.0, sample_rate, frames);
        for n in 0..frames {
            let p = (n % 8) as f64 / 8.0;
            // p = 0 is the only phase within Δ of the saw's and the square's
            // jumps (0 and ½): blep(0) = −1.
            let (saw_expected, square_expected) = match n % 8 {
                0 => (0.0, 0.0),
                4 => (2.0 * p - 1.0, 0.0),
                _ => (2.0 * p - 1.0, if p < 0.5 { 1.0 } else { -1.0 }),
            };
            assert_eq!(saw[n], saw_expected, "saw frame {n}");
            assert_eq!(square[n], square_expected, "square frame {n}");
            // The corners ¼ and ¾ are at frames 2 and 6: blamp(0) = 1/6.
            let triangle_expected = match n % 8 {
                2 => 1.0 - 8.0 / 8.0 / 6.0,
                6 => -1.0 + 8.0 / 8.0 / 6.0,
                _ => tri(p),
            };
            assert_eq!(triangle[n], triangle_expected, "triangle frame {n}");
        }

        // Between frames, next to the saw's jump: Δ = 0.1, φ = 0.95 and 0.05.
        approx::assert_relative_eq!(blep(0.95, 0.1), 0.25, max_relative = 1e-14);
        approx::assert_relative_eq!(blep(0.05, 0.1), -0.25, max_relative = 1e-14);
        approx::assert_relative_eq!(blamp(0.05, 0.1), 0.125 / 6.0, max_relative = 1e-14);
        assert_eq!(blep(0.5, 0.0), 0.0);
        assert_eq!(blamp(0.999, 0.0), 0.0);
    }

    #[test]
    fn pulse_width_sets_the_part_of_the_period_at_plus_one() {
        // Δ = 1/100: frames 2 ..= 23 lie between the edges at 0 and 0.25.
        let frames = 100;
        let zeros = vec![0.0; frames];
        let out = osc(
            1000.0,
            &vec![10.0; frames],
            &vec![2.0; frames],
            &vec![0.25; frames],
            &zeros,
            16,
        );
        assert!(out[2..24].iter().all(|&y| y == 1.0));
        assert!(out[27..99].iter().all(|&y| y == -1.0));
        // φ[25] is 0.25 up to rounding: the falling edge is corrected to its midpoint.
        assert!(out[25].abs() < 1e-12, "{}", out[25]);
    }

    /// AC2d: `shape` is rounded half up on every frame.
    #[test]
    fn shape_is_a_choice_rounded_half_up() {
        let frames = 64;
        let reference: Vec<Vec<f64>> = (0..4)
            .map(|s| tone(440.0, s as f64, 48_000.0, frames))
            .collect();
        for (value, index) in [(0.49, 0), (0.5, 1), (1.49, 1), (1.5, 2), (2.5, 3), (3.0, 3)] {
            assert_eq!(
                tone(440.0, value, 48_000.0, frames),
                reference[index],
                "{value}"
            );
        }
    }

    /// AC2d: above `sr/2` the frequency is limited to `sr/2`, for every shape.
    #[test]
    fn frequency_is_limited_to_half_the_sample_rate() {
        for (name, shape) in SHAPES {
            let above = tone(40_000.0, shape, 48_000.0, 64);
            let nyquist = tone(24_000.0, shape, 48_000.0, 64);
            assert_eq!(above, nyquist, "{name}");
        }
    }

    /// The declared output range holds for every shape, frequency up to
    /// `sr/2` and pulse width.
    #[test]
    fn output_stays_within_minus_one_to_one() {
        let frames = 4000;
        let freq: Vec<f64> = (0..frames)
            .map(|n| 24_000.0 * (n as f64 / 3999.0))
            .collect();
        for (name, shape) in SHAPES {
            for width in [0.01, 0.3, 0.5, 0.99] {
                let out = osc(
                    48_000.0,
                    &freq,
                    &vec![shape; frames],
                    &vec![width; frames],
                    &vec![0.0; frames],
                    256,
                );
                assert!(
                    out.iter().all(|y| (-1.0..=1.0).contains(y)),
                    "{name}, width {width}"
                );
            }
        }
    }
}
