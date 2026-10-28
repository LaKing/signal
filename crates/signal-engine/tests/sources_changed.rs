//! Changed sources (ADR 0037 §6): the first call after a swap tells a
//! surviving instance which of its inputs the swap gave another set of
//! sources (`Inputs::sources_changed`), domain-free, at that call's first
//! frame only, whatever the block size, partition and thread count; an
//! added instance and an unchanged input read false; an instance suspended
//! across the swap sees it on its first call after its resume; and the
//! swaps allocate nothing on the calling thread.

signal_contract::install_counting_allocator!();

mod common;

use common::{config_threads, inst, registry};
use signal_contract::alloc_counter::{self, Guard};
use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Frame, Inputs, InstanceAddress, ModuleDescriptor,
    ModuleType, Outputs, PortDescriptor, PortKind, PortShape, PrepareContext, Primitive,
    RateDivisor, Registry,
};
use signal_engine::Engine;
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, build_plan};

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

/// `test.watch`: `seen_in` and `seen_ev` are 1 on a call's first frame
/// when its `in` (signal) and `ev` (events) had their sources changed,
/// else 0.
const WATCH: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.watch",
    doc: "fixture: shows which inputs' sources a swap changed",
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
struct Watch;

impl DirectPrimitive for Watch {
    fn describe() -> ModuleDescriptor {
        WATCH
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        // Outputs and indices past the ports read unchanged.
        let quiet = !inputs.sources_changed(2) && !inputs.sources_changed(99);
        for row in 0..2 {
            let seen = inputs.sources_changed(row);
            for (n, out) in outputs.signal(row).iter_mut().enumerate() {
                *out = match (quiet, n == 0 && seen) {
                    (false, _) => -1.0,
                    (true, true) => 1.0,
                    (true, false) => 0.0,
                };
            }
        }
    }
}

fn watch_registry() -> Registry {
    let mut registry = registry();
    registry
        .register(ModuleType {
            descriptor: WATCH,
            create: || Primitive::Direct(Box::new(Watch)),
        })
        .unwrap();
    registry
}

/// A watch `w` (and `w2` when `second`) with `connections` (from, to,
/// scale) among two `test.const`s `c`, `d` and a `test.trigger` `t`.
fn plan(connections: &[(&str, &str, Option<f64>)], second: bool) -> ExecutionPlan {
    let mut instances = vec![
        inst("w", "test.watch"),
        inst("c", "test.const"),
        inst("d", "test.const"),
        inst("t", "test.trigger"),
    ];
    if second {
        instances.push(inst("w2", "test.watch"));
    }
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections: connections
            .iter()
            .map(|(f, t, scale)| ConnectionDef {
                from: (*f).to_owned(),
                to: (*t).to_owned(),
                scale: *scale,
            })
            .collect(),
        exports: Exports::default(),
    };
    build_plan(&root, &[], &watch_registry(), 0).unwrap()
}

/// (seen_in, seen_ev) of `watch` per frame over `frames`, swapping from
/// `first` to `next` at `at`, in blocks of `k` on `threads` threads, with
/// `watch` suspended over `suspended` (frames) if given.
fn run(
    (first, next): (&ExecutionPlan, &ExecutionPlan),
    at: u64,
    frames: u64,
    (k, threads): (usize, usize),
    watch: &str,
    suspended: Option<(u64, u64)>,
) -> Vec<(f64, f64)> {
    let registry = watch_registry();
    let mut engine = Engine::new(
        first,
        &registry,
        config_threads(1_000, 1, threads),
        Vec::new(),
    )
    .unwrap();
    let address: InstanceAddress = format!("/{watch}").parse().unwrap();
    if let Some((from, to)) = suspended {
        engine.suspend(&"/w".parse().unwrap(), Frame(from)).unwrap();
        engine.resume(&"/w".parse().unwrap(), Frame(to)).unwrap();
    }
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
        // `None` before the swap adds the watched instance.
        let index =
            |name: &str| engine.signal_output_index(&format!("{address}.{name}").parse().unwrap());
        let (a, b) = (index("seen_in"), index("seen_ev"));
        let value = |i: Option<usize>, f: usize| {
            i.and_then(|i| engine.signal_output_at(i, f)).unwrap_or(0.0)
        };
        for f in 0..len {
            trace.push((value(a, f), value(b, f)));
        }
        engine.drain_log(&mut |_| {});
    }
    trace
}

/// What a swap changes is seen once, on the swap frame, for every block
/// size and thread count.
#[test]
fn a_rewired_input_is_seen_once_at_the_swap() {
    let c_in = plan(&[("c.out", "w.in", None)], false);
    let d_in = plan(&[("d.out", "w.in", None)], false);
    let c_half = plan(&[("c.out", "w.in", Some(0.5))], false);
    let both = plan(&[("c.out", "w.in", None), ("d.out", "w.in", None)], false);
    let events = plan(&[("c.out", "w.in", None), ("t.out", "w.ev", None)], false);
    let none = plan(&[], false);
    let expect = |at: usize, seen: (f64, f64)| {
        let mut trace = vec![(0.0, 0.0); 40];
        trace[at] = seen;
        trace
    };
    for k in [1, 3, 64] {
        for threads in [1, 4] {
            let setup = (k, threads);
            let case = |from: &ExecutionPlan, to: &ExecutionPlan| {
                run((from, to), 13, 40, setup, "w", None)
            };
            assert_eq!(case(&c_in, &d_in), expect(13, (1.0, 0.0)), "another source");
            assert_eq!(
                case(&c_in, &c_half),
                expect(13, (1.0, 0.0)),
                "another scale"
            );
            assert_eq!(case(&c_in, &both), expect(13, (1.0, 0.0)), "one more");
            assert_eq!(case(&c_in, &none), expect(13, (1.0, 0.0)), "none left");
            assert_eq!(case(&none, &c_in), expect(13, (1.0, 0.0)), "the first");
            assert_eq!(case(&c_in, &events), expect(13, (0.0, 1.0)), "events");
            assert_eq!(case(&c_in, &c_in), vec![(0.0, 0.0); 40], "the same");
            // Duplicate connections from one source with other scales are
            // allowed; their canonical order is by scale (total order),
            // whatever order the file writes them in, so the same set
            // written otherwise is no change, and another scale is.
            let twice = plan(
                &[("c.out", "w.in", Some(-2.0)), ("c.out", "w.in", Some(0.5))],
                false,
            );
            let swapped = plan(
                &[("c.out", "w.in", Some(0.5)), ("c.out", "w.in", Some(-2.0))],
                false,
            );
            let other = plan(
                &[("c.out", "w.in", Some(0.5)), ("c.out", "w.in", Some(-1.0))],
                false,
            );
            assert_eq!(case(&twice, &swapped), vec![(0.0, 0.0); 40], "reordered");
            assert_eq!(case(&twice, &other), expect(13, (1.0, 0.0)), "rescaled");
        }
    }
    // An added instance has its own first frame: it reads false.
    let added = plan(&[("c.out", "w.in", None), ("d.out", "w2.in", None)], true);
    let trace = run((&c_in, &added), 13, 40, (4, 1), "w2", None);
    assert!(trace[13..].iter().all(|&t| t == (0.0, 0.0)), "{trace:?}");
}

/// Suspended across the swap, the instance sees the change on its first
/// call after the resume (the change it has not seen yet).
#[test]
fn a_suspended_instance_sees_the_change_when_it_resumes() {
    let c_in = plan(&[("c.out", "w.in", None)], false);
    let d_in = plan(&[("d.out", "w.in", None)], false);
    for k in [1, 5, 64] {
        let trace = run((&c_in, &d_in), 13, 40, (k, 1), "w", Some((10, 20)));
        // Its suspended outputs hold their last value (0): the change shows
        // on the resume frame alone.
        let seen: Vec<usize> = (0..40).filter(|&n| trace[n].0 == 1.0).collect();
        assert_eq!(seen, [20], "k {k}");
    }
}

/// Committing swaps that change sources, and the calls that see them,
/// allocate and free nothing on the calling thread.
#[test]
fn a_swap_of_sources_allocates_nothing() {
    assert!(alloc_counter::is_installed());
    let registry = watch_registry();
    let mut plans = [
        plan(&[("c.out", "w.in", None)], false),
        plan(&[("d.out", "w.in", None)], false),
        plan(
            &[("d.out", "w.in", Some(2.0)), ("t.out", "w.ev", None)],
            false,
        ),
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
