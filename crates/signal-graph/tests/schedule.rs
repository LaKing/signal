//! Dependency graph, schedule, canonical sources and diagnostics (ADR 0016).

mod common;

use common::{addr, build, composite, conn, errors, inst, port_addr};
use signal_core::PortKind;
use signal_graph::{ConnectionDef, GraphError, GraphWarning, Source};

fn source(from: &str, scale: f64) -> Source {
    Source {
        from: port_addr(from),
        scale,
    }
}

#[test]
fn cycle_without_state_output_is_rejected_and_listed() {
    let project = composite(
        "project",
        vec![
            inst("a", "test.vca"),
            inst("b", "test.vca"),
            inst("c", "test.vca"),
        ],
        vec![
            conn("a.out", "c.in"),
            conn("c.out", "b.in"),
            conn("b.out", "a.in"),
        ],
    );
    let errors = errors(&project, &[]);
    assert_eq!(
        errors,
        [GraphError::Cycle {
            instances: vec![addr("/a"), addr("/c"), addr("/b")]
        }]
    );
    assert_eq!(
        errors[0].to_string(),
        "dependency cycle without a state-output instance: /a -> /c -> /b -> /a"
    );
}

#[test]
fn same_loop_through_state_output_is_accepted() {
    let project = composite(
        "project",
        vec![
            inst("a", "test.vca"),
            inst("b", "test.delay"),
            inst("c", "test.vca"),
        ],
        vec![
            conn("a.out", "c.in"),
            conn("c.out", "b.in"),
            conn("b.out", "a.in"),
        ],
    );
    let plan = build(&project, &[]).unwrap();
    // `b.out -> a.in` is no dependency edge: `b` is state-output.
    assert_eq!(plan.schedule, [addr("/a"), addr("/c"), addr("/b")]);
    assert_eq!(plan.state_outputs, [addr("/b")]);
}

#[test]
fn ties_break_by_address() {
    let project = composite(
        "project",
        vec![
            inst("c", "test.osc"),
            inst("a", "test.vca"),
            inst("b", "test.osc"),
        ],
        vec![conn("c.out", "a.in")],
    );
    let plan = build(&project, &[]).unwrap();
    // Ready at first: /b and /c; /a only after /c.
    assert_eq!(plan.schedule, [addr("/b"), addr("/c"), addr("/a")]);
    assert!(plan.state_outputs.is_empty());
}

#[test]
fn disconnected_instance_is_scheduled_and_warned() {
    let project = composite(
        "project",
        vec![
            inst("osc", "test.osc"),
            inst("spk", "test.out"),
            inst("lonely", "test.osc"),
        ],
        vec![conn("osc.out", "spk.in")],
    );
    let plan = build(&project, &[]).unwrap();
    assert_eq!(plan.schedule, [addr("/lonely"), addr("/osc"), addr("/spk")]);
    assert_eq!(
        plan.warnings,
        [GraphWarning::NoSinkReachable {
            instance: addr("/lonely")
        }]
    );
}

#[test]
fn fan_in_and_fan_out_are_allowed() {
    let project = composite(
        "project",
        vec![
            inst("a", "test.osc"),
            inst("b", "test.osc"),
            inst("t1", "test.trig"),
            inst("t2", "test.trig"),
            inst("env", "test.env"),
            inst("vca", "test.vca"),
            inst("spk", "test.out"),
        ],
        vec![
            // Two signals into one signal input (summed).
            conn("a.out", "vca.in"),
            conn("b.out", "vca.in"),
            // Two event sources into one event input (merged).
            conn("t2.out", "env.trig"),
            conn("t1.out", "env.trig"),
            // One output into two inputs.
            conn("env.out", "vca.cv"),
            conn("vca.out", "spk.in"),
            conn("a.out", "spk.in"),
        ],
    );
    let plan = build(&project, &[]).unwrap();
    assert_eq!(
        plan.sources[&port_addr("/vca.in")],
        [source("/a.out", 1.0), source("/b.out", 1.0)]
    );
    assert_eq!(
        plan.sources[&port_addr("/env.trig")],
        [source("/t1.out", 1.0), source("/t2.out", 1.0)]
    );
    assert_eq!(
        plan.sources[&port_addr("/spk.in")],
        [source("/a.out", 1.0), source("/vca.out", 1.0)]
    );
}

#[test]
fn sources_are_in_canonical_order_with_scales() {
    let project = composite(
        "project",
        vec![
            inst("zed", "test.osc"),
            inst("alpha", "test.osc"),
            inst("vca", "test.vca"),
        ],
        vec![
            ConnectionDef {
                scale: Some(0.5),
                ..conn("zed.out", "vca.in")
            },
            conn("alpha.out", "vca.in"),
        ],
    );
    let plan = build(&project, &[]).unwrap();
    assert_eq!(
        plan.sources[&port_addr("/vca.in")],
        [source("/alpha.out", 1.0), source("/zed.out", 0.5)]
    );
    // Every input port and parameter has an entry, connected or not.
    assert!(plan.sources[&port_addr("/vca.cv")].is_empty());
    assert!(plan.sources[&port_addr("/zed.freq")].is_empty());
    assert_eq!(plan.sources.len(), 4);
}

#[test]
fn kind_mismatch_is_a_warning() {
    let project = composite(
        "project",
        vec![inst("env", "test.env"), inst("spk", "test.out")],
        vec![conn("env.out", "spk.in")],
    );
    let plan = build(&project, &[]).unwrap();
    assert_eq!(
        plan.warnings,
        [GraphWarning::KindMismatch {
            from: port_addr("/env.out"),
            to: port_addr("/spk.in"),
            from_kind: PortKind::Control,
            to_kind: PortKind::Audio,
        }]
    );
}
