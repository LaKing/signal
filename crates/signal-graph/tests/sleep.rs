//! Sleep groups in the plan (ADR 0044 §11, `docs/semantics.md` §17): a
//! composite declares one over an instance, and the plan carries it with
//! its members' address and the driver's primitive port; every plan that
//! could not run the group as one unit after its driver is refused with a
//! named reason.

mod common;

use common::{build, composite, conn, errors, input, inst, output};
use signal_graph::{CompositeDef, GraphError, SleepDef};

/// A voice: a VCA whose `in` and `out` are exported, `cv` too.
fn voice() -> CompositeDef {
    let mut def = composite("voice", vec![inst("vca", "test.vca")], vec![]);
    def.exports.inputs = vec![input("in", &["vca.in"]), input("cv", &["vca.cv"])];
    def.exports.outputs = vec![output("out", "vca.out")];
    def
}

/// `drv` into the voice `g`, heard on `spk`, plus `extra` instances and
/// connections, with `sleep`.
fn root(extra: &[(&str, &str)], wires: &[(&str, &str)], sleep: Vec<SleepDef>) -> CompositeDef {
    let mut instances = vec![
        inst("drv", "test.osc"),
        inst("g", "voice"),
        inst("spk", "test.out"),
    ];
    instances.extend(extra.iter().map(|(name, module)| inst(name, module)));
    let mut connections = vec![conn("drv.out", "g.in"), conn("g.out", "spk.in")];
    connections.extend(wires.iter().map(|(from, to)| conn(from, to)));
    let mut def = composite("project", instances, connections);
    def.sleep = sleep;
    def
}

fn group(members: &str, driver: &str, rest: u64) -> SleepDef {
    SleepDef {
        members: members.to_owned(),
        driver: driver.to_owned(),
        rest,
    }
}

fn reason(root: &CompositeDef) -> String {
    let found = errors(root, &[voice()]);
    match found.as_slice() {
        [GraphError::SleepGroup { reason, .. }] => reason.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_group_is_planned_with_its_members_and_driver() {
    let plan = build(&root(&[], &[], vec![group("g", "drv.out", 64)]), &[voice()]).unwrap();
    let [found] = plan.sleep_groups.as_slice() else {
        panic!("{:?}", plan.sleep_groups);
    };
    assert_eq!(found.members.as_str(), "/g");
    assert_eq!(found.driver.to_string(), "/drv.out");
    assert_eq!(found.rest, 64);
    assert!(found.contains(&"/g/vca".parse().unwrap()));
    assert!(!found.contains(&"/gx".parse().unwrap()));
    // The plan's JSON says nothing of groups when there are none.
    let plain = build(&root(&[], &[], vec![]), &[voice()]).unwrap();
    assert!(
        !serde_json::to_string(&plain)
            .unwrap()
            .contains("sleep_groups")
    );
}

#[test]
fn a_group_that_cannot_run_after_its_driver_is_refused() {
    let cases: Vec<(CompositeDef, &str)> = vec![
        (
            root(&[], &[], vec![group("g", "g.out", 64)]),
            "inside the group",
        ),
        (
            root(&[("t", "test.trig")], &[], vec![group("g", "t.out", 64)]),
            "not a signal output",
        ),
        (
            root(&[], &[], vec![group("g", "drv.out", 0)]),
            "rest length is 0",
        ),
        (
            root(&[], &[], vec![group("nope", "drv.out", 64)]),
            "no instance `nope`",
        ),
        (
            root(
                &[],
                &[],
                vec![group("g", "drv.out", 64), group("g", "drv.out", 32)],
            ),
            "overlaps",
        ),
        (
            root(
                &[("x", "test.delay")],
                &[("g.out", "x.in"), ("x.out", "g.cv")],
                vec![group("g", "drv.out", 64)],
            ),
            "come back into it through /x",
        ),
        (
            root(
                &[("x", "test.vca"), ("y", "test.vca")],
                &[("g.out", "x.in"), ("x.out", "y.cv")],
                vec![group("g", "x.out", 64)],
            ),
            "reaches its driver",
        ),
    ];
    for (root, expected) in cases {
        let found = reason(&root);
        assert!(found.contains(expected), "{expected}: {found}");
    }
}
