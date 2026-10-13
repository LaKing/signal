//! `oracle.probe`: an audio sink with eight signal inputs and nothing else,
//! so that a render records eight of a generated graph's outputs.

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor, Registry,
    RegistryError, Reproducibility, SinkKind,
};

/// The probe's signal inputs.
pub const PROBE_INPUTS: usize = 8;

const fn input(name: &'static str) -> PortDescriptor {
    PortDescriptor {
        name,
        direction: Direction::In,
        shape: PortShape::Signal,
        rate: RateDivisor::ONE,
        kind: PortKind::Generic,
        unit: None,
        range: None,
    }
}

const PROBE: ModuleDescriptor = ModuleDescriptor {
    type_name: "oracle.probe",
    doc: "test fixture: an audio sink recording eight signal inputs",
    ports: &[
        input("in0"),
        input("in1"),
        input("in2"),
        input("in3"),
        input("in4"),
        input("in5"),
        input("in6"),
        input("in7"),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: Some(SinkKind::Audio),
    latency: None,
    reproducibility: Reproducibility::BitExact,
};

struct Probe;

impl DirectPrimitive for Probe {
    fn describe() -> ModuleDescriptor {
        PROBE
    }
    fn prepare(&mut self, _: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, _: &Inputs<'_>, _: &mut Outputs<'_>) {}
}

/// Adds `oracle.probe` to `registry`.
pub fn register_probe(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register(ModuleType {
        descriptor: PROBE,
        create: || Primitive::Direct(Box::new(Probe)),
    })
}
