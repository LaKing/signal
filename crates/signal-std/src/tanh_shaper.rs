//! `std.tanh_shaper`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, parameter, signal_port};

/// Below this `|u[n] − u[n−1]|` the ADAA quotient is replaced by its limit.
const EPSILON: f64 = 1e-6;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.tanh_shaper",
    doc: "tanh saturation of drive·in, antialiased with first-order ADAA (half a frame of delay).",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, crate::BIPOLAR),
    ],
    parameters: &[parameter(
        "drive",
        PortKind::Gain,
        None,
        0.1,
        20.0,
        1.0,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::FILTER),
};

/// `std.tanh_shaper`: `tanh` saturation, antialiased by first-order
/// antiderivative antialiasing (ADAA, `docs/dsp-conventions.md`).
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `out` (signal out, audio, `[−1, 1]`).
///
/// **Parameters**
/// - `drive` (gain, linear, no unit, range `[0.1, 20]`, default 1, combine `add`).
///
/// **Feedthrough:** direct (`in[n]` reaches `out[n]`).
///
/// **Formula** (u\[−1\] = 0):
///
/// ```text
/// u[n] = drive[n]·in[n]
/// F(u) = ln cosh u = |u| + ln(1 + e^(−2|u|)) − ln 2
/// out[n] = (F(u[n]) − F(u[n−1])) / (u[n] − u[n−1])     if |u[n] − u[n−1]| ≥ 1e-6
/// out[n] = tanh((u[n] + u[n−1]) / 2)                    otherwise
/// ```
///
/// `F` is the antiderivative of `tanh`, written so it does not overflow for
/// large `|u|`. The difference `F(u[n]) − F(u[n−1])` is summed as
/// `(|u[n]| − |u[n−1]|) + (ln(1 + e^(−2|u[n]|)) − ln(1 + e^(−2|u[n−1]|)))`:
/// the same value, with less cancellation. `ln(1 + ·)` is `libm::log1p`, `e^` is `libm::exp`,
/// `tanh` is `libm::tanh`. The second branch is the limit of the first as the
/// step goes to 0, so the output is finite and continuous there.
///
/// **Half-frame delay.** The quotient is the mean of `tanh` over the straight
/// line from `u[n−1]` to `u[n]`, so for a slowly varying input it equals
/// `tanh` of the midpoint: the output lags the input by half a frame
/// (≈ 5.2 µs at 96 kHz). A path that must stay time-aligned with the dry
/// signal needs the same half frame of delay. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct TanhShaper {
    /// `u[n−1]`.
    previous: f64,
}

/// `ln(1 + e^(−2|u|))`, the part of `F(u)` besides `|u| − ln 2`.
fn softplus_term(u: f64) -> f64 {
    libm::log1p(libm::exp(-2.0 * u.abs()))
}

/// The first-order ADAA of `tanh` on the step from `previous` to `u`.
///
/// `F(u) − F(previous)` is summed as `(|u| − |previous|)` plus the difference
/// of the bounded `ln(1 + ·)` terms; the `ln 2` terms cancel exactly. For a
/// small step of one sign the first part is exact, so the quotient's rounding
/// error stays near `1e-16 / |step|` even for large `|u|`.
fn adaa(u: f64, previous: f64) -> f64 {
    let step = u - previous;
    if step.abs() < EPSILON {
        libm::tanh((u + previous) / 2.0)
    } else {
        ((u.abs() - previous.abs()) + (softplus_term(u) - softplus_term(previous))) / step
    }
}

impl DirectPrimitive for TanhShaper {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.previous = 0.0;
    }

    fn reset(&mut self) {
        self.previous = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let drive = inputs.parameter(0);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let u = at(drive, n, 0.0) * at(x, n, 0.0);
            *out = adaa(u, self.previous);
            self.previous = u;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn shape(input: &[f64], drive: f64) -> Vec<f64> {
        let mut shaper = TanhShaper::default();
        shaper.prepare(&context(48_000.0, 256, 0));
        let drive = vec![drive; input.len()];
        let run = render(
            &mut shaper,
            &Timeline {
                signals: &[input],
                parameters: &[&drive],
                triggers: &[],
            },
            input.len(),
            256,
        );
        run.signals.into_iter().next().unwrap()
    }

    /// AC9: for a slow input the output is `tanh` within 1e-3 (and `tanh` of
    /// the half-frame-delayed input far more closely).
    #[test]
    fn matches_tanh_for_slow_inputs() {
        let sample_rate = 48_000.0;
        let input: Vec<f64> = (0..48_000)
            .map(|n| 1.5 * libm::sin(std::f64::consts::TAU * 2.0 * n as f64 / sample_rate))
            .collect();
        for drive in [0.1, 1.0, 3.0] {
            let out = shape(&input, drive);
            let mut previous = 0.0;
            for (n, (&x, &y)) in input.iter().zip(&out).enumerate() {
                let u = drive * x;
                assert!(
                    (y - libm::tanh(u)).abs() < 1e-3,
                    "drive {drive}, frame {n}: {y} vs tanh {}",
                    libm::tanh(u)
                );
                let midpoint = libm::tanh((u + previous) / 2.0);
                assert!((y - midpoint).abs() < 1e-7, "drive {drive}, frame {n}");
                previous = u;
            }
        }
    }

    /// AC9: as the step Δu goes to 0 the output stays finite and continuous,
    /// across the switch to the limit branch at |Δu| = 1e-6 too.
    #[test]
    fn finite_and_continuous_as_the_step_goes_to_zero() {
        for previous in [-19.0, -0.7, 0.0, 0.3, 2.0, 20.0] {
            for exponent in 0..=30 {
                for sign in [1.0, -1.0] {
                    let step = sign * libm::pow(10.0, -(exponent as f64) / 2.0);
                    let y = adaa(previous + step, previous);
                    assert!(y.is_finite(), "u[n-1] {previous}, step {step}");
                    // The mean of tanh over the step is within the step's
                    // curvature of tanh at the midpoint.
                    let midpoint = libm::tanh(previous + step / 2.0);
                    let bound = 1e-9 + step * step;
                    assert!(
                        (y - midpoint).abs() <= bound,
                        "u[n-1] {previous}, step {step}: {y} vs {midpoint}"
                    );
                }
            }
            assert_eq!(adaa(previous, previous), libm::tanh(previous));
            // Either side of the branch switch.
            let below = adaa(previous + EPSILON * (1.0 - 1e-6), previous);
            let above = adaa(previous + EPSILON * (1.0 + 1e-6), previous);
            assert!(
                (above - below).abs() < 1e-9,
                "{previous}: {below} vs {above}"
            );
        }
    }

    /// u\[−1\] = 0: the first frame is the mean of tanh over \[0, u\[0\]\].
    #[test]
    fn starts_from_zero_and_handles_a_step() {
        let out = shape(&[1.0, 1.0, 0.0, 0.0], 1.0);
        let log_cosh_1 = libm::log(libm::cosh(1.0));
        assert!((out[0] - log_cosh_1).abs() < 1e-15, "{}", out[0]);
        assert_eq!(out[1], libm::tanh(1.0));
        assert!((out[2] - log_cosh_1).abs() < 1e-15, "{}", out[2]);
        assert_eq!(out[3], 0.0);
    }
}
