//! M2.8-10 (operator decision operator-request:020, condition 4): the
//! oracle gives every instance it creates the same
//! `PrepareContext::generation` as the engine (`docs/semantics.md` §12,
//! "Added instances"): 0 for the built plan, the version a swap makes for an
//! instance it adds, a new one when an instance is removed and added again
//! at the same address. A fixture records the value it is prepared with and
//! touches no signal; the two runs' audio and logs also match.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;

use common::{Setup, compare, graph_of, run_engine};
use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor, Registry,
    Reproducibility,
};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_oracle::{Script, Swap};

thread_local! {
    /// The generations instances were prepared with, in order.
    static PREPARED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

const RECORDER: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.generation",
    doc: "test fixture: records the generation it is prepared with",
    ports: &[PortDescriptor {
        name: "out",
        direction: Direction::Out,
        shape: PortShape::Signal,
        rate: RateDivisor::ONE,
        kind: PortKind::Generic,
        unit: None,
        range: None,
    }],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: Reproducibility::BitExact,
    category: None,
};

struct Recorder;

impl DirectPrimitive for Recorder {
    fn describe() -> ModuleDescriptor {
        RECORDER
    }
    fn prepare(&mut self, ctx: &PrepareContext) {
        PREPARED.with(|p| p.borrow_mut().push(ctx.generation));
    }
    fn reset(&mut self) {}
    fn process(&mut self, _: &Inputs<'_>, _: &mut Outputs<'_>) {}
}

fn registry() -> Registry {
    let mut registry = common::registry();
    registry
        .register(ModuleType {
            descriptor: RECORDER,
            create: || Primitive::Direct(Box::new(Recorder)),
        })
        .unwrap();
    registry
}

/// `/a` and the probe, with `/b` when `with_b`, at plan version `version`.
fn plan(registry: &Registry, with_b: bool, version: u64) -> ExecutionPlan {
    let instance = |name: &str, module: &str| InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: BTreeMap::new(),
    };
    let mut instances = vec![
        instance("a", "test.generation"),
        instance("p", "oracle.probe"),
    ];
    let mut connections = vec![ConnectionDef {
        from: "a.out".to_owned(),
        to: "p.in0".to_owned(),
        scale: None,
    }];
    if with_b {
        instances.push(instance("b", "test.generation"));
        connections.push(ConnectionDef {
            from: "b.out".to_owned(),
            to: "p.in1".to_owned(),
            scale: None,
        });
    }
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, version).unwrap()
}

fn prepared() -> Vec<u64> {
    PREPARED.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// Swaps at 100 (adds `/b`), 200 (removes it) and 300 (adds it again):
/// `/a` gets 0, `/b` 1 and then 3, in the oracle and in the engine.
#[test]
fn the_oracle_and_the_engine_give_the_same_generations() {
    let registry = registry();
    let initial = plan(&registry, false, 0);
    let swaps: Vec<ExecutionPlan> = [(true, 1), (false, 2), (true, 3)]
        .into_iter()
        .map(|(with_b, version)| plan(&registry, with_b, version))
        .collect();
    let script = Script {
        sample_rate: 48_000,
        seed: 3,
        frames: 400,
        events: Vec::new(),
        edits: Vec::new(),
        swaps: swaps
            .iter()
            .zip([100, 200, 300])
            .map(|(p, frame)| Swap {
                frame,
                graph: graph_of(p, frame),
            })
            .collect(),
    };
    prepared();
    let oracle = signal_oracle::run(&registry, &graph_of(&initial, 0), &script).unwrap();
    let by_oracle = prepared();
    let setup = Setup {
        block: 64,
        threads: 1,
        splits: Vec::new(),
    };
    let engine = run_engine(&registry, &initial, &swaps, &script, &setup, &oracle);
    let by_engine = prepared();
    assert_eq!(by_oracle, vec![0, 1, 3]);
    assert_eq!(by_engine, by_oracle);
    compare(&oracle, &engine).unwrap();
}
