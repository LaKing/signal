//! M2.8-6 (ADR 0005, `docs/semantics.md` §16): the rate converters the graph
//! compiler inserts pass every contract check, as any primitive must: a
//! scalar hold, a vector decimate and a hold of a 256-element vector.

use signal_core::{ModuleType, PortKind, PortShape, RateDivisor, converter};

signal_contract::install_counting_allocator!();

/// The converter from divisor `from` to `to` for ports of `shape` and `kind`.
fn converter_type(from: u32, to: u32, shape: PortShape, kind: PortKind) -> ModuleType {
    let d = |n| RateDivisor::new(n).unwrap();
    *converter(d(from), d(to), shape, kind).unwrap()
}

fn scalar_hold() -> ModuleType {
    converter_type(64, 1, PortShape::Signal, PortKind::Audio)
}

fn vector_decimate() -> ModuleType {
    converter_type(1, 64, PortShape::Vector(4), PortKind::Control)
}

fn long_vector_hold() -> ModuleType {
    converter_type(1600, 2, PortShape::Vector(256), PortKind::Generic)
}

mod hold_d64_d1_signal_audio {
    signal_contract::contract_tests!(super::scalar_hold());
}

mod decimate_d1_d64_vector4_control {
    signal_contract::contract_tests!(super::vector_decimate());
}

mod hold_d1600_d2_vector256_generic {
    signal_contract::contract_tests!(super::long_vector_hold());
}
