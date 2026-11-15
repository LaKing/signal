//! `std.device_out<N>` and `std.device_out_tap<N>` (ADR 0038 §2, §3, §7):
//! the play side of a discrete layout, *N* channels with no spatial meaning
//! (stems), `N` from 1 to [`MAX_DEVICE_WIDTH`].
//!
//! - `std.device_out<N>`: an audio sink with inputs `c1 … cN` and `gain`;
//!   the driver records `gain` times each input, as for `std.audio_out`.
//! - `std.device_out_tap<N>`: its tap (ADR 0035 §3), registered with
//!   `Registry::set_sink_tap`: outputs `c1_out … cN_out`, each `gain` times
//!   its input, as `std.audio_tap`'s.
//!
//! A discrete device slot (`play = ["kick", "snare"]`) plans as a composite
//! that exports the written names onto one of these (`signal-project`):
//! the plan knows widths, never the names' meaning (hard rule 1).
//!
//! **Registration.** As the vector types' (`vector.rs`), each width's
//! descriptor and its channel names are made once per process, the first
//! time [`types`] is asked, and leaked.

use std::sync::OnceLock;

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PrepareContext, Primitive, SinkKind,
};

use crate::{at, signal_port};

/// The widest discrete layout (ADR 0038 §2).
pub const MAX_DEVICE_WIDTH: usize = 64;

/// The type name of the discrete sink of `width` channels.
pub fn sink_name(width: usize) -> String {
    format!("std.device_out{width}")
}

/// The type name of the tap of the discrete sink of `width` channels.
pub fn tap_name(width: usize) -> String {
    format!("std.device_out_tap{width}")
}

/// Every width's sink and tap, sinks first.
pub(crate) fn types() -> &'static [ModuleType] {
    static TYPES: OnceLock<Vec<ModuleType>> = OnceLock::new();
    TYPES.get_or_init(make_types)
}

/// Each width's sink and its tap, by type name.
pub(crate) fn sink_taps() -> impl Iterator<Item = (&'static str, &'static str)> {
    let types = types();
    let (sinks, taps) = types.split_at(MAX_DEVICE_WIDTH);
    sinks
        .iter()
        .zip(taps)
        .map(|(s, t)| (s.descriptor.type_name, t.descriptor.type_name))
}

/// The leaked names `c1 … c64` (inputs) or `c1_out … c64_out` (outputs).
pub(crate) fn channel_names(out: bool) -> &'static [&'static str] {
    static IN: OnceLock<Vec<&'static str>> = OnceLock::new();
    static OUT: OnceLock<Vec<&'static str>> = OnceLock::new();
    let names = if out { &OUT } else { &IN };
    names.get_or_init(|| {
        (1..=MAX_DEVICE_WIDTH)
            .map(|k| {
                let name = if out {
                    format!("c{k}_out")
                } else {
                    format!("c{k}")
                };
                &*Box::leak(name.into_boxed_str())
            })
            .collect()
    })
}

/// The descriptor of the sink (`tap` false) or the tap of `width`.
fn descriptor(width: usize, tap: bool) -> ModuleDescriptor {
    let port = |name, direction| signal_port(name, direction, PortKind::Audio, None, None);
    let mut ports: Vec<PortDescriptor> = channel_names(false)
        .get(..width)
        .unwrap_or_default()
        .iter()
        .map(|&c| port(c, Direction::In))
        .collect();
    if tap {
        ports.extend(
            channel_names(true)
                .get(..width)
                .unwrap_or_default()
                .iter()
                .map(|&c| port(c, Direction::Out)),
        );
    }
    let (name, doc) = if tap {
        (
            tap_name(width),
            format!(
                "The tap of std.device_out{width} (ADR 0035 §3): what the discrete sink would record, as outputs c<k>_out = gain·c<k>."
            ),
        )
    } else {
        (
            sink_name(width),
            format!(
                "Discrete audio sink of {width} channel{} (ADR 0038 §2): the driver records gain times each channel, never mixed.",
                if width == 1 { "" } else { "s" }
            ),
        )
    };
    ModuleDescriptor {
        type_name: Box::leak(name.into_boxed_str()),
        doc: Box::leak(doc.into_boxed_str()),
        ports: Box::leak(ports.into_boxed_slice()),
        parameters: std::slice::from_ref(&crate::audio_layouts::GAIN),
        feedthrough: Feedthrough::Direct,
        sink: (!tap).then_some(SinkKind::Audio),
        source: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: Some(crate::category::IO),
    }
}

/// The registered descriptor of the sink or tap of `N`.
fn describe<const N: usize>(tap: bool) -> ModuleDescriptor {
    let index = N - 1 + if tap { MAX_DEVICE_WIDTH } else { 0 };
    types()
        .get(index)
        .map_or_else(|| descriptor(N, tap), |t| t.descriptor)
}

/// `std.device_out<N>`: the discrete sink; the driver reads its inputs.
#[derive(Clone, Debug, Default)]
pub struct DeviceOut<const N: usize>;

impl<const N: usize> DirectPrimitive for DeviceOut<N> {
    fn describe() -> ModuleDescriptor {
        describe::<N>(false)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, _inputs: &Inputs<'_>, _outputs: &mut Outputs<'_>) {}
}

/// `std.device_out_tap<N>`: `c<k>_out = gain·c<k>`.
#[derive(Clone, Debug, Default)]
pub struct DeviceOutTap<const N: usize>;

impl<const N: usize> DirectPrimitive for DeviceOutTap<N> {
    fn describe() -> ModuleDescriptor {
        describe::<N>(true)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let gain = inputs.parameter(0);
        for channel in 0..N {
            let x = inputs.signal(channel);
            for (n, out) in outputs.signal(channel).iter_mut().enumerate() {
                *out = at(gain, n, 1.0) * at(x, n, 0.0);
            }
        }
    }
}

/// The sink and the tap of one width.
fn family<const N: usize>() -> (ModuleType, ModuleType) {
    (
        ModuleType {
            descriptor: descriptor(N, false),
            create: || Primitive::Direct(Box::new(DeviceOut::<N>)),
        },
        ModuleType {
            descriptor: descriptor(N, true),
            create: || Primitive::Direct(Box::new(DeviceOutTap::<N>)),
        },
    )
}

macro_rules! widths {
    ($($w:literal),*) => {
        fn make_types() -> Vec<ModuleType> {
            let families = [$(family::<$w>()),*];
            const { assert!([$($w),*].len() == MAX_DEVICE_WIDTH) };
            let (sinks, taps): (Vec<_>, Vec<_>) = families.into_iter().unzip();
            sinks.into_iter().chain(taps).collect()
        }
    };
}

widths!(
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50,
    51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    #[test]
    fn every_width_has_a_sink_and_its_tap() {
        let pairs: Vec<_> = sink_taps().collect();
        assert_eq!(pairs.len(), MAX_DEVICE_WIDTH);
        assert_eq!(pairs[0], ("std.device_out1", "std.device_out_tap1"));
        assert_eq!(pairs[63], ("std.device_out64", "std.device_out_tap64"));
        let four = DeviceOut::<4>::describe();
        assert_eq!(four.type_name, "std.device_out4");
        let names: Vec<_> = four.ports.iter().map(|p| p.name).collect();
        assert_eq!(names, ["c1", "c2", "c3", "c4"]);
        assert_eq!(four.sink, Some(SinkKind::Audio));
        let tap = DeviceOutTap::<2>::describe();
        let names: Vec<_> = tap.ports.iter().map(|p| p.name).collect();
        assert_eq!(names, ["c1", "c2", "c1_out", "c2_out"]);
        assert_eq!(tap.sink, None);
    }

    #[test]
    fn a_tap_s_outputs_are_its_gained_inputs() {
        let channels: Vec<[f64; 2]> = (0..3).map(|i| [i as f64, -(i as f64) - 0.5]).collect();
        let signals: Vec<&[f64]> = channels.iter().map(|c| &c[..]).collect();
        let mut tap = DeviceOutTap::<3>;
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
}
