//! `std.pan`.

use std::f64::consts::FRAC_PI_4;

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, parameter, signal_port};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.pan",
    doc: "Equal-power panner: θ = (pan + 1)·π/4, l = in·cos θ, r = in·sin θ.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("l", Direction::Out, PortKind::Audio, None, None),
        signal_port("r", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[parameter(
        "pan",
        PortKind::Control,
        None,
        -1.0,
        1.0,
        0.0,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.pan`: an equal-power panner from one signal to two.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `l`, `r` (signal out, audio): the left and right signals.
///
/// **Parameters**
/// - `pan` (control, no unit, range `[−1, 1]`, default 0, combine `add`): −1 is
///   hard left, 0 the centre, 1 hard right.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `θ = (pan[n] + 1)·π/4`, `l[n] = in[n]·cos θ`, `r[n] = in[n]·sin θ`,
/// so `l² + r² = in²` at every position (equal power); in the centre each side
/// is `in·cos(π/4) ≈ 0.7071·in` (−3 dB). `cos` and `sin` are `libm`'s.
/// Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Pan;

impl DirectPrimitive for Pan {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (x, pan) = (inputs.signal(0), inputs.parameter(0));
        let mut sides = outputs.signals();
        let (Some(left), Some(right)) = (sides.next(), sides.next()) else {
            return;
        };
        for (n, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let theta = (at(pan, n, 0.0) + 1.0) * FRAC_PI_4;
            let xn = at(x, n, 0.0);
            *l = xn * libm::cos(theta);
            *r = xn * libm::sin(theta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// AC2c: l² + r² = in² across the whole pan range, hard left and right are
    /// exact, and the centre is cos(π/4) on both sides.
    #[test]
    fn the_pan_law_is_equal_power() {
        let frames = 2001;
        let pan: Vec<f64> = (0..frames).map(|n| n as f64 / 1000.0 - 1.0).collect();
        let x: Vec<f64> = (0..frames)
            .map(|n| libm::sin(n as f64 * 0.37) * 1.7)
            .collect();
        let mut panner = Pan;
        panner.prepare(&context(48_000.0, 64, 0));
        let run = render(
            &mut panner,
            &Timeline {
                signals: &[&x],
                parameters: &[&pan],
                triggers: &[],
            },
            frames,
            64,
        );
        let (l, r) = (&run.signals[0], &run.signals[1]);
        for n in 0..frames {
            approx::assert_relative_eq!(
                l[n] * l[n] + r[n] * r[n],
                x[n] * x[n],
                max_relative = 1e-14
            );
        }
        // pan −1: θ = 0, all left; pan 1: θ = π/2, r = in (cos(π/2) ≈ 6e-17).
        assert_eq!((l[0], r[0]), (x[0], 0.0));
        assert_eq!(r[2000], x[2000]);
        assert!(l[2000].abs() < 1e-15 * x[2000].abs().max(1.0));
        // pan 0 at frame 1000: both sides cos(π/4)·in.
        assert_eq!(l[1000], x[1000] * libm::cos(FRAC_PI_4));
        approx::assert_relative_eq!(l[1000], r[1000], max_relative = 1e-15);
    }
}
