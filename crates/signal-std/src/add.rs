//! `std.add`.

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, signal_port};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.add",
    doc: "Adds its two signal inputs: out = a + b.",
    ports: &[
        signal_port("a", Direction::In, PortKind::Generic, None, None),
        signal_port("b", Direction::In, PortKind::Generic, None, None),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.add`: the sum of two signals.
///
/// **Ports**
/// - `a`, `b` (signal in, generic, unbounded); unconnected they read 0.
/// - `out` (signal out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = a[n] + b[n]`. Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Add;

impl DirectPrimitive for Add {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (a, b) = (inputs.signal(0), inputs.signal(1));
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = at(a, n, 0.0) + at(b, n, 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    #[test]
    fn out_is_the_sum() {
        let a = [1.5, -2.0, 0.0, 3.0];
        let b = [2.0, 0.25, 7.0, -1.0];
        let mut add = Add;
        add.prepare(&context(48_000.0, 4, 0));
        let run = render(
            &mut add,
            &Timeline {
                signals: &[&a, &b],
                ..Timeline::default()
            },
            4,
            3,
        );
        assert_eq!(run.signals[0], [3.5, -1.75, 7.0, 2.0]);
    }
}
