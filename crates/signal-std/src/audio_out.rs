//! `std.audio_out`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, SinkKind,
};

use crate::{parameter, signal_port};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.audio_out",
    doc: "Stereo audio sink: the driver records gain·l and gain·r.",
    ports: &[
        signal_port("l", Direction::In, PortKind::Audio, None, None),
        signal_port("r", Direction::In, PortKind::Audio, None, None),
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
    sink: Some(SinkKind::Audio),
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::IO),
};

/// `std.audio_out`: the stereo audio sink (`sink = Audio`). A plan has exactly
/// one; the driver records what it receives.
///
/// **Ports**
/// - `l`, `r` (signal in, audio, unbounded): the left and right channels, in
///   that order; unconnected they read 0.
/// - No outputs.
///
/// **Parameters**
/// - `gain` (gain, linear, no unit, range `[0, 4]`, default 1, combine `multiply`).
///
/// **Feedthrough:** direct.
///
/// **Formula:** channel 0 is `gain[n]·l[n]`, channel 1 is `gain[n]·r[n]`.
///
/// **How the driver gets the gained values.** A sink has no outputs, and the
/// offline driver records the sink's signal inputs as the executor sums them
/// (M1-4, `docs/semantics.md` §8). The gain is therefore applied **by the
/// driver**: when the sink's type declares a parameter named `gain`, each
/// recorded input is multiplied by that parameter's effective value in the same
/// frame, as the executor computed it for this instance. `process` does nothing:
/// the module has no state and nothing to write. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct AudioOut;

impl DirectPrimitive for AudioOut {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, _inputs: &Inputs<'_>, _outputs: &mut Outputs<'_>) {}
}
