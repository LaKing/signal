//! One test per validation error kind, asserting the variant and the addresses
//! it names.

mod common;

use common::{addr, composite, conn, errors, input, inst, inst_with, nested, output, port_addr};
use signal_core::{Direction, PortShape, Range, RateDivisor};
use signal_graph::{CompositeDef, ConnectionDef, Exports, GraphError};

const CONVERTER: &str = "converter required (ADR 0005), not available in M1";

/// A root with an oscillator `osc`, a sink `spk` and `connections`.
fn root(connections: Vec<ConnectionDef>) -> CompositeDef {
    composite(
        "project",
        vec![inst("osc", "test.osc"), inst("spk", "test.out")],
        connections,
    )
}

/// A composite `c` around one `test.vca` named `x`, with `exports`.
fn wrapper(exports: Exports) -> CompositeDef {
    CompositeDef {
        exports,
        ..composite("c", vec![inst("x", "test.vca")], vec![])
    }
}

#[test]
fn duplicate_composite() {
    let c = wrapper(Exports::default());
    assert_eq!(
        errors(&root(vec![]), &[c.clone(), c]),
        [GraphError::DuplicateComposite { name: "c".into() }]
    );
}

#[test]
fn ambiguous_module() {
    let c = composite("test.osc", vec![], vec![]);
    assert_eq!(
        errors(&root(vec![]), &[c]),
        [GraphError::AmbiguousModule {
            module: "test.osc".into()
        }]
    );
}

#[test]
fn unknown_module() {
    let project = composite("project", vec![inst("x", "test.nope")], vec![]);
    assert_eq!(
        errors(&project, &[]),
        [GraphError::UnknownModule {
            instance: addr("/x"),
            module: "test.nope".into()
        }]
    );
}

#[test]
fn recursive_composite() {
    let a = composite("a", vec![inst("inner", "b")], vec![]);
    let b = composite("b", vec![inst("back", "a")], vec![]);
    let project = composite("project", vec![inst("outer", "a")], vec![]);
    let errors = errors(&project, &[a, b]);
    assert_eq!(
        errors,
        [GraphError::RecursiveComposite {
            instance: addr("/outer/inner/back"),
            chain: vec!["a".into(), "b".into(), "a".into()],
        }]
    );
    assert_eq!(
        errors[0].to_string(),
        "/outer/inner/back: recursive composite a -> b -> a"
    );
}

#[test]
fn invalid_name() {
    let project = composite("project", vec![inst("Osc", "test.osc")], vec![]);
    assert_eq!(
        errors(&project, &[]),
        [GraphError::InvalidName {
            scope: None,
            name: "Osc".into()
        }]
    );
}

#[test]
fn duplicate_instance() {
    let project = composite(
        "project",
        vec![inst("osc", "test.osc"), inst("osc", "test.vca")],
        vec![],
    );
    assert_eq!(
        errors(&project, &[]),
        [GraphError::DuplicateInstance {
            scope: None,
            name: "osc".into()
        }]
    );
}

#[test]
fn export_name_collision() {
    let c = wrapper(Exports {
        inputs: vec![input("io", &["x.in"])],
        outputs: vec![output("io", "x.out")],
        parameters: vec![],
    });
    let project = composite("project", vec![inst("c1", "c")], vec![]);
    assert_eq!(
        errors(&project, &[c]),
        [GraphError::ExportNameCollision {
            scope: Some(addr("/c1")),
            composite: "c".into(),
            name: "io".into()
        }]
    );
}

#[test]
fn empty_export() {
    let c = wrapper(Exports {
        inputs: vec![input("in", &[])],
        outputs: vec![],
        parameters: vec![],
    });
    let project = composite("project", vec![inst("c1", "c")], vec![]);
    assert_eq!(
        errors(&project, &[c]),
        [GraphError::EmptyExport {
            scope: Some(addr("/c1")),
            composite: "c".into(),
            name: "in".into()
        }]
    );
}

#[test]
fn invalid_endpoint() {
    assert_eq!(
        errors(&root(vec![conn("osc", "spk.in")]), &[]),
        [GraphError::InvalidEndpoint {
            scope: None,
            endpoint: "osc".into()
        }]
    );
}

#[test]
fn unknown_instance() {
    assert_eq!(
        errors(&root(vec![conn("ghost.out", "spk.in")]), &[]),
        [GraphError::UnknownInstance {
            scope: None,
            name: "ghost".into()
        }]
    );
}

#[test]
fn unknown_port() {
    // On a composite instance, a port is unknown when nothing exports it.
    let (mut project, composites) = nested();
    project.connections.push(conn("drums.nope", "speaker.in"));
    assert_eq!(
        errors(&project, &composites),
        [GraphError::UnknownPort {
            port: port_addr("/drums.nope")
        }]
    );
}

#[test]
fn unknown_parameter() {
    let project = composite(
        "project",
        vec![inst_with("osc", "test.osc", &[("gain", 1.0)])],
        vec![],
    );
    assert_eq!(
        errors(&project, &[]),
        [GraphError::UnknownParameter {
            parameter: port_addr("/osc.gain")
        }]
    );
}

#[test]
fn wrong_direction() {
    assert_eq!(
        errors(&root(vec![conn("spk.in", "osc.out")]), &[]),
        [
            GraphError::WrongDirection {
                port: port_addr("/spk.in"),
                expected: Direction::Out
            },
            GraphError::WrongDirection {
                port: port_addr("/osc.out"),
                expected: Direction::In
            },
        ]
    );
}

#[test]
fn shape_mismatch() {
    let project = composite(
        "project",
        vec![inst("trig", "test.trig"), inst("spk", "test.out")],
        vec![conn("trig.out", "spk.in")],
    );
    let errors = errors(&project, &[]);
    assert_eq!(
        errors,
        [GraphError::ShapeMismatch {
            from: port_addr("/trig.out"),
            to: port_addr("/spk.in"),
            from_shape: PortShape::Events,
            to_shape: PortShape::Signal,
        }]
    );
    assert!(errors[0].to_string().contains(CONVERTER));
}

/// M2.8-6: divisors 2 and 3 are not multiples of each other, so no
/// converter joins them (2 -> 1 is a hold now: `tests/rates.rs`).
#[test]
fn rate_mismatch() {
    let project = composite(
        "project",
        vec![inst("slow", "test.slow"), inst("x", "test.in3")],
        vec![conn("slow.out", "x.in")],
    );
    let errors = errors(&project, &[]);
    assert_eq!(
        errors,
        [GraphError::RateMismatch {
            from: port_addr("/slow.out"),
            to: port_addr("/x.in"),
            from_rate: RateDivisor::new(2).unwrap(),
            to_rate: RateDivisor::new(3).unwrap(),
        }]
    );
    let message = errors[0].to_string();
    assert!(
        message.contains("/slow.out (rate divisor 2) -> /x.in (rate divisor 3)"),
        "{message}"
    );
    assert!(
        message.contains("hold") && message.contains("decimate"),
        "{message}"
    );
}

#[test]
fn parameter_out_of_range() {
    // Set through two levels of exported parameters.
    let (mut project, composites) = nested();
    project.instances[1]
        .parameters
        .insert("kick_pitch".into(), 30_000.0);
    assert_eq!(
        errors(&project, &composites),
        [GraphError::ParameterOutOfRange {
            parameter: port_addr("/drums/kick/osc.freq"),
            value: 30_000.0,
            range: Range {
                min: 0.0,
                max: 20_000.0
            },
        }]
    );
}

#[test]
fn non_finite_scale() {
    let connection = ConnectionDef {
        scale: Some(f64::INFINITY),
        ..conn("osc.out", "spk.in")
    };
    assert_eq!(
        errors(&root(vec![connection]), &[]),
        [GraphError::NonFiniteScale {
            from: port_addr("/osc.out"),
            to: port_addr("/spk.in"),
            scale: f64::INFINITY,
        }]
    );
}

#[test]
fn cycle() {
    let project = composite(
        "project",
        vec![inst("b", "test.vca"), inst("a", "test.vca")],
        vec![conn("a.out", "b.in"), conn("b.out", "a.in")],
    );
    assert_eq!(
        errors(&project, &[]),
        [GraphError::Cycle {
            instances: vec![addr("/a"), addr("/b")]
        }]
    );
}
