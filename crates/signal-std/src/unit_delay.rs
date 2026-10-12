//! `std.unit_delay`.

use signal_core::{
    Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind, PrepareContext,
    StateOutputPrimitive,
};

use crate::signal_port;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.unit_delay",
    doc: "Delays its input by exactly one frame (z^-1); breaks feedback cycles.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Generic, None, None),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::StateOutput,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.unit_delay`: a one-frame delay, z⁻¹.
///
/// **Ports**
/// - `in` (signal in, generic, unbounded); unconnected it reads 0.
/// - `out` (signal out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** state-output: `out[n]` depends on state\[n−1\] only, so a
/// unit delay may close a feedback cycle (ADR 0016 §2).
///
/// **Formula:** `out[n] = in[n−1]`, `out[0] = 0`. `emit` writes the stored
/// sample, `absorb` stores `in[n]`. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct UnitDelay {
    last: f64,
}

impl StateOutputPrimitive for UnitDelay {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.last = 0.0;
    }

    fn reset(&mut self) {
        self.last = 0.0;
    }

    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(self.last);
    }

    fn absorb(&mut self, inputs: &Inputs<'_>) {
        if let Some(&x) = inputs.signal(0).last() {
            self.last = x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::context;

    /// AC10: the output is the input shifted by exactly one frame, starting at 0.
    #[test]
    fn shifts_by_exactly_one_frame() {
        let input: Vec<f64> = (0..1000)
            .map(|n| libm::sin(n as f64 * 0.37) + n as f64)
            .collect();
        let mut delay = UnitDelay::default();
        delay.prepare(&context(48_000.0, 1, 0));
        let mut output = Vec::new();
        for &x in &input {
            let mut out = [f64::NAN];
            delay.emit(&mut Outputs::new(1, &mut out, &mut []));
            delay.absorb(&Inputs::new(1, &[x], &[], &[], &[]));
            output.push(out[0]);
        }
        assert_eq!(output[0].to_bits(), 0.0f64.to_bits());
        for n in 1..input.len() {
            assert_eq!(output[n].to_bits(), input[n - 1].to_bits(), "frame {n}");
        }
    }
}
