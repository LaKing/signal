//! `Inputs::sources_changed` (ADR 0037 §6) against the oracle, where it is
//! hardest to get right: a rate-divided instance (divisor 4) that sees a
//! swap between its ticks on its next tick; a state-output instance that
//! gets it in `absorb`; and an instance suspended across two swaps that
//! sees both on the call after its resume, although the second swap put
//! back the sources it had before. The block engine must match the oracle
//! bit for bit for every block size, partition and thread count.

mod common;

use common::{Setup, compare, graph_of, run_engine};
use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor, Registry,
    StateOutputPrimitive,
};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_oracle::{Edit, Script, Swap};

const fn port(name: &'static str, direction: Direction, rate: u32) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape: PortShape::Signal,
        rate: match RateDivisor::new(rate) {
            Some(r) => r,
            None => RateDivisor::ONE,
        },
        kind: PortKind::Control,
        unit: None,
        range: None,
    }
}

const fn descriptor(
    type_name: &'static str,
    ports: &'static [PortDescriptor],
    feedthrough: Feedthrough,
) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc: "fixture: shows whether a swap changed its input's sources",
        ports,
        parameters: &[],
        feedthrough,
        sink: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: None,
    }
}

const WATCH_PORTS: [PortDescriptor; 2] =
    [port("in", Direction::In, 1), port("out", Direction::Out, 1)];
const WATCH4_PORTS: [PortDescriptor; 2] =
    [port("in", Direction::In, 4), port("out", Direction::Out, 4)];

/// `out` is 1 on a call's first frame (tick) when `in`'s sources changed.
#[derive(Default)]
struct Watch<const D: u32>;

impl<const D: u32> DirectPrimitive for Watch<D> {
    fn describe() -> ModuleDescriptor {
        if D == 1 {
            descriptor("test.watch", &WATCH_PORTS, Feedthrough::Direct)
        } else {
            descriptor("test.watch4", &WATCH4_PORTS, Feedthrough::Direct)
        }
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let seen = inputs.sources_changed(0);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = f64::from(u8::from(n == 0 && seen));
        }
    }
}

const STATE_PORTS: [PortDescriptor; 2] =
    [port("in", Direction::In, 1), port("out", Direction::Out, 1)];

/// State-output: `out` shows what the last `absorb` saw.
#[derive(Default)]
struct WatchState {
    seen: f64,
}

impl StateOutputPrimitive for WatchState {
    fn describe() -> ModuleDescriptor {
        descriptor("test.watch_state", &STATE_PORTS, Feedthrough::StateOutput)
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {
        self.seen = 0.0;
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        if let Some(out) = outputs.signal(0).first_mut() {
            *out = self.seen;
        }
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        self.seen = f64::from(u8::from(inputs.sources_changed(0)));
    }
}

fn registry() -> Registry {
    let mut registry = common::registry();
    for module_type in [
        ModuleType {
            descriptor: Watch::<1>::describe(),
            create: || Primitive::Direct(Box::<Watch<1>>::default()),
        },
        ModuleType {
            descriptor: Watch::<4>::describe(),
            create: || Primitive::Direct(Box::<Watch<4>>::default()),
        },
        ModuleType {
            descriptor: WatchState::describe(),
            create: || Primitive::StateOutput(Box::<WatchState>::default()),
        },
    ] {
        registry.register(module_type).unwrap();
    }
    registry
}

fn instance(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect(),
        scope: None,
    }
}

fn wire(from: &str, to: &str, scale: f64) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: Some(scale),
    }
}

/// The three watchers fed by `source` (`c` or `d`), heard on the sink.
fn plan(source: &str, version: u64) -> ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            instance("c", "std.const", &[("value", 0.5)]),
            instance("d", "std.const", &[("value", 0.25)]),
            instance("w4", "test.watch4", &[]),
            instance("ws", "test.watch_state", &[]),
            instance("wp", "test.watch", &[]),
            instance("out", "std.audio_out", &[]),
        ],
        connections: vec![
            wire(&format!("{source}.out"), "w4.in", 1.0),
            wire(&format!("{source}.out"), "ws.in", 1.0),
            wire(&format!("{source}.out"), "wp.in", 1.0),
            wire("w4.out", "out.l", 1.0),
            wire("ws.out", "out.r", 0.5),
            wire("wp.out", "out.r", 0.25),
        ],
        exports: Exports::default(),
    };
    let mut plan = build_plan(&root, &[], &registry(), 0).unwrap();
    plan.version = version;
    plan
}

#[test]
fn changed_sources_reach_divided_state_output_and_suspended_instances_alike() {
    let registry = registry();
    let first = plan("c", 0);
    // Swaps between w4's ticks (101, 203); wp suspended over both, back to
    // its first sources by the second, resumed at 250.
    let swaps = vec![(101, plan("d", 1)), (203, plan("c", 2))];
    let script = Script {
        sample_rate: 1000,
        seed: 3,
        frames: 300,
        events: Vec::new(),
        edits: vec![
            Edit {
                frame: 90,
                instance: "/wp".parse().unwrap(),
                resume: false,
            },
            Edit {
                frame: 250,
                instance: "/wp".parse().unwrap(),
                resume: true,
            },
        ],
        swaps: swaps
            .iter()
            .map(|(frame, plan)| Swap {
                frame: *frame,
                graph: graph_of(plan, *frame),
            })
            .collect(),
    };
    let plans: Vec<ExecutionPlan> = swaps.into_iter().map(|(_, p)| p).collect();
    let oracle = signal_oracle::run(&registry, &graph_of(&first, 1), &script).unwrap();
    for block in [1, 3, 4, 7, 64, 256] {
        for threads in [1, 2, 4] {
            let setup = Setup {
                block,
                threads,
                splits: vec![50, 102, 205],
            };
            let engine = run_engine(&registry, &first, &plans, &script, &setup, &oracle);
            if let Err(problem) = compare(&oracle, &engine) {
                panic!("{setup:?}: {problem}");
            }
        }
    }
    let l: Vec<f64> = oracle.samples.iter().step_by(2).copied().collect();
    let r: Vec<f64> = oracle.samples.iter().skip(1).step_by(2).copied().collect();
    // w4 (divisor 4) sees each swap on its next tick: 104 and 204, and
    // shows it until its next tick (through the converter to the sink).
    let marked: Vec<usize> = (0..300).filter(|&n| l[n] != 0.0).collect();
    assert!(
        !marked.is_empty()
            && marked
                .iter()
                .all(|&n| (104..112).contains(&n) || (204..212).contains(&n)),
        "{marked:?}"
    );
    assert!(marked.iter().any(|&n| n < 200) && marked.iter().any(|&n| n > 200));
    // ws absorbs the mark on the swap frame and emits it on the next.
    assert_eq!((r[102], r[204]), (0.5, 0.5));
    // wp, suspended from 90 to 250 across both swaps, sees them at 250.
    assert_eq!(r[250], 0.25);
    let others: Vec<usize> = (0..300)
        .filter(|&n| r[n] != 0.0 && ![102, 204, 250].contains(&n))
        .collect();
    assert_eq!(others, Vec::<usize>::new());
}
