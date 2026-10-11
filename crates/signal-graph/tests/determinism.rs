//! Declaration order does not matter: shuffling instances, connections, exports
//! and composites yields an identical serialized plan.

mod common;

use common::{build, nested};
use proptest::prelude::*;
use signal_graph::CompositeDef;

/// `def` with its instances, connections and exports in shuffled order.
fn shuffled(def: CompositeDef) -> impl Strategy<Value = CompositeDef> {
    let exports = def.exports.clone();
    (
        Just(def.instances.clone()).prop_shuffle(),
        Just(def.connections.clone()).prop_shuffle(),
        Just(exports.inputs).prop_shuffle(),
        Just(exports.outputs).prop_shuffle(),
        Just(exports.parameters).prop_shuffle(),
    )
        .prop_map(
            move |(instances, connections, inputs, outputs, parameters)| {
                let mut def = def.clone();
                def.instances = instances;
                def.connections = connections;
                def.exports.inputs = inputs;
                def.exports.outputs = outputs;
                def.exports.parameters = parameters;
                def
            },
        )
}

fn shuffled_fixture() -> impl Strategy<Value = (CompositeDef, Vec<CompositeDef>)> {
    let (root, composites) = nested();
    let composites: Vec<_> = composites.into_iter().map(shuffled).collect();
    (shuffled(root), composites.prop_shuffle())
}

proptest! {
    #[test]
    fn declaration_order_does_not_change_the_plan((root, composites) in shuffled_fixture()) {
        let (base_root, base_composites) = nested();
        let expected = serde_json::to_string(&build(&base_root, &base_composites).unwrap()).unwrap();
        let actual = serde_json::to_string(&build(&root, &composites).unwrap()).unwrap();
        prop_assert_eq!(actual, expected);
    }
}
