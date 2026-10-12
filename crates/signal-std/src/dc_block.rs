//! `std.dc_block`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{FLUSH_THRESHOLD, at, parameter, signal_port};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.dc_block",
    doc: "First-order DC blocker: y[n] = x[n] - x[n-1] + R·y[n-1], R = exp(-2π·cutoff_hz/sr).",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[parameter(
        "cutoff_hz",
        PortKind::Frequency,
        Some("Hz"),
        1.0,
        200.0,
        10.0,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.dc_block`: a first-order high-pass that removes DC (a one-pole,
/// one-zero filter).
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `out` (signal out, audio).
///
/// **Parameters**
/// - `cutoff_hz` (frequency, Hz, range `[1, 200]`, default 10, combine `add`).
///
/// **Feedthrough:** direct (`in[n]` reaches `out[n]`).
///
/// **Formula** (x = `in`, y = `out`, x\[−1\] = y\[−1\] = 0, sr the sample rate):
///
/// ```text
/// R[n] = exp(−2π·cutoff_hz[n] / sr)
/// y[n] = x[n] − x[n−1] + R[n]·y[n−1]
/// ```
///
/// A constant input decays to exactly 0. The feedback state is flushed: when
/// `|y[n]| < 1e-9` it is set to exactly 0 (`docs/dsp-conventions.md`,
/// "Denormals"). `exp` is `libm::exp`. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct DcBlock {
    sample_rate: f64,
    x1: f64,
    y1: f64,
}

impl DirectPrimitive for DcBlock {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let cutoff = inputs.parameter(0);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let r = libm::exp(-std::f64::consts::TAU * at(cutoff, n, 0.0) / self.sample_rate);
            let xn = at(x, n, 0.0);
            let mut y = xn - self.x1 + r * self.y1;
            if y.abs() < FLUSH_THRESHOLD {
                y = 0.0;
            }
            self.x1 = xn;
            self.y1 = y;
            *out = y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn filter(input: &[f64], cutoff: f64, sample_rate: f64) -> Vec<f64> {
        let mut dc = DcBlock::default();
        dc.prepare(&context(sample_rate, 512, 0));
        let cutoff = vec![cutoff; input.len()];
        let run = render(
            &mut dc,
            &Timeline {
                signals: &[input],
                parameters: &[&cutoff],
                triggers: &[],
            },
            input.len(),
            512,
        );
        run.signals.into_iter().next().unwrap()
    }

    /// AC8: a constant input is removed; the output decays as R^n and ends at
    /// exactly 0.
    #[test]
    fn removes_a_constant_input() {
        for sample_rate in [44_100.0, 96_000.0] {
            let frames = 2 * sample_rate as usize;
            let out = filter(&vec![0.5; frames], 10.0, sample_rate);
            let r = libm::exp(-std::f64::consts::TAU * 10.0 / sample_rate);
            assert_eq!(out[0], 0.5);
            assert_eq!(out[1], r * 0.5);
            // The analytic step response, until the flush.
            for (n, &y) in out.iter().enumerate().take(1000) {
                let expected = 0.5 * libm::pow(r, n as f64);
                approx::assert_relative_eq!(y, expected, max_relative = 1e-12);
            }
            assert_eq!(*out.last().unwrap(), 0.0);
            let settled = out.iter().position(|&y| y == 0.0).unwrap();
            assert!(out[settled..].iter().all(|&y| y == 0.0));
        }
    }

    /// The pass band is kept: a 1 kHz sine is passed almost unchanged.
    #[test]
    fn passes_audio_frequencies() {
        let sample_rate = 48_000.0;
        let input: Vec<f64> = (0..48_000)
            .map(|n| libm::sin(std::f64::consts::TAU * 1000.0 * n as f64 / sample_rate))
            .collect();
        let out = filter(&input, 10.0, sample_rate);
        let peak = out[24_000..].iter().fold(0.0f64, |m, y| m.max(y.abs()));
        assert!((peak - 1.0).abs() < 1e-3, "peak {peak}");
    }
}
