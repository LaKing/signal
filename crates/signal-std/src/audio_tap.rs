//! `std.audio_tap`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, parameter, signal_port};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.audio_tap",
    doc: "The tap of std.audio_out (ADR 0035 §3): what the audio sink would record, as outputs: l_out = gain·l, r_out = gain·r.",
    ports: &[
        signal_port("l", Direction::In, PortKind::Audio, None, None),
        signal_port("r", Direction::In, PortKind::Audio, None, None),
        signal_port("l_out", Direction::Out, PortKind::Audio, None, None),
        signal_port("r_out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[parameter(
        "gain",
        PortKind::Gain,
        None,
        0.0,
        4.0,
        1.0,
        Combine::Multiply,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::IO),
};

/// `std.audio_tap`: the tap of `std.audio_out` (ADR 0035 §3), registered as
/// its tap (`Registry::sink_tap`). A project included in another project
/// has every `std.audio_out` planned as a `std.audio_tap` at the same
/// address, with the same inputs, parameters and connections: the host's
/// own `std.audio_out` stays the plan's one audio sink, and the included
/// project's output becomes signals the host wires where it wants them.
///
/// **Ports**
/// - `l`, `r` (signal in, audio, unbounded): as `std.audio_out`'s;
///   unconnected they read 0.
/// - `l_out`, `r_out` (signal out, audio): what the audio adapter would
///   have recorded from a `std.audio_out` in its place.
///
/// **Parameters**
/// - `gain` (gain, linear, no unit, range `[0, 4]`, default 1, combine
///   `multiply`), as `std.audio_out`'s.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `l_out[n] = gain[n]·l[n]`, `r_out[n] = gain[n]·r[n]`: the
/// adapter's expression on the same parameter row (`docs/semantics.md` §8),
/// so `l_out` and `r_out` are, as numbers, the included project's own
/// `audio.wav` before its conversion to `f32` (a −0 stays −0). Nothing is
/// clamped: the clamp to [−1, 1] happens once, at the device. Unlike a sink,
/// whose inputs are recorded whatever they hold, the tap's outputs are
/// checked like any output (`docs/semantics.md` §7): a non-finite value, or
/// one beyond 10⁶, faults the tap. Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct AudioTap;

impl DirectPrimitive for AudioTap {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let gain = inputs.parameter(0);
        for channel in 0..2 {
            let x = inputs.signal(channel);
            for (n, out) in outputs.signal(channel).iter_mut().enumerate() {
                *out = at(gain, n, 1.0) * at(x, n, 0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// The outputs are the gained inputs, a −0 kept.
    #[test]
    fn the_outputs_are_the_gained_inputs() {
        let l = [1.0, -0.5, -0.0, 0.25];
        let r = [0.0, 2.0, -1.0, 0.125];
        let mut tap = AudioTap;
        tap.prepare(&context(48_000.0, 4, 0));
        let run = render(
            &mut tap,
            &Timeline {
                signals: &[&l, &r],
                ..Timeline::default()
            },
            4,
            3,
        );
        assert_eq!(run.signals[0], [1.0, -0.5, -0.0, 0.25]);
        assert!(run.signals[0][2].is_sign_negative());
        assert_eq!(run.signals[1], [0.0, 2.0, -1.0, 0.125]);
    }
}
