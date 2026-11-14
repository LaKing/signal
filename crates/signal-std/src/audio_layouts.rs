//! The audio sinks of ADR 0038 §2's other layouts, and their taps:
//! `std.audio_out_mono`, `std.audio_out_quad`, `std.audio_out_5_1`,
//! `std.audio_out_7_1` and `std.audio_tap_mono`, `std.audio_tap_quad`,
//! `std.audio_tap_5_1`, `std.audio_tap_7_1`.
//!
//! Each is `std.audio_out` (or `std.audio_tap`) with its layout's channels
//! in place of `l`, `r`, in the layout's canonical order (the order of a
//! render's WAV channels): `m`; `l r lb rb`; `l r c lfe ls rs`;
//! `l r c lfe lb rb ls rs`. A device slot `{ play = "5.1" }` plans as
//! `std.audio_out_5_1`; inside an included project it is its tap (ADR 0035
//! §3). The driver records each input times `gain`, as for `std.audio_out`;
//! what a machine's layout makes of it is the output adapter's (ADR 0038
//! §5), never the plan's.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, SinkKind,
};

use crate::{at, parameter, signal_port};

/// `gain`, as `std.audio_out`'s.
const GAIN: signal_core::ParameterDescriptor = parameter(
    "gain",
    PortKind::Gain,
    None,
    0.0,
    4.0,
    1.0,
    Combine::Multiply,
);

/// A sink of `$channels` and its tap of the same channels with `_out`
/// outputs.
macro_rules! layout {
    ($sink:ident, $sink_name:literal, $tap:ident, $tap_name:literal, $layout:literal,
     [$($channel:literal),+], [$($out:literal),+]) => {
        #[doc = concat!("`", $sink_name, "`: the ", $layout, " audio sink (ADR 0038 §2): the driver records `gain` times each input, in the layout's order.")]
        #[derive(Clone, Debug, Default)]
        pub struct $sink;

        impl $sink {
            const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
                type_name: $sink_name,
                doc: concat!($layout, " audio sink (ADR 0038 §2): the driver records gain times each channel."),
                ports: &[$(signal_port($channel, Direction::In, PortKind::Audio, None, None)),+],
                parameters: &[GAIN],
                feedthrough: Feedthrough::Direct,
                sink: Some(SinkKind::Audio),
                source: None,
                latency: None,
                reproducibility: signal_core::Reproducibility::BitExact,
                category: Some(crate::category::IO),
            };
        }

        impl DirectPrimitive for $sink {
            fn describe() -> ModuleDescriptor {
                Self::DESCRIPTOR
            }

            fn prepare(&mut self, _ctx: &PrepareContext) {}

            fn reset(&mut self) {}

            fn process(&mut self, _inputs: &Inputs<'_>, _outputs: &mut Outputs<'_>) {}
        }

        #[doc = concat!("`", $tap_name, "`: the tap of `", $sink_name, "` (ADR 0035 §3): each output is `gain` times its input, as `std.audio_tap`'s.")]
        #[derive(Clone, Debug, Default)]
        pub struct $tap;

        impl $tap {
            const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
                type_name: $tap_name,
                doc: concat!("The tap of ", $sink_name, " (ADR 0035 §3): what the audio sink would record, as outputs <channel>_out = gain·<channel>."),
                ports: &[
                    $(signal_port($channel, Direction::In, PortKind::Audio, None, None),)+
                    $(signal_port($out, Direction::Out, PortKind::Audio, None, None)),+
                ],
                parameters: &[GAIN],
                feedthrough: Feedthrough::Direct,
                sink: None,
                source: None,
                latency: None,
                reproducibility: signal_core::Reproducibility::BitExact,
                category: Some(crate::category::IO),
            };
        }

        impl DirectPrimitive for $tap {
            fn describe() -> ModuleDescriptor {
                Self::DESCRIPTOR
            }

            fn prepare(&mut self, _ctx: &PrepareContext) {}

            fn reset(&mut self) {}

            fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
                let gain = inputs.parameter(0);
                for channel in 0..[$($channel),+].len() {
                    let x = inputs.signal(channel);
                    for (n, out) in outputs.signal(channel).iter_mut().enumerate() {
                        *out = at(gain, n, 1.0) * at(x, n, 0.0);
                    }
                }
            }
        }
    };
}

layout!(
    AudioOutMono,
    "std.audio_out_mono",
    AudioTapMono,
    "std.audio_tap_mono",
    "Mono",
    ["m"],
    ["m_out"]
);
layout!(
    AudioOutQuad,
    "std.audio_out_quad",
    AudioTapQuad,
    "std.audio_tap_quad",
    "Quad",
    ["l", "r", "lb", "rb"],
    ["l_out", "r_out", "lb_out", "rb_out"]
);
layout!(
    AudioOut51,
    "std.audio_out_5_1",
    AudioTap51,
    "std.audio_tap_5_1",
    "5.1",
    ["l", "r", "c", "lfe", "ls", "rs"],
    ["l_out", "r_out", "c_out", "lfe_out", "ls_out", "rs_out"]
);
layout!(
    AudioOut71,
    "std.audio_out_7_1",
    AudioTap71,
    "std.audio_tap_7_1",
    "7.1",
    ["l", "r", "c", "lfe", "lb", "rb", "ls", "rs"],
    [
        "l_out", "r_out", "c_out", "lfe_out", "lb_out", "rb_out", "ls_out", "rs_out"
    ]
);

/// Each layout's sink and its tap, by type name (ADR 0038 §2).
pub const SINKS: [(&str, &str); 4] = [
    ("std.audio_out_mono", "std.audio_tap_mono"),
    ("std.audio_out_quad", "std.audio_tap_quad"),
    ("std.audio_out_5_1", "std.audio_tap_5_1"),
    ("std.audio_out_7_1", "std.audio_tap_7_1"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// A 5.1 tap's outputs are its gained inputs, channel for channel.
    #[test]
    fn a_tap_s_outputs_are_its_gained_inputs() {
        let channels: Vec<[f64; 2]> = (0..6).map(|i| [i as f64, -(i as f64) - 0.5]).collect();
        let signals: Vec<&[f64]> = channels.iter().map(|c| &c[..]).collect();
        let mut tap = AudioTap51;
        tap.prepare(&context(48_000.0, 2, 0));
        let run = render(
            &mut tap,
            &Timeline {
                signals: &signals,
                ..Timeline::default()
            },
            2,
            1,
        );
        for (i, out) in run.signals.iter().enumerate() {
            assert_eq!(out, &channels[i], "channel {i}");
        }
    }

    /// The sinks' inputs are their layouts' channels, in order (ADR 0038 §2).
    #[test]
    fn the_sinks_name_their_layouts_channels() {
        let names = |d: ModuleDescriptor| d.ports.iter().map(|p| p.name).collect::<Vec<_>>();
        assert_eq!(names(AudioOutMono::describe()), ["m"]);
        assert_eq!(names(AudioOutQuad::describe()), ["l", "r", "lb", "rb"]);
        assert_eq!(
            names(AudioOut51::describe()),
            ["l", "r", "c", "lfe", "ls", "rs"]
        );
        assert_eq!(
            names(AudioOut71::describe()),
            ["l", "r", "c", "lfe", "lb", "rb", "ls", "rs"]
        );
    }
}
