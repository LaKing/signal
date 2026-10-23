//! ADR 0035 §4: scope roots. Flattening gives every primitive under a scope
//! root the key (seed, address relative to the innermost root); outside
//! every root there is none; a primitive cannot be a root.

mod common;

use common::{addr, build, composite, errors, inst};
use signal_graph::{GraphError, ScopeDef};

fn scoped(name: &str, module: &str, seed: u64) -> signal_graph::InstanceDef {
    signal_graph::InstanceDef {
        scope: Some(ScopeDef { seed }),
        ..inst(name, module)
    }
}

#[test]
fn the_innermost_scope_root_keys_the_stream() {
    let inner = composite("inner", vec![inst("osc", "test.osc")], vec![]);
    let outer = composite(
        "outer",
        vec![
            inst("osc", "test.osc"),
            scoped("deep", "inner", 7),
            inst("plain", "inner"),
        ],
        vec![],
    );
    let root = composite(
        "",
        vec![scoped("top", "outer", 3), inst("free", "test.osc")],
        vec![],
    );
    let plan = build(&root, &[inner, outer]).unwrap();
    let key = |address: &str| {
        plan.instances
            .iter()
            .find(|i| i.address == addr(address))
            .unwrap()
            .stream
            .as_ref()
            .map(|k| (k.seed, k.relative.to_string()))
    };
    assert_eq!(key("/free"), None);
    assert_eq!(key("/top/osc"), Some((3, "/osc".to_owned())));
    assert_eq!(key("/top/plain/osc"), Some((3, "/plain/osc".to_owned())));
    assert_eq!(
        key("/top/deep/osc"),
        Some((7, "/osc".to_owned())),
        "innermost wins"
    );
    let dump = serde_json::to_string(&plan).unwrap();
    assert!(dump.contains("\"stream\""));
    // Without scope roots, nothing is serialized.
    let plain = build(&composite("", vec![inst("free", "test.osc")], vec![]), &[]).unwrap();
    assert!(!serde_json::to_string(&plain).unwrap().contains("stream"));
}

#[test]
fn a_primitive_cannot_be_a_scope_root() {
    let root = composite("", vec![scoped("osc", "test.osc", 1)], vec![]);
    let found = errors(&root, &[]);
    assert_eq!(
        found,
        [GraphError::ScopeOnPrimitive {
            instance: addr("/osc"),
            module: "test.osc".to_owned(),
        }]
    );
    assert!(
        found[0]
            .to_string()
            .contains("only a composite instance can be a scope root")
    );
}
