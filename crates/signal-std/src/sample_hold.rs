//! `std.sample_hold`.

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, event_port, signal_port, triggered};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.sample_hold",
    doc: "Sample and hold: on a trigger the output takes that frame's input and holds it; initially 0.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Generic, None, None),
        event_port("trigger", Direction::In),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.sample_hold`: samples its input on each trigger and holds it.
///
/// **Ports**
/// - `in` (signal in, generic, unbounded); unconnected it reads 0.
/// - `trigger` (event in, trigger): each frame with at least one `Trigger`
///   event samples `in`; several on one frame count once. `Set` and `Ramp`
///   events are ignored.
/// - `out` (signal out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct (a trigger on frame *n* sets `out[n]` to `in[n]`).
///
/// **Formula:** `h[−1] = 0`; `h[n] = in[n]` on a trigger frame, else
/// `h[n−1]`; `out[n] = h[n]`. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct SampleHold {
    held: f64,
}

impl DirectPrimitive for SampleHold {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.held = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (x, events) = (inputs.signal(0), inputs.events(0));
        let mut next = 0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            if triggered(events, &mut next, n) {
                self.held = at(x, n, 0.0);
            }
            *out = self.held;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    #[test]
    fn holds_the_input_of_each_trigger_frame() {
        let x: Vec<f64> = (0..20).map(|n| n as f64 + 0.5).collect();
        let mut sh = SampleHold::default();
        sh.prepare(&context(48_000.0, 4, 0));
        let run = render(
            &mut sh,
            &Timeline {
                signals: &[&x],
                parameters: &[],
                triggers: &[&[3, 4, 11]],
            },
            20,
            4,
        );
        let mut expected = vec![0.0; 3];
        expected.extend([3.5; 1]);
        expected.extend([4.5; 7]);
        expected.extend([11.5; 9]);
        assert_eq!(run.signals[0], expected);
    }
}
