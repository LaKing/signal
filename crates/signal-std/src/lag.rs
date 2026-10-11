//! `std.lag`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{FLUSH_THRESHOLD, at, parameter, signal_port};

const RISE_MS: f64 = 10.0;
const FALL_MS: f64 = 10.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.lag",
    doc: "One-pole lag with separate rise and fall times: y += (x - y)·(1 - e^(-1/τ)), τ = ms·sr/1000.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Control, None, None),
        signal_port("out", Direction::Out, PortKind::Control, None, None),
    ],
    parameters: &[
        parameter(
            "rise_ms",
            PortKind::Control,
            Some("ms"),
            0.0,
            60_000.0,
            RISE_MS,
            Combine::Add,
        ),
        parameter(
            "fall_ms",
            PortKind::Control,
            Some("ms"),
            0.0,
            60_000.0,
            FALL_MS,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
};

/// `std.lag`: a one-pole lag (slew) whose time constant depends on the
/// direction of the change.
///
/// **Ports**
/// - `in` (signal in, control, unbounded); unconnected it reads 0.
/// - `out` (signal out, control).
///
/// **Parameters**
/// - `rise_ms` (control, ms, range `[0, 60 000]`, default 10, combine `add`):
///   used on frames where `in[n] > y[n−1]`.
/// - `fall_ms` (control, ms, range `[0, 60 000]`, default 10, combine `add`):
///   used on every other frame.
///
/// **Feedthrough:** direct (`in[n]` reaches `out[n]`).
///
/// **Formula** (x = `in`, y = `out`, y\[−1\] = 0, sr the sample rate):
///
/// ```text
/// ms   = rise_ms[n] if x[n] > y[n−1], else fall_ms[n]
/// τ    = ms·sr/1000                                   frames
/// y[n] = x[n]                                         if τ = 0
/// y[n] = y[n−1] + (x[n] − y[n−1])·(1 − e^(−1/τ))      otherwise
/// ```
///
/// 0 ms passes the input through unchanged. The step response from 0 to a
/// constant `x` is `y[n] = x·(1 − e^(−(n+1)/τ))`. When `|x[n] − y[n]|` falls
/// below 1e-9, `y[n]` is set to exactly `x[n]`: the decaying difference is
/// flushed, so a constant input is reached exactly and a decay to 0 ends at
/// exactly 0 (`docs/dsp-conventions.md`, "Denormals"). `exp` is `libm::exp`.
/// Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Lag {
    sample_rate: f64,
    /// `y[n−1]`.
    y: f64,
}

impl DirectPrimitive for Lag {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.y = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let rise = inputs.parameter(0);
        let fall = inputs.parameter(1);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let xn = at(x, n, 0.0);
            let ms = if xn > self.y {
                at(rise, n, RISE_MS)
            } else {
                at(fall, n, FALL_MS)
            };
            let tau = ms * self.sample_rate / 1000.0;
            if tau == 0.0 {
                self.y = xn;
            } else {
                let y = self.y + (xn - self.y) * (1.0 - libm::exp(-1.0 / tau));
                self.y = if (xn - y).abs() < FLUSH_THRESHOLD {
                    xn
                } else {
                    y
                };
            }
            *out = self.y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn lag(input: &[f64], rise: &[f64], fall: &[f64], sample_rate: f64) -> Vec<f64> {
        let mut lag = Lag::default();
        lag.prepare(&context(sample_rate, 128, 0));
        let run = render(
            &mut lag,
            &Timeline {
                signals: &[input],
                parameters: &[rise, fall],
                triggers: &[],
            },
            input.len(),
            128,
        );
        run.signals.into_iter().next().unwrap()
    }

    /// AC5a: a step up and back down follows the closed form, with the rise
    /// time going up and the fall time going down (y\[−1\] = 0).
    #[test]
    fn step_response_follows_the_closed_form() {
        let (sample_rate, frames, up) = (48_000.0, 20_000, 500);
        let mut input = vec![0.0; frames];
        input[..up].fill(0.8);
        // rise 1 ms: τ = 48 frames; fall 5 ms: τ = 240 frames.
        let out = lag(&input, &vec![1.0; frames], &vec![5.0; frames], sample_rate);
        for n in [0, 1, 10, 47, 100, 300, 499] {
            let expected = 0.8 * (1.0 - libm::exp(-((n + 1) as f64) / 48.0));
            approx::assert_relative_eq!(out[n], expected, max_relative = 1e-12);
        }
        let top = out[up - 1];
        for j in [0, 1, 10, 239, 1000, 3000] {
            let expected = top * libm::exp(-((j + 1) as f64) / 240.0);
            approx::assert_relative_eq!(out[up + j], expected, max_relative = 1e-12);
        }
        // The decay to 0 is flushed to exactly 0 once it is below 1e-9.
        let zero = out.iter().rposition(|&y| y != 0.0).unwrap() + 1;
        assert!(out[zero - 1] >= 1e-9 && zero < frames);
        assert!(out[zero..].iter().all(|&y| y == 0.0));
    }

    /// AC5b: 0 ms is the identity, in both directions.
    #[test]
    fn zero_ms_is_the_identity() {
        let frames = 5000;
        let input: Vec<f64> = (0..frames)
            .map(|n| libm::sin(n as f64 * 0.37) * ((n % 11) as f64 - 5.0))
            .collect();
        let zeros = vec![0.0; frames];
        assert_eq!(lag(&input, &zeros, &zeros, 48_000.0), input);

        // Rise 0 only: rising steps pass through, falling ones lag.
        let out = lag(&[1.0, 2.0, 0.0], &[0.0; 3], &[1.0; 3], 1000.0);
        assert_eq!(&out[..2], [1.0, 2.0]);
        assert_eq!(out[2], 2.0 + (0.0 - 2.0) * (1.0 - libm::exp(-1.0)));
    }

    #[test]
    fn a_constant_input_is_reached_exactly() {
        let frames = 2000;
        let out = lag(
            &vec![0.3; frames],
            &vec![1.0; frames],
            &vec![1.0; frames],
            48_000.0,
        );
        assert_eq!(*out.last().unwrap(), 0.3);
        assert!(out.iter().all(|&y| y <= 0.3));
    }
}
