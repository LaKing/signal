//! A test binary without the counting allocator: `no_allocation` fails instead
//! of passing vacuously.

use signal_contract::{Check, Settings, stimulus};
use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor,
};

struct Silence;

const OUT: PortDescriptor = PortDescriptor {
    name: "out",
    direction: Direction::Out,
    shape: PortShape::Signal,
    rate: RateDivisor::ONE,
    kind: PortKind::Generic,
    unit: None,
    range: None,
};

impl DirectPrimitive for Silence {
    fn describe() -> ModuleDescriptor {
        ModuleDescriptor {
            type_name: "test.silence",
            doc: "Outputs 0.",
            ports: &[OUT],
            parameters: &[],
            feedthrough: Feedthrough::Direct,
            sink: None,
            latency: None,
            reproducibility: signal_core::Reproducibility::BitExact,
            category: None,
        }
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, _inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(0.0);
    }
}

#[test]
fn no_allocation_fails_when_the_allocator_is_missing() {
    let silence = ModuleType {
        descriptor: Silence::describe(),
        create: || Primitive::Direct(Box::new(Silence)),
    };
    let failure = Check::NoAllocation
        .run(&silence, stimulus::random, &Settings::without_persistence())
        .expect_err("must not pass without the counting allocator");
    assert!(
        failure.detail.contains("install_counting_allocator!"),
        "{failure}"
    );
    for check in Check::ALL.into_iter().filter(|&c| c != Check::NoAllocation) {
        check.assert(&silence, stimulus::random, &Settings::without_persistence());
    }
}
