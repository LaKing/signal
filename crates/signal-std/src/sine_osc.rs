//! `std.sine_osc`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{BIPOLAR, at, fract, parameter, signal_port, sine_of_cycles};

/// The upper end of `freq`'s declared range: half the highest sample rate the
/// engine accepts (any non-zero `u32`), so the executor's clamp to the range
/// never cuts a frequency below `sr/2`. A descriptor's range is static; the
/// effective limit `sr/2` is applied in `process`. `std.osc` uses it too.
pub(crate) const FREQ_MAX: f64 = u32::MAX as f64 / 2.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.sine_osc",
    doc: "Sine oscillator with an f64 phase accumulator and a phase-offset input `pm` in cycles.",
    ports: &[
        signal_port("pm", Direction::In, PortKind::Phase, Some("cycles"), None),
        signal_port("out", Direction::Out, PortKind::Audio, None, BIPOLAR),
    ],
    parameters: &[parameter(
        "freq",
        PortKind::Frequency,
        Some("Hz"),
        0.0,
        FREQ_MAX,
        440.0,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.sine_osc`: a sine oscillator whose phase is an f64 accumulator in cycles.
///
/// **Ports**
/// - `pm` (signal in, phase, cycles, unbounded): phase offset added to the
///   phase for this frame only; unconnected it reads 0.
/// - `out` (signal out, audio, `[−1, 1]`).
///
/// **Parameters**
/// - `freq` (frequency, Hz, range `[0, sr/2]`, default 440, combine `add`).
///   A descriptor's range is static, so it declares `[0, (2³² − 1)/2]`
///   (≈ 2.147 GHz): `sr/2` at the highest sample rate the engine accepts
///   (`EngineConfig::sample_rate` is a non-zero `u32`). `process` limits the
///   effective value to `sr/2` of the rate it was prepared at.
///
/// **Feedthrough:** direct (`pm[n]` reaches `out[n]`).
///
/// **Formula** (φ in cycles, f64, φ\[0\] = 0, sr the sample rate):
///
/// ```text
/// out[n]   = sin(2π·(φ[n] + pm[n]))
/// φ[n+1]   = fract(φ[n] + min(freq[n], sr/2) / sr)
/// ```
///
/// `fract(x) = x − ⌊x⌋`. `sin` is `libm::sin`. Work per frame is constant.
#[derive(Clone, Debug, Default)]
pub struct SineOsc {
    sample_rate: f64,
    phase: f64,
}

impl DirectPrimitive for SineOsc {
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
        let nyquist = self.sample_rate / 2.0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = sine_of_cycles(self.phase + at(pm, n, 0.0));
            let f = at(freq, n, 0.0).max(0.0).min(nyquist);
            self.phase = fract(self.phase + f / self.sample_rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn tone(freq: f64, sample_rate: f64, frames: usize, pm: &[f64]) -> Vec<f64> {
        let mut osc = SineOsc::default();
        osc.prepare(&context(sample_rate, 4096, 0));
        let freq = vec![freq; frames];
        let run = render(
            &mut osc,
            &Timeline {
                signals: &[pm],
                parameters: &[&freq],
                triggers: &[],
            },
            frames,
            4096,
        );
        run.signals.into_iter().next().unwrap()
    }

    /// The frequency measured from the upward zero crossings of `samples`:
    /// (crossings − 1) cycles over the time from the first to the last.
    /// Each crossing is located between the two samples bracketing it by
    /// interpolating their phases `asin(y)`, which is exact for a unit sine, so
    /// the measurement adds only rounding error.
    fn zero_crossing_frequency(samples: &[f64], sample_rate: f64) -> f64 {
        let crossings: Vec<f64> = samples
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(n, w)| {
                let (before, after) = (libm::asin(w[0]), libm::asin(w[1]));
                n as f64 + -before / (after - before)
            })
            .collect();
        assert!(crossings.len() > 100, "{} crossings", crossings.len());
        let span = crossings.last().unwrap() - crossings.first().unwrap();
        (crossings.len() - 1) as f64 * sample_rate / span
    }

    /// AC6: a 1 kHz tone measures 1 kHz within 1e-9 relative, at 96 kHz and at
    /// every other rate the contract suite uses.
    #[test]
    fn frequency_by_zero_crossings_is_exact_within_1e_9() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let frames = 2 * sample_rate as usize;
            let samples = tone(1000.0, sample_rate, frames, &vec![0.0; frames]);
            let measured = zero_crossing_frequency(&samples, sample_rate);
            let error = (measured - 1000.0).abs() / 1000.0;
            assert!(
                error < 1e-9,
                "at {sample_rate} Hz: measured {measured} Hz, relative error {error:e}"
            );
        }
    }

    #[test]
    fn phase_starts_at_zero_and_pm_offsets_it_in_cycles() {
        let pm = [0.0, 0.25, 0.0, -0.25];
        let out = tone(0.0, 48_000.0, 4, &pm);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 1.0);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], -1.0);
    }

    /// auditor-1-frequency-ceiling: at 384 kHz a 150 kHz tone is inside the
    /// declared range (the executor's clamp leaves it alone) and reaches the
    /// phase increment unclamped: the output is the normative formula.
    #[test]
    fn high_sample_rates_keep_frequencies_up_to_half_the_rate() {
        let range = DESCRIPTOR.parameters[0].range;
        assert_eq!(range.max, f64::from(u32::MAX) / 2.0);
        for sample_rate in [384_000.0, 768_000.0, f64::from(u32::MAX)] {
            assert!(range.min <= 0.0 && sample_rate / 2.0 <= range.max);
        }

        let (freq, sample_rate, frames) = (150_000.0, 384_000.0, 64);
        let out = tone(freq, sample_rate, frames, &vec![0.0; frames]);
        let mut phase = 0.0;
        for (n, &y) in out.iter().enumerate() {
            assert_eq!(y, libm::sin(std::f64::consts::TAU * phase), "frame {n}");
            phase = fract(phase + freq / sample_rate);
        }
        assert!((out[1] - 0.6343932841636455).abs() < 1e-15, "{}", out[1]);

        // sr/2 itself passes; above it, process limits the frequency to sr/2.
        let zeros = vec![0.0; frames];
        assert_eq!(
            tone(250_000.0, sample_rate, frames, &zeros),
            tone(192_000.0, sample_rate, frames, &zeros)
        );
    }

    #[test]
    fn frequency_is_limited_to_half_the_sample_rate() {
        let frames = 64;
        let zeros = vec![0.0; frames];
        let above = tone(40_000.0, 48_000.0, frames, &zeros);
        let nyquist = tone(24_000.0, 48_000.0, frames, &zeros);
        assert_eq!(above, nyquist);
    }
}
