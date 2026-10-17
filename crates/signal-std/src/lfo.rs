//! `std.lfo`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{BIPOLAR, at, fract, parameter, signal_port, sine_of_cycles};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.lfo",
    doc: "Low-frequency sine oscillator with an f64 phase accumulator; output in [-1, 1].",
    ports: &[signal_port(
        "out",
        Direction::Out,
        PortKind::Control,
        None,
        BIPOLAR,
    )],
    parameters: &[
        parameter(
            "freq_hz",
            PortKind::Frequency,
            Some("Hz"),
            0.001,
            100.0,
            1.0,
            Combine::Add,
        ),
        parameter(
            "shape",
            PortKind::Generic,
            None,
            0.0,
            0.0,
            0.0,
            Combine::Replace,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::GENERATOR),
};

/// `std.lfo`: a low-frequency sine oscillator, computed as [`SineOsc`](crate::SineOsc)
/// is, without a phase-offset input.
///
/// **Ports**
/// - `out` (signal out, control, `[−1, 1]`).
///
/// **Parameters**
/// - `freq_hz` (frequency, Hz, range `[0.001, 100]`, default 1, combine `add`).
/// - `shape` (generic, no unit, range `[0, 0]`, default 0, combine `replace`):
///   the waveform. 0 is sine, the only shape in M1; other waveforms are out of
///   scope.
///
/// **Feedthrough:** direct (it has no signal inputs; `freq_hz[n]` sets the
/// phase step after frame *n*).
///
/// **Formula** (φ in cycles, f64, φ\[0\] = 0, sr the sample rate):
///
/// ```text
/// out[n]   = sin(2π·φ[n])
/// φ[n+1]   = fract(φ[n] + freq_hz[n] / sr)
/// ```
///
/// `fract(x) = x − ⌊x⌋`. `sin` is `libm::sin`. Work per frame is constant.
#[derive(Clone, Debug, Default)]
pub struct Lfo {
    sample_rate: f64,
    phase: f64,
}

impl DirectPrimitive for Lfo {
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
        let freq = inputs.parameter(0);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = sine_of_cycles(self.phase);
            self.phase = fract(self.phase + at(freq, n, 0.0) / self.sample_rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SineOsc;
    use crate::testing::{Timeline, context, render};

    #[test]
    fn lfo_equals_sine_osc_without_phase_offset() {
        let frames = 10_000;
        let freq: Vec<f64> = (0..frames).map(|n| 0.5 + (n % 97) as f64).collect();
        let shape = vec![0.0; frames];
        let zeros = vec![0.0; frames];

        let mut lfo = Lfo::default();
        lfo.prepare(&context(1000.0, 64, 0));
        let lfo = render(
            &mut lfo,
            &Timeline {
                signals: &[],
                parameters: &[&freq, &shape],
                triggers: &[],
            },
            frames,
            64,
        );
        let mut osc = SineOsc::default();
        osc.prepare(&context(1000.0, 64, 0));
        let osc = render(
            &mut osc,
            &Timeline {
                signals: &[&zeros],
                parameters: &[&freq],
                triggers: &[],
            },
            frames,
            64,
        );
        assert_eq!(lfo.signals, osc.signals);
        assert!(lfo.signals[0].iter().all(|x| (-1.0..=1.0).contains(x)));
    }
}
