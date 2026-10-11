//! `std.noise`.

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, Range, RngStream,
};

use crate::signal_port;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.noise",
    doc: "White noise in [-1, 1) from the instance's seeded random stream, one draw per frame.",
    ports: &[signal_port(
        "out",
        Direction::Out,
        PortKind::Audio,
        None,
        Some(Range {
            min: -1.0,
            max: 1.0,
        }),
    )],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
};

/// `std.noise`: uniform white noise from the instance's `RngStream` (ADR 0008).
///
/// **Ports**
/// - `out` (signal out, audio, `[−1, 1)`).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct (it has no inputs).
///
/// **Formula:** `out[n] = 2·next_f64() − 1`, exactly one draw from the stream
/// per frame, so the output depends only on the master seed, the instance
/// address and the frame. `reset` restarts the stream. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Noise {
    /// The stream as `prepare` received it, and the stream being drawn from.
    /// `None` before `prepare`, when `process` outputs 0.
    streams: Option<(RngStream, RngStream)>,
}

impl DirectPrimitive for Noise {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.streams = Some((ctx.rng.clone(), ctx.rng.clone()));
    }

    fn reset(&mut self) {
        if let Some((initial, stream)) = &mut self.streams {
            stream.clone_from(initial);
        }
    }

    fn process(&mut self, _inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let out = outputs.signal(0);
        match &mut self.streams {
            Some((_, stream)) => {
                for x in out {
                    *x = 2.0 * stream.next_f64() - 1.0;
                }
            }
            None => out.fill(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render, stream};

    #[test]
    fn one_draw_per_frame_from_the_instance_stream() {
        let mut noise = Noise::default();
        noise.prepare(&context(48_000.0, 7, 9));
        let run = render(&mut noise, &Timeline::default(), 100, 7);
        let mut expected = stream(9);
        for (n, &x) in run.signals[0].iter().enumerate() {
            assert_eq!(x, 2.0 * expected.next_f64() - 1.0, "frame {n}");
            assert!((-1.0..1.0).contains(&x));
        }
    }
}
