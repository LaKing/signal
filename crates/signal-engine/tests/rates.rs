//! M2.8-6 (`docs/semantics.md` §16): what the engine refuses about rate
//! divisors and vector ports when it instantiates a plan, and the ticks of
//! a divided-rate instance, whose outputs hold between them.

mod common;

use common::{addr, conn, inst, inst_with, latent_registry, port_addr, registry};
use signal_core::{
    ModuleType, ParameterDescriptor, PortDescriptor, PortShape, RateDivisor, Registry,
};
use signal_engine::{Engine, EngineConfig, EngineError};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};

/// The fixture `base` registered again as `name`, with `ports` and
/// `parameters` mapped.
fn variant(
    registry: &mut Registry,
    base: &str,
    name: &'static str,
    ports: impl Fn(&PortDescriptor) -> PortDescriptor,
    parameters: impl Fn(&ParameterDescriptor) -> ParameterDescriptor,
) {
    let t = *registry.get(base).unwrap();
    let mut descriptor = t.descriptor;
    descriptor.type_name = name;
    descriptor.ports = Box::leak(descriptor.ports.iter().map(ports).collect());
    descriptor.parameters = Box::leak(descriptor.parameters.iter().map(parameters).collect());
    registry
        .register(ModuleType {
            descriptor,
            create: t.create,
        })
        .unwrap();
}

fn d(n: u32) -> RateDivisor {
    RateDivisor::new(n).unwrap()
}

fn plan_with(
    registry: &Registry,
    instances: Vec<InstanceDef>,
    connections: Vec<ConnectionDef>,
) -> ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, 1).unwrap()
}

fn refusal(registry: &Registry, module: &str) -> EngineError {
    let plan = plan_with(registry, vec![inst("x", module)], vec![]);
    Engine::new(&plan, registry, EngineConfig::new(48_000, 1), vec![]).unwrap_err()
}

/// One rate per type; a divided rate has no event ports, latency or sink;
/// a vector has at least one element.
#[test]
fn what_runs_only_at_divisor_one_is_refused() {
    let mut registry = latent_registry();
    let same_param = |p: &ParameterDescriptor| *p;
    let at2 = |p: &PortDescriptor| PortDescriptor { rate: d(2), ..*p };
    let at2_param = |p: &ParameterDescriptor| ParameterDescriptor { rate: d(2), ..*p };
    // Outputs at 2, parameter at 1.
    variant(&mut registry, "test.osc", "x.mixed", at2, same_param);
    variant(&mut registry, "test.trigger", "x.events", at2, at2_param);
    // A sink's rate is its parameters' (its inputs may differ).
    variant(
        &mut registry,
        "test.gained_recorder",
        "x.sink",
        at2,
        at2_param,
    );
    variant(
        &mut registry,
        "test.mixed_latent",
        "x.latent",
        at2,
        at2_param,
    );
    variant(
        &mut registry,
        "test.gain",
        "x.empty",
        |p| PortDescriptor {
            shape: PortShape::Vector(0),
            ..*p
        },
        same_param,
    );
    assert_eq!(
        refusal(&registry, "x.mixed"),
        EngineError::MixedRates {
            instance: addr("/x")
        }
    );
    assert!(
        refusal(&registry, "x.mixed")
            .to_string()
            .contains("rate divisor")
    );
    for (module, what) in [
        ("x.events", "event ports"),
        ("x.sink", "a sink"),
        ("x.latent", "a declared latency"),
    ] {
        let error = refusal(&registry, module);
        assert_eq!(
            error,
            EngineError::UnsupportedRate {
                instance: addr("/x"),
                what
            }
        );
        assert!(error.to_string().contains("rate divisor"), "{error}");
    }
    assert_eq!(
        refusal(&registry, "x.empty"),
        EngineError::UnsupportedShape {
            port: port_addr("/x.in")
        }
    );
}

/// A divided-rate instance is called at the frames that are multiples of
/// its divisor and holds its output between them: `test.frames` counts its
/// calls, so at divisor 3 it reads 0, 0, 0, 1, 1, 1, 2, … through a hold.
#[test]
fn a_divided_instance_ticks_on_multiples_of_its_divisor() {
    let mut registry = registry();
    let at3 = |p: &PortDescriptor| PortDescriptor { rate: d(3), ..*p };
    variant(&mut registry, "test.frames", "d3.test.frames", at3, |p| *p);
    let plan = plan_with(
        &registry,
        vec![
            inst("count", "d3.test.frames"),
            inst_with("g", "test.gain", &[("gain", 1.0)]),
        ],
        vec![conn("count.out", "g.in")],
    );
    let mut engine = Engine::new(&plan, &registry, EngineConfig::new(48_000, 1), vec![]).unwrap();
    let mut seen = Vec::new();
    // Blocks of 2 and 5 frames: the ticks fall inside and across them.
    for k in [2, 5, 2, 5, 2] {
        engine.run_block(k);
        let o = engine.signal_output_index(&port_addr("/g.out")).unwrap();
        for f in 0..k {
            seen.push(engine.signal_output_at(o, f).unwrap());
        }
        engine.drain_log(&mut |_| {});
    }
    let expected: Vec<f64> = (0..16).map(|n| (n / 3) as f64).collect();
    assert_eq!(seen, expected);
}
