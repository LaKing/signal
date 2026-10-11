//! The plan of the nested fixture: a project containing a composite containing
//! a composite, with exported fan-out and a feedback loop through a
//! state-output instance.

mod common;

use common::{addr, build, nested, port_addr};
use signal_graph::{GraphWarning, Source};

#[test]
fn nested_plan() {
    let (root, composites) = nested();
    let plan = build(&root, &composites).unwrap();
    insta::assert_json_snapshot!(plan);
}

#[test]
fn connections_resolve_through_exports() {
    let (root, composites) = nested();
    let plan = build(&root, &composites).unwrap();

    let clock = vec![Source {
        from: port_addr("/clock.out"),
        scale: 1.0,
    }];
    // Exported fan-out: `/drums.trigger` -> `kick.trigger` -> both envelopes.
    assert_eq!(plan.sources[&port_addr("/drums/kick/amp_env.trig")], clock);
    assert_eq!(
        plan.sources[&port_addr("/drums/kick/pitch_env.trig")],
        clock
    );
    // Exported output: `/drums.out` is `/drums/mix.out`.
    assert_eq!(
        plan.sources[&port_addr("/speaker.in")],
        [Source {
            from: port_addr("/drums/mix.out"),
            scale: 1.0
        }]
    );
    // Exported parameter, set on `/drums`, overrides the value set inside `voice`.
    let osc = plan
        .instances
        .iter()
        .find(|i| i.address == addr("/drums/kick/osc"))
        .unwrap();
    assert_eq!(osc.type_name, "test.osc");
    assert_eq!(osc.parameters["freq"], 55.0);

    // The loop through the state-output `echo` is accepted.
    assert_eq!(plan.state_outputs, [addr("/drums/echo")]);
    assert_eq!(plan.schedule.len(), plan.instances.len());
    assert_eq!(
        plan.warnings,
        [
            GraphWarning::NoSinkReachable {
                instance: addr("/drums/kick/pitch_env")
            },
            GraphWarning::NoSinkReachable {
                instance: addr("/lonely")
            },
        ]
    );
}
