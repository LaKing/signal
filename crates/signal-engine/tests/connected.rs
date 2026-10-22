//! Connected inputs (ADR 0033): every call tells a primitive which of its
//! ports the plan connects (`Inputs::is_connected`), domain-free; a swap
//! that connects or disconnects an input changes it at the swap frame
//! without allocating on the calling thread; and a plan that connects both
//! inputs of a pair the registry makes exclusive is refused, with the
//! connection named.

signal_contract::install_counting_allocator!();

mod common;

use common::{config_threads, inst, registry};
use signal_contract::alloc_counter::{self, Guard};
use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Frame, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor, Registry,
};
use signal_engine::Engine;
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, GraphError, build_plan};

const fn port(name: &'static str, direction: Direction, shape: PortShape) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape,
        rate: RateDivisor::ONE,
        kind: PortKind::Control,
        unit: None,
        range: None,
    }
}

/// `test.probe`: `seen_in` and `seen_ev` are 1 while its `in` (signal) and
/// `ev` (events) inputs are connected, else 0.
const PROBE: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.probe",
    doc: "fixture: shows which inputs are connected",
    ports: &[
        port("in", Direction::In, PortShape::Signal),
        port("ev", Direction::In, PortShape::Events),
        port("seen_in", Direction::Out, PortShape::Signal),
        port("seen_ev", Direction::Out, PortShape::Signal),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: None,
};

#[derive(Default)]
struct Probe;

impl DirectPrimitive for Probe {
    fn describe() -> ModuleDescriptor {
        PROBE
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let seen = [inputs.is_connected(0), inputs.is_connected(1)];
        // Outputs and indices past the ports read unconnected.
        let quiet = !inputs.is_connected(2) && !inputs.is_connected(99);
        for (row, seen) in seen.into_iter().enumerate() {
            for out in outputs.signal(row).iter_mut() {
                *out = if quiet {
                    f64::from(u8::from(seen))
                } else {
                    -1.0
                };
            }
        }
    }
}

fn probe_registry() -> Registry {
    let mut registry = registry();
    registry
        .register(ModuleType {
            descriptor: PROBE,
            create: || Primitive::Direct(Box::new(Probe)),
        })
        .unwrap();
    registry.exclusive_inputs("test.probe", "in", "ev");
    registry
}

fn conn(from: &str, to: &str) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    }
}

/// A probe `p` with `connections` into it from a `test.const` `c` and a
/// `test.trigger` `t`.
fn plan(connections: &[(&str, &str)]) -> Result<ExecutionPlan, Vec<GraphError>> {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            inst("p", "test.probe"),
            inst("c", "test.const"),
            inst("t", "test.trigger"),
        ],
        connections: connections.iter().map(|(f, t)| conn(f, t)).collect(),
        exports: Exports::default(),
    };
    build_plan(&root, &[], &probe_registry(), 0)
}

/// (seen_in, seen_ev) per frame over `frames`, swapping from `first` to
/// `next` at `at`, in blocks of `k` on `threads` threads.
fn run(
    first: &ExecutionPlan,
    next: &ExecutionPlan,
    at: u64,
    frames: u64,
    k: usize,
    threads: usize,
) -> Vec<(f64, f64)> {
    let registry = probe_registry();
    let mut engine = Engine::new(
        first,
        &registry,
        config_threads(1_000, 1, threads),
        Vec::new(),
    )
    .unwrap();
    let mut next = next.clone();
    next.version = 1;
    let mut swap = Some(engine.prepare_swap(&next, &registry).unwrap());
    let mut trace = Vec::new();
    while engine.next_frame().0 < frames {
        let now = engine.next_frame().0;
        if now == at
            && let Some(mut prepared) = swap.take()
        {
            engine.drain_log(&mut |_| {});
            engine.commit_swap(&mut prepared, Frame(at)).unwrap();
        }
        let limit = if now < at { at } else { frames };
        let len = k.min(usize::try_from(limit - now).unwrap());
        assert_eq!(engine.run_block(len), len, "{:?}", engine.failure());
        let index = |name: &str| engine.signal_output_index(&name.parse().unwrap()).unwrap();
        let (a, b) = (index("/p.seen_in"), index("/p.seen_ev"));
        for f in 0..len {
            trace.push((
                engine.signal_output_at(a, f).unwrap(),
                engine.signal_output_at(b, f).unwrap(),
            ));
        }
        engine.drain_log(&mut |_| {});
    }
    trace
}

/// What the plan connects is what the probe sees, from the swap frame on,
/// for every block size and thread count.
#[test]
fn a_probe_sees_its_connections_and_a_swap_changes_them() {
    let none = plan(&[]).unwrap();
    let signal = plan(&[("c.out", "p.in")]).unwrap();
    let events = plan(&[("t.out", "p.ev")]).unwrap();
    let trace = run(&none, &signal, 10, 20, 4, 1);
    assert_eq!(trace[..10], [(0.0, 0.0); 10]);
    assert_eq!(trace[10..], [(1.0, 0.0); 10]);
    for k in [1, 3, 64] {
        for threads in [1, 4] {
            let trace = run(&signal, &events, 7, 16, k, threads);
            assert_eq!(trace[..7], [(1.0, 0.0); 7], "k {k}, threads {threads}");
            assert_eq!(trace[7..], [(0.0, 1.0); 9], "k {k}, threads {threads}");
        }
    }
}

/// Committing swaps that connect and disconnect, and running the frames
/// around them, allocate and free nothing on the calling thread.
#[test]
fn a_swap_of_connections_allocates_nothing() {
    assert!(alloc_counter::is_installed());
    let registry = probe_registry();
    let mut plans = [
        plan(&[]).unwrap(),
        plan(&[("c.out", "p.in")]).unwrap(),
        plan(&[("t.out", "p.ev")]).unwrap(),
    ];
    for (v, p) in plans.iter_mut().enumerate() {
        p.version = v as u64;
    }
    let mut engine = Engine::new(
        &plans[0],
        &registry,
        config_threads(1_000, 1, 1),
        Vec::new(),
    )
    .unwrap();
    let mut old = Vec::new();
    for (step, next) in plans[1..].iter().enumerate() {
        let at = 100 * (step as u64 + 1);
        let mut swap = engine.prepare_swap(next, &registry).unwrap();
        let guard = Guard::start();
        while engine.next_frame().0 < at {
            let len = 64.min(usize::try_from(at - engine.next_frame().0).unwrap());
            engine.run_block(len);
            engine.drain_log(&mut |_| {});
        }
        engine.commit_swap(&mut swap, Frame(at)).unwrap();
        engine.run_block(64);
        engine.drain_log(&mut |_| {});
        let counts = guard.counts();
        drop(guard);
        assert!(counts.is_zero(), "swap {step} allocated: {counts:?}");
        old.push(swap);
    }
}

/// Both inputs of an exclusive pair connected: refused, on a connection
/// into the second, naming both ports; one of them alone plans.
#[test]
fn exclusive_inputs_are_refused_together() {
    let errors = plan(&[("c.out", "p.in"), ("t.out", "p.ev")]).unwrap_err();
    assert_eq!(errors.len(), 1);
    let GraphError::ExclusiveInputs {
        from,
        to,
        inputs,
        type_name,
    } = &errors[0]
    else {
        panic!("{errors:?}");
    };
    assert_eq!(
        (from.to_string(), to.to_string()),
        ("/t.out".to_owned(), "/p.ev".to_owned())
    );
    assert_eq!(
        (inputs.0.to_string(), inputs.1.to_string()),
        ("/p.ev".to_owned(), "/p.in".to_owned())
    );
    assert_eq!(*type_name, "test.probe");
    assert_eq!(
        errors[0].to_string(),
        "/t.out -> /p.ev: /p.ev and /p.in are both connected; a test.probe takes `in` or `ev`, not both"
    );
    assert!(plan(&[("c.out", "p.in")]).is_ok());
    assert!(plan(&[("t.out", "p.ev"), ("t.out", "p.ev")]).is_ok());
}
