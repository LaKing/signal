//! The pass-throughs a capture side plans as inside another project (ADR
//! 0038 §7): only the host's roles reach the machine, so an included
//! project's source becomes a plain module whose outputs are its inputs,
//! which the host wires or leaves at 0. Each keeps its source's output
//! names, so the included project's own wires are unchanged; its inputs
//! are the outputs' names with `_in`.
//!
//! - `std.audio_in_pass` (`l_in`, `r_in` → `l`, `r`), for `std.audio_in`;
//! - `std.audio_in_mono_pass` (`m_in` → `m`), for `std.audio_in_mono`;
//! - `std.device_in_pass<N>` (`c<k>_in` → `c<k>`, widths 1 to 64), for
//!   `std.device_in<N>` and `std.control_in<N>` (generic ports: audio or
//!   control values pass alike).
//!
//! `signal-project` (`device::pass_of_source`) maps each source to its
//! pass. Stateless; `out[n] = in[n]`, 0 where nothing is connected.

use std::sync::OnceLock;

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PrepareContext, Primitive,
};

use crate::{at, signal_port};

/// The widest discrete pass-through.
pub const MAX_PASS_WIDTH: usize = crate::device_out::MAX_DEVICE_WIDTH;

const STEREO: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.audio_in_pass",
    doc: "std.audio_in inside another project (ADR 0038 §7): l and r are the inputs l_in and r_in, which the host wires or leaves at 0; only the host captures.",
    ports: &[
        signal_port("l_in", Direction::In, PortKind::Audio, None, None),
        signal_port("r_in", Direction::In, PortKind::Audio, None, None),
        signal_port("l", Direction::Out, PortKind::Audio, None, None),
        signal_port("r", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    source: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::IO),
};

const MONO: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.audio_in_mono_pass",
    doc: "std.audio_in_mono inside another project (ADR 0038 §7): m is the input m_in, which the host wires or leaves at 0; only the host captures.",
    ports: &[
        signal_port("m_in", Direction::In, PortKind::Audio, None, None),
        signal_port("m", Direction::Out, PortKind::Audio, None, None),
    ],
    ..STEREO
};

/// Copies each of the first `rows` signal inputs to the output of its row.
fn pass(inputs: &Inputs<'_>, outputs: &mut Outputs<'_>, rows: usize) {
    for row in 0..rows {
        let x = inputs.signal(row);
        for (n, out) in outputs.signal(row).iter_mut().enumerate() {
            *out = at(x, n, 0.0);
        }
    }
}

/// `std.audio_in_pass`: `l = l_in`, `r = r_in`.
#[derive(Clone, Debug, Default)]
pub struct AudioInPass;

impl DirectPrimitive for AudioInPass {
    fn describe() -> ModuleDescriptor {
        STEREO
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        pass(inputs, outputs, 2);
    }
}

/// `std.audio_in_mono_pass`: `m = m_in`.
#[derive(Clone, Debug, Default)]
pub struct AudioInMonoPass;

impl DirectPrimitive for AudioInMonoPass {
    fn describe() -> ModuleDescriptor {
        MONO
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        pass(inputs, outputs, 1);
    }
}

/// Every width's pass-through, by width from 1.
pub(crate) fn types() -> &'static [ModuleType] {
    static TYPES: OnceLock<Vec<ModuleType>> = OnceLock::new();
    TYPES.get_or_init(make_types)
}

/// The leaked names `c1_in … c64_in`.
fn input_names() -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        (1..=MAX_PASS_WIDTH)
            .map(|k| &*Box::leak(format!("c{k}_in").into_boxed_str()))
            .collect()
    })
}

fn descriptor(width: usize) -> ModuleDescriptor {
    let port = |name, direction| signal_port(name, direction, PortKind::Generic, None, None);
    let inputs = input_names().get(..width).unwrap_or_default();
    let outputs = crate::device_out::channel_names(false)
        .get(..width)
        .unwrap_or_default();
    let ports: Vec<PortDescriptor> = inputs
        .iter()
        .map(|&c| port(c, Direction::In))
        .chain(outputs.iter().map(|&c| port(c, Direction::Out)))
        .collect();
    ModuleDescriptor {
        type_name: Box::leak(format!("std.device_in_pass{width}").into_boxed_str()),
        doc: Box::leak(
            format!(
                "std.device_in{width} or std.control_in{width} inside another project (ADR 0038 §7): c<k> is the input c<k>_in, which the host wires or leaves at 0; only the host captures."
            )
            .into_boxed_str(),
        ),
        ports: Box::leak(ports.into_boxed_slice()),
        ..STEREO
    }
}

/// `std.device_in_pass<N>`: `c<k> = c<k>_in`.
#[derive(Clone, Debug, Default)]
pub struct DeviceInPass<const N: usize>;

impl<const N: usize> DirectPrimitive for DeviceInPass<N> {
    fn describe() -> ModuleDescriptor {
        types()
            .get(N - 1)
            .map_or_else(|| descriptor(N), |t| t.descriptor)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        pass(inputs, outputs, N);
    }
}

fn family<const N: usize>() -> ModuleType {
    ModuleType {
        descriptor: descriptor(N),
        create: || Primitive::Direct(Box::new(DeviceInPass::<N>)),
    }
}

macro_rules! widths {
    ($($w:literal),*) => {
        fn make_types() -> Vec<ModuleType> {
            const { assert!([$($w),*].len() == MAX_PASS_WIDTH) };
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
    fn each_pass_keeps_its_source_s_output_names() {
        let names = |d: &ModuleDescriptor, direction| -> Vec<&str> {
            d.ports
                .iter()
                .filter(|p| p.direction == direction)
                .map(|p| p.name)
                .collect()
        };
        assert_eq!(names(&STEREO, Direction::Out), ["l", "r"]);
        assert_eq!(names(&STEREO, Direction::In), ["l_in", "r_in"]);
        assert_eq!(names(&MONO, Direction::Out), ["m"]);
        let three = DeviceInPass::<3>::describe();
        assert_eq!(three.type_name, "std.device_in_pass3");
        assert_eq!(names(&three, Direction::Out), ["c1", "c2", "c3"]);
        assert_eq!(names(&three, Direction::In), ["c1_in", "c2_in", "c3_in"]);
        assert!(three.source.is_none() && three.sink.is_none());
        assert_eq!(types().len(), MAX_PASS_WIDTH);
    }
}
