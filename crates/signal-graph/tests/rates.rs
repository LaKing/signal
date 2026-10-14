//! M2.8-6, AC5 (ADR 0005, `docs/semantics.md` §16): a connection across rate
//! divisors that hold or decimate joins runs through an inserted converter
//! instance, visible in the plan and as a warning; anything else is an
//! error naming the connection as written.

mod common;

use common::{build, composite, conn, errors, inst, port_addr};
use signal_core::{Conversion, PortShape, RateDivisor};
use signal_graph::{ConnectionDef, GraphError, GraphWarning, Source};

fn d(n: u32) -> RateDivisor {
    RateDivisor::new(n).unwrap()
}

fn scaled(from: &str, to: &str, scale: f64) -> ConnectionDef {
    ConnectionDef {
        scale: Some(scale),
        ..conn(from, to)
    }
}

fn converters(warnings: &[GraphWarning]) -> Vec<&GraphWarning> {
    warnings
        .iter()
        .filter(|w| matches!(w, GraphWarning::ConverterInserted { .. }))
        .collect()
}

/// A hold into an input and one into a parameter, from two sources: each
/// source gets its converter, the target its scale, and both are reported.
#[test]
fn a_hold_is_inserted_into_an_input_and_a_parameter() {
    let project = composite(
        "project",
        vec![
            inst("slow", "test.slow"),
            inst("lfo", "test.slow64"),
            inst("osc", "test.osc"),
            inst("spk", "test.out"),
        ],
        vec![
            scaled("slow.out", "spk.in", 0.5),
            scaled("lfo.out", "osc.freq", 100.0),
            conn("osc.out", "spk.in"),
        ],
    );
    let plan = build(&project, &[]).unwrap();
    let types: Vec<(&str, &str)> = plan
        .instances
        .iter()
        .map(|i| (i.address.as_str(), i.type_name))
        .collect();
    assert_eq!(
        types,
        [
            ("/lfo", "test.slow64"),
            ("/lfo/out_d1", "core.hold.d64.d1.signal.control"),
            ("/osc", "test.osc"),
            ("/slow", "test.slow"),
            ("/slow/out_d1", "core.hold.d2.d1.signal.audio"),
            ("/spk", "test.out"),
        ]
    );
    let source = |from: &str, scale| Source {
        from: port_addr(from),
        scale,
        shape: PortShape::Signal,
    };
    assert_eq!(
        plan.sources[&port_addr("/slow/out_d1.in")],
        [source("/slow.out", 1.0)]
    );
    assert_eq!(
        plan.sources[&port_addr("/spk.in")],
        [source("/osc.out", 1.0), source("/slow/out_d1.out", 0.5)]
    );
    assert_eq!(
        plan.sources[&port_addr("/osc.freq")],
        [source("/lfo/out_d1.out", 100.0)]
    );
    // The converter runs between its source and its target.
    let at = |a: &str| plan.schedule.iter().position(|s| s.as_str() == a).unwrap();
    assert!(at("/slow") < at("/slow/out_d1") && at("/slow/out_d1") < at("/spk"));

    let inserted = converters(&plan.warnings);
    assert_eq!(
        inserted[0],
        &GraphWarning::ConverterInserted {
            from: port_addr("/lfo.out"),
            to: port_addr("/osc.freq"),
            converter: "/lfo/out_d1".parse().unwrap(),
            conversion: Conversion::Hold,
            from_rate: d(64),
            to_rate: d(1),
        }
    );
    assert_eq!(
        inserted[1].to_string(),
        "/slow.out -> /spk.in: inserted converter /slow/out_d1 (hold, rate divisor 2 -> 1)"
    );
    assert_eq!(inserted.len(), 2);
}

/// One converter serves every target of a source port at one divisor.
#[test]
fn targets_at_one_divisor_share_a_converter() {
    let project = composite(
        "project",
        vec![
            inst("lfo", "test.slow64"),
            inst("a", "test.osc"),
            inst("b", "test.osc"),
        ],
        vec![conn("lfo.out", "a.freq"), conn("lfo.out", "b.freq")],
    );
    let plan = build(&project, &[]).unwrap();
    let names: Vec<&str> = plan.instances.iter().map(|i| i.address.as_str()).collect();
    assert_eq!(names, ["/a", "/b", "/lfo", "/lfo/out_d1"]);
    assert_eq!(converters(&plan.warnings).len(), 2);
}

/// A vector of 4 at divisor 1 into one at 64: a decimate, element-wise.
#[test]
fn a_vector_is_decimated() {
    let project = composite(
        "project",
        vec![inst("v", "test.vsrc"), inst("x", "test.vin64")],
        vec![conn("v.out", "x.in")],
    );
    let plan = build(&project, &[]).unwrap();
    let converter = plan
        .instances
        .iter()
        .find(|i| i.address.as_str() == "/v/out_d64")
        .unwrap();
    assert_eq!(converter.type_name, "core.decimate.d1.d64.vector4.control");
    assert_eq!(
        plan.sources[&port_addr("/x.in")],
        [Source {
            from: port_addr("/v/out_d64.out"),
            scale: 1.0,
            shape: PortShape::Vector(4),
        }]
    );
    assert!(matches!(
        converters(&plan.warnings)[..],
        [GraphWarning::ConverterInserted {
            conversion: Conversion::Decimate,
            ..
        }]
    ));
}

/// Equal rates insert nothing: no converter in a plan of divisor-1 ports.
#[test]
fn equal_rates_insert_nothing() {
    let (project, composites) = common::nested();
    let plan = build(&project, &composites).unwrap();
    assert!(
        plan.instances
            .iter()
            .all(|i| !i.type_name.starts_with("core."))
    );
    assert!(converters(&plan.warnings).is_empty());
}

/// auditor-2-005: a connection that a converter cannot join keeps the
/// endpoints the user wrote in its errors.
#[test]
fn a_shape_mismatch_across_rates_names_the_connection_as_written() {
    let project = composite(
        "project",
        vec![inst("v", "test.vslow"), inst("spk", "test.out")],
        vec![conn("v.out", "spk.in")],
    );
    let errors = errors(&project, &[]);
    assert_eq!(
        errors[0],
        GraphError::ShapeMismatch {
            from: port_addr("/v.out"),
            to: port_addr("/spk.in"),
            from_shape: PortShape::Vector(4),
            to_shape: PortShape::Signal,
        }
    );
    for error in &errors {
        let message = error.to_string();
        assert!(message.starts_with("/v.out ("), "{message}");
        assert!(message.contains("-> /spk.in ("), "{message}");
    }
    assert!(errors.iter().all(|e| matches!(
        e,
        GraphError::ShapeMismatch { from, .. } | GraphError::RateMismatch { from, .. }
            if *from == port_addr("/v.out")
    )));
}

/// Divisors that are not multiples, mismatched vector sizes and a vector
/// into a parameter are errors.
#[test]
fn other_connections_across_rates_or_shapes_are_errors() {
    let case = |module: &str, from: &str, target: &str, to: &str| {
        let project = composite(
            "project",
            vec![inst("s", module), inst("t", target)],
            vec![conn(from, to)],
        );
        errors(&project, &[])
    };
    let rate = |from: u32, to: u32| GraphError::RateMismatch {
        from: port_addr("/s.out"),
        to: port_addr("/t.in"),
        from_rate: d(from),
        to_rate: d(to),
    };
    assert_eq!(
        case("test.slow3", "s.out", "test.in64", "t.in"),
        [rate(3, 64)]
    );
    assert_eq!(
        case("test.slow3", "s.out", "test.in1600", "t.in"),
        [rate(3, 1600)]
    );
    assert_eq!(
        case("test.vsrc", "s.out", "test.vin2", "t.in"),
        [GraphError::ShapeMismatch {
            from: port_addr("/s.out"),
            to: port_addr("/t.in"),
            from_shape: PortShape::Vector(4),
            to_shape: PortShape::Vector(2),
        }]
    );
    assert_eq!(
        case("test.vsrc", "s.out", "test.osc", "t.freq"),
        [GraphError::ShapeMismatch {
            from: port_addr("/s.out"),
            to: port_addr("/t.freq"),
            from_shape: PortShape::Vector(4),
            to_shape: PortShape::Signal,
        }]
    );
}

/// auditor-2-007: a project may name a converter explicitly by its reserved
/// name; a malformed `core.` name is an unknown module.
#[test]
fn converters_can_be_named_and_malformed_names_are_unknown() {
    let explicit = composite(
        "project",
        vec![
            inst("lfo", "test.slow64"),
            inst("h", "core.hold.d64.d1.signal.control"),
            inst("osc", "test.osc"),
        ],
        vec![conn("lfo.out", "h.in"), conn("h.out", "osc.freq")],
    );
    let plan = build(&explicit, &[]).unwrap();
    assert_eq!(plan.instances.len(), 3);
    assert!(converters(&plan.warnings).is_empty());

    let malformed = composite(
        "project",
        vec![inst("h", "core.hold.d0.d1.signal.audio")],
        vec![],
    );
    assert_eq!(
        errors(&malformed, &[]),
        [GraphError::UnknownModule {
            instance: "/h".parse().unwrap(),
            module: "core.hold.d0.d1.signal.audio".to_owned(),
        }]
    );
}
