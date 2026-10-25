//! The JSON form of a module descriptor, as `signal modules --json` will print it.

use signal_core::{
    Combine, Direction, Feedthrough, ModuleDescriptor, ParameterDescriptor, PortDescriptor,
    PortKind, PortShape, Range, RateDivisor, SinkKind,
};

const SAMPLE: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.sample_out",
    doc: "Sample descriptor covering every field: plays `in` scaled by `level`.",
    ports: &[
        PortDescriptor {
            name: "in",
            direction: Direction::In,
            shape: PortShape::Signal,
            rate: RateDivisor::ONE,
            kind: PortKind::Audio,
            unit: None,
            range: Some(Range {
                min: -1.0,
                max: 1.0,
            }),
        },
        PortDescriptor {
            name: "mute",
            direction: Direction::In,
            shape: PortShape::Events,
            rate: RateDivisor::ONE,
            kind: PortKind::Trigger,
            unit: None,
            range: None,
        },
        PortDescriptor {
            name: "phase",
            direction: Direction::Out,
            shape: PortShape::Signal,
            rate: RateDivisor::ONE,
            kind: PortKind::Phase,
            unit: Some("cycles"),
            range: Some(Range { min: 0.0, max: 1.0 }),
        },
    ],
    parameters: &[
        ParameterDescriptor {
            name: "level",
            kind: PortKind::Gain,
            rate: RateDivisor::ONE,
            unit: None,
            range: Range { min: 0.0, max: 2.0 },
            default: 1.0,
            combine: Combine::Multiply,
            values: None,
        },
        ParameterDescriptor {
            name: "freq",
            kind: PortKind::Frequency,
            rate: RateDivisor::ONE,
            unit: Some("Hz"),
            range: Range {
                min: 0.0,
                max: 20_000.0,
            },
            default: 440.0,
            combine: Combine::Add,
            values: None,
        },
    ],
    feedthrough: Feedthrough::Direct,
    sink: Some(SinkKind::Audio),
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: None,
};

#[test]
fn module_descriptor_json() {
    insta::assert_json_snapshot!(SAMPLE);
}
