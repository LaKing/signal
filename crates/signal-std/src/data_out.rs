//! `std.data_out<N>` (ADR 0057 §2): the play side of a data device slot
//! (`kind = "dmx"`), *N* channels from 1 to [`MAX_DATA_WIDTH`], whose
//! inputs a non-audio output adapter reads (`SinkKind::Data`): for Art-Net,
//! each a DMX channel's level in `[0, 1]`.
//!
//! Inputs `c1 … cN`, no parameters; a sink computes nothing. Inside an
//! included project it plans as `std.device_out_tap<N>`, whose outputs
//! `c<k>_out` are its inputs (its `gain` left at 1).
//!
//! **Registration.** As `std.device_out<N>`'s: each width's descriptor made
//! once per process and leaked, its channel names shared.

use std::sync::OnceLock;

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PrepareContext, Primitive, SinkKind,
};

use crate::signal_port;

/// The widest data slot.
pub const MAX_DATA_WIDTH: usize = crate::device_out::MAX_DEVICE_WIDTH;

/// Every width's sink, by width from 1.
pub(crate) fn types() -> &'static [ModuleType] {
    static TYPES: OnceLock<Vec<ModuleType>> = OnceLock::new();
    TYPES.get_or_init(make_types)
}

/// Each width's sink with the tap it plans as inside an include.
pub(crate) fn sink_taps() -> impl Iterator<Item = (&'static str, &'static str)> {
    types()
        .iter()
        .zip(crate::device_out::sink_taps())
        .map(|(sink, (_, tap))| (sink.descriptor.type_name, tap))
}

fn descriptor(width: usize) -> ModuleDescriptor {
    let ports: Vec<PortDescriptor> = crate::device_out::channel_names(false)
        .get(..width)
        .unwrap_or_default()
        .iter()
        .map(|&c| signal_port(c, Direction::In, PortKind::Generic, None, None))
        .collect();
    ModuleDescriptor {
        type_name: Box::leak(format!("std.data_out{width}").into_boxed_str()),
        doc: Box::leak(
            format!(
                "Data sink of {width} channel{} (ADR 0057): a non-audio output adapter reads its inputs (Art-Net: DMX levels in [0, 1]).",
                if width == 1 { "" } else { "s" }
            )
            .into_boxed_str(),
        ),
        ports: Box::leak(ports.into_boxed_slice()),
        parameters: &[],
        feedthrough: Feedthrough::Direct,
        sink: Some(SinkKind::Data),
        source: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: Some(crate::category::IO),
    }
}

/// `std.data_out<N>`: the driver reads its inputs.
#[derive(Clone, Debug, Default)]
pub struct DataOut<const N: usize>;

impl<const N: usize> DirectPrimitive for DataOut<N> {
    fn describe() -> ModuleDescriptor {
        types()
            .get(N - 1)
            .map_or_else(|| descriptor(N), |t| t.descriptor)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, _inputs: &Inputs<'_>, _outputs: &mut Outputs<'_>) {}
}

fn family<const N: usize>() -> ModuleType {
    ModuleType {
        descriptor: descriptor(N),
        create: || Primitive::Direct(Box::new(DataOut::<N>)),
    }
}

macro_rules! widths {
    ($($w:literal),*) => {
        fn make_types() -> Vec<ModuleType> {
            const { assert!([$($w),*].len() == MAX_DATA_WIDTH) };
            vec![$(family::<$w>()),*]
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

    #[test]
    fn every_width_is_a_data_sink_with_a_tap() {
        let four = DataOut::<4>::describe();
        assert_eq!(four.type_name, "std.data_out4");
        assert_eq!(four.sink, Some(SinkKind::Data));
        let names: Vec<_> = four.ports.iter().map(|p| p.name).collect();
        assert_eq!(names, ["c1", "c2", "c3", "c4"]);
        let pairs: Vec<_> = sink_taps().collect();
        assert_eq!(pairs.len(), MAX_DATA_WIDTH);
        assert_eq!(pairs[3], ("std.data_out4", "std.device_out_tap4"));
    }
}
