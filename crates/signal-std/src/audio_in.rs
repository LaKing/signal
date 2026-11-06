//! `std.audio_in` and `std.audio_in_mono`: the capture side of a device
//! slot (ADR 0038 §3, §7), sources whose outputs the driver writes.

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, SourceKind,
};

use crate::{at, signal_port};

const STEREO: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.audio_in",
    doc: "Audio input (ADR 0038): the stereo capture side of a device slot; l and r are what the driver captured (a device's channels live, the recorded input in a replay, the slot's render file offline), 0 where it gave nothing.",
    ports: &[
        signal_port("l", Direction::Out, PortKind::Audio, None, None),
        signal_port("r", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    source: Some(SourceKind::Audio),
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::IO),
};

const MONO: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.audio_in_mono",
    doc: "Audio input (ADR 0038): the mono capture side of a device slot; m is what the driver captured, 0 where it gave nothing.",
    ports: &[signal_port(
        "m",
        Direction::Out,
        PortKind::Audio,
        None,
        None,
    )],
    ..STEREO
};

/// `std.audio_in`: the stereo capture side of a device slot (ADR 0038 §3,
/// §7), what `{ device = { capture = "stereo" } }` plans. A **source**
/// ([`SourceKind::Audio`]): the engine hands it what the driver wrote for
/// the block (ADR 0038 §7, frame phase 1), and it puts that on its outputs.
///
/// **Ports**
/// - `l`, `r` (signal out, audio): live, the device channels the machine
///   binds the slot's role to (`[capture.<role>]`); in a replay, the
///   recorded input (`inputs/<slot>.f32`); offline, the slot's `render`
///   file, else 0.
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct (no inputs).
///
/// **Formula:** `l[n] = external_l[n]`, `r[n] = external_r[n]`; 0 where the
/// driver wrote nothing. Checked like any output (`docs/semantics.md` §7):
/// a non-finite value faults the slot. Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct AudioIn;

/// `std.audio_in_mono`: [`AudioIn`] with one output, `m`.
#[derive(Clone, Debug, Default)]
pub struct AudioInMono;

fn copy(inputs: &Inputs<'_>, outputs: &mut Outputs<'_>, rows: usize) {
    for row in 0..rows {
        let x = inputs.external(row);
        for (n, out) in outputs.signal(row).iter_mut().enumerate() {
            *out = at(x, n, 0.0);
        }
    }
}

impl DirectPrimitive for AudioIn {
    fn describe() -> ModuleDescriptor {
        STEREO
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        copy(inputs, outputs, 2);
    }
}

impl DirectPrimitive for AudioInMono {
    fn describe() -> ModuleDescriptor {
        MONO
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        copy(inputs, outputs, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// Without external rows (as every test harness and a driver that
    /// writes none), the outputs read 0.
    #[test]
    fn without_external_rows_the_outputs_read_0() {
        let mut input = AudioIn;
        input.prepare(&context(48_000.0, 4, 0));
        let run = render(&mut input, &Timeline::default(), 4, 3);
        assert_eq!(run.signals[0], [0.0; 4]);
        assert_eq!(run.signals[1], [0.0; 4]);
    }
}
