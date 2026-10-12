//! `std.gain`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, parameter, signal_port};

/// The lowest gain, which is silence.
const FLOOR_DB: f64 = -120.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.gain",
    doc: "Gain in decibels: out = in·10^(db/20), exactly 0 at -120 dB.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[parameter(
        "db",
        PortKind::Control,
        Some("dB"),
        FLOOR_DB,
        24.0,
        0.0,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.gain`: a gain set in decibels.
///
/// Gains elsewhere are linear factors (`docs/dsp-conventions.md`, "Units");
/// this primitive exists to take decibels, which is why its parameter's unit
/// is `dB`.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `out` (signal out, audio).
///
/// **Parameters**
/// - `db` (control, dB, range `[−120, 24]`, default 0, combine `add`).
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = in[n]·10^(db[n]/20)`, with `pow` from `libm`; at
/// −120 dB (the bottom of the range) the factor is exactly 0, so the output is
/// silence rather than −120 dB. 0 dB is exactly the identity. Stateless;
/// constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Gain;

/// The linear factor of `db`.
fn factor(db: f64) -> f64 {
    if db <= FLOOR_DB {
        0.0
    } else {
        libm::pow(10.0, db / 20.0)
    }
}

impl DirectPrimitive for Gain {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (x, db) = (inputs.signal(0), inputs.parameter(0));
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = at(x, n, 0.0) * factor(at(db, n, 0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn gain(x: &[f64], db: f64) -> Vec<f64> {
        let mut gain = Gain;
        gain.prepare(&context(48_000.0, 16, 0));
        let db = vec![db; x.len()];
        let run = render(
            &mut gain,
            &Timeline {
                signals: &[x],
                parameters: &[&db],
                triggers: &[],
            },
            x.len(),
            16,
        );
        run.signals.into_iter().next().unwrap()
    }

    /// AC2c: 0 dB is the identity, −6.0206 dB halves, −120 dB is exactly 0.
    #[test]
    fn gain_at_zero_minus_six_and_minus_one_twenty_db() {
        let x: Vec<f64> = (0..50).map(|n| libm::sin(n as f64 * 0.37) * 3.0).collect();
        assert_eq!(gain(&x, 0.0), x);
        for (y, x) in gain(&x, -6.0206).iter().zip(&x) {
            approx::assert_relative_eq!(*y, 0.5 * x, max_relative = 1e-5);
            assert_eq!(*y, x * libm::pow(10.0, -6.0206 / 20.0));
        }
        let silent = gain(&x, -120.0);
        assert!(silent.iter().all(|&y| y == 0.0));
        // Just above the floor it is −119.9 dB, not silence.
        let quiet = gain(&[1.0], -119.9);
        approx::assert_relative_eq!(quiet[0], 1.0116e-6, max_relative = 1e-4);
        approx::assert_relative_eq!(gain(&[1.0], 24.0)[0], 15.848_931_924_611_133);
    }
}
