//! `std.control_in<N>` (ADR 0057 §1): the capture side of a control device
//! slot (`kind = "osc"`), *N* values from 1 to [`MAX_CONTROL_WIDTH`]: a
//! source (`SourceKind::Data`) whose outputs `c1 … cN` the driver writes
//! block by block, each the last value a control surface sent, recorded
//! as an audio capture side's words are and replayed exactly.
//!
//! No inputs, no parameters; the driver writes the outputs (0 when nothing
//! does). Inside an included project it is refused, as every capture side
//! is (only the host's roles reach the machine).

use std::sync::OnceLock;

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PrepareContext, Primitive, SourceKind,
};

use crate::signal_port;

/// The widest control slot.
pub const MAX_CONTROL_WIDTH: usize = crate::device_out::MAX_DEVICE_WIDTH;

/// Every width's source, by width from 1.
pub(crate) fn types() -> &'static [ModuleType] {
    static TYPES: OnceLock<Vec<ModuleType>> = OnceLock::new();
    TYPES.get_or_init(make_types)
}

fn descriptor(width: usize) -> ModuleDescriptor {
    let ports: Vec<PortDescriptor> = crate::device_out::channel_names(false)
        .get(..width)
        .unwrap_or_default()
        .iter()
        .map(|&c| signal_port(c, Direction::Out, PortKind::Generic, None, None))
        .collect();
    ModuleDescriptor {
        type_name: Box::leak(format!("std.control_in{width}").into_boxed_str()),
        doc: Box::leak(
            format!(
                "Control source of {width} value{} (ADR 0057): the driver writes its outputs, each the last value a control surface sent (OSC).",
                if width == 1 { "" } else { "s" }
            )
            .into_boxed_str(),
        ),
        ports: Box::leak(ports.into_boxed_slice()),
        parameters: &[],
        feedthrough: Feedthrough::Direct,
        sink: None,
        source: Some(SourceKind::Data),
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: Some(crate::category::IO),
    }
}

/// `std.control_in<N>`: its outputs are the rows the driver wrote.
#[derive(Clone, Debug, Default)]
pub struct ControlIn<const N: usize>;

impl<const N: usize> DirectPrimitive for ControlIn<N> {
    fn describe() -> ModuleDescriptor {
        types()
            .get(N - 1)
            .map_or_else(|| descriptor(N), |t| t.descriptor)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        crate::audio_in::copy(inputs, outputs, N);
    }
}

fn family<const N: usize>() -> ModuleType {
    ModuleType {
        descriptor: descriptor(N),
        create: || Primitive::Direct(Box::new(ControlIn::<N>)),
    }
}

macro_rules! widths {
    ($($w:literal),*) => {
        fn make_types() -> Vec<ModuleType> {
            const { assert!([$($w),*].len() == MAX_CONTROL_WIDTH) };
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
    fn every_width_is_a_control_source() {
        let three = ControlIn::<3>::describe();
        assert_eq!(three.type_name, "std.control_in3");
        assert_eq!(three.source, Some(SourceKind::Data));
        let names: Vec<_> = three.ports.iter().map(|p| (p.name, p.direction)).collect();
        assert_eq!(
            names,
            [
                ("c1", Direction::Out),
                ("c2", Direction::Out),
                ("c3", Direction::Out)
            ]
        );
        assert_eq!(types().len(), MAX_CONTROL_WIDTH);
    }
}
