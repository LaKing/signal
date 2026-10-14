//! M2.6-1 AC4: block execution allocates and frees nothing, measured with the
//! counting allocator of `signal-contract` (ADR 0017): `run_block` with
//! in-block events, edits and faults, `commit_swap` between blocks, and the
//! `Runner` processing callbacks larger than its block capacity while the
//! control thread swaps plans and sends commands.

signal_contract::install_counting_allocator!();

mod common;

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    addr, config_threads, conn, engine_threads, ext, inst, inst_with, plan, port_addr, ramp,
    registry, scaled,
};
use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_core::{EventPayload, Frame};
use signal_engine::{
    ControlSink, HookPoint, Outcome, RealtimeConfig, Report, RtOptions, RtPolicy, SentCommand,
    Stamp,
};
use signal_graph::ExecutionPlan;
use signal_output::{CallbackSizes, realtime};

/// Heap calls counted on the worker threads while they take part in a block.
static WORKER_HEAP_CALLS: AtomicU64 = AtomicU64::new(0);
/// Times a worker entered a block under a guard.
static WORKER_ENTRIES: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static WORKER_GUARD: RefCell<Option<Guard>> = const { RefCell::new(None) };
}

/// The engine's test hook: a guard on each worker (lanes 1 and up) from the
/// moment it enters a block until it leaves it. The calling thread (lane 0)
/// is guarded by the test itself.
fn guard_workers(point: HookPoint) {
    match point {
        HookPoint::Enter { lane } if lane > 0 => {
            WORKER_GUARD.with(|g| *g.borrow_mut() = Some(Guard::start()));
            WORKER_ENTRIES.fetch_add(1, Ordering::Relaxed);
        }
        HookPoint::Leave { lane } if lane > 0 => {
            let counts = WORKER_GUARD.with(|g| g.borrow_mut().take().map(|g| g.counts()));
            if let Some(counts) = counts {
                WORKER_HEAP_CALLS
                    .fetch_add(counts.allocations + counts.deallocations, Ordering::Relaxed);
            }
        }
        _ => {}
    }
}

/// Tasks a worker started under [`guard_workers_with_workers`] (its test's
/// own count: the other tests of this file run beside it).
static WORKER_TASKS_HERE: AtomicU64 = AtomicU64::new(0);

/// [`guard_workers`], and the calling thread waits (up to 100 ms) at the start
/// of each job until a worker has started a task in it, so that workers run
/// tasks of these few blocks even where the calling thread alone would be
/// quicker (release builds): the path under test is the workers'.
fn guard_workers_with_workers(point: HookPoint) {
    match point {
        HookPoint::Enter { lane: 0 } => {
            let before = WORKER_TASKS_HERE.load(Ordering::SeqCst);
            let start = Instant::now();
            while WORKER_TASKS_HERE.load(Ordering::SeqCst) == before
                && start.elapsed() < Duration::from_millis(100)
            {
                std::hint::spin_loop();
            }
        }
        HookPoint::Task { lane, .. } if lane > 0 => {
            WORKER_TASKS_HERE.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
    guard_workers(point);
}

/// Blocks of several sizes, each with events pushed ahead to land inside it,
/// a suspend and a resume inside it, faults inside the first blocks (with the
/// undo of the frames after them), and a swap committed between blocks.
#[test]
fn blocks_and_swaps_allocate_nothing() {
    blocks_and_swaps(1);
}

/// M2.6-2 AC3: the same on 8 threads, every segment on the workers: neither
/// the calling thread nor any worker allocates or frees during a block or a
/// swap.
#[test]
fn blocks_and_swaps_on_workers_allocate_nothing() {
    blocks_and_swaps(8);
    assert!(WORKER_ENTRIES.load(Ordering::Relaxed) > 0);
    assert_eq!(WORKER_HEAP_CALLS.load(Ordering::Relaxed), 0);
}

fn blocks_and_swaps(threads: usize) {
    assert!(alloc_counter::is_installed());
    let base = || {
        vec![
            inst_with("o", "test.osc", &[("freq", 130.0)]),
            inst("d", "test.dc"),
            inst("pm", "test.probe_mul"),
            inst_with("t", "test.trigger", &[("period", 3.0)]),
            inst("c", "test.counter"),
            inst_with("flood", "test.flood", &[("count", 0.0)]),
            inst("c2", "test.counter"),
            inst("x", "test.faulty"),
            inst("u", "test.unit_delay"),
            inst("g", "test.gain"),
            inst("rec", "test.recorder"),
        ]
    };
    let wiring = || {
        vec![
            conn("o.out", "rec.zeta"),
            conn("d.out", "rec.alpha"),
            conn("d.out", "pm.value"),
            conn("t.out", "c.in"),
            conn("flood.out", "c2.in"),
            conn("x.out", "rec.zeta"),
            conn("g.out", "u.in"),
            conn("u.out", "g.in"),
            scaled("o.out", "g.in", 0.5),
        ]
    };
    let a = plan(base(), wiring());
    let mut with_noise = base();
    with_noise.push(inst("n", "test.noise"));
    let mut noise_wiring = wiring();
    noise_wiring.push(conn("n.out", "rec.alpha"));
    let b = plan(with_noise, noise_wiring);

    let mut engine = engine_threads(
        &a,
        1_000,
        threads,
        vec![
            ext(3, 1, "/pm.value", ramp(0.25, 30)),
            ext(2, 2, "/flood.count", ramp(30.0, 40)),
            ext(60, 3, "/flood.count", EventPayload::Set(1.0)),
        ],
    );
    engine.set_task_hook(Some(guard_workers_with_workers));
    let mut swaps = [Some(b.clone()), None, Some(a), None, Some(b)].into_iter();
    let mut sequence = 100;
    let mut old = Vec::new();
    for (round, k) in [7usize, 64, 256, 3, 128].into_iter().enumerate() {
        let start = engine.next_frame().0;
        let mut swap = swaps
            .next()
            .flatten()
            .map(|plan| engine.prepare_swap(&plan, &registry()).unwrap());
        let mut events: Vec<_> = (0..4u64)
            .map(|n| {
                sequence += 1;
                // Late, at the block's start, and ahead inside it.
                let frame = (start + n * k as u64 / 4).saturating_sub(2);
                ext(frame, sequence, "/c.in", EventPayload::Trigger)
            })
            .collect();
        let edits = [
            (addr("/o"), start + k as u64 / 3, true),
            (addr("/o"), start + k as u64 / 2 + 1, false),
        ];

        let guard = Guard::start();
        for event in events.drain(..) {
            engine.push_external(event).unwrap();
        }
        for (instance, at, suspend) in edits {
            if suspend {
                engine.push_suspend(instance, Frame(at)).unwrap();
            } else {
                engine.push_resume(instance, Frame(at)).unwrap();
            }
        }
        assert_eq!(engine.run_block(k), k);
        engine.drain_log(&mut |_| {});
        if let Some(swap) = swap.as_mut() {
            engine.commit_swap(swap, engine.next_frame()).unwrap();
        }
        let counts = guard.counts();
        drop(guard);
        assert!(
            counts.is_zero(),
            "block {round} (k = {k}) allocated: {counts:?}"
        );

        old.extend(swap);
        drop(engine.reclaim_externals().collect::<Vec<_>>());
        drop(engine.reclaim_edits().collect::<Vec<_>>());
    }
    assert_eq!(engine.version(), 3);
    assert_eq!(engine.log_overflows(), 0);
    assert_eq!(engine.input_overflows(), 0);
    assert_eq!(engine.is_muted(&addr("/x")), Some(true));
    assert_eq!(engine.is_muted(&addr("/flood")), Some(true));
    assert!(engine.signal_output(&port_addr("/c.out")).unwrap() > 0.0);
    assert!(old.iter().all(|swap| swap.is_committed()));
    if threads > 1 {
        let stats = engine.schedule_stats();
        assert!(
            stats.parallel_segments > 0 && stats.worker_tasks > 0,
            "{stats:?}"
        );
    }
}

/// M2.8-6 AC3a: divided-rate scheduling, vector buffers and converters
/// allocate and free nothing, on the calling thread and on the workers:
/// blocks of several sizes with ticks inside and across them, a parameter
/// set and a suspend and resume of a divided instance at frames that are
/// no tick, and swaps that remove and add a converter (and move the held
/// values of every survivor).
#[test]
fn divided_rates_and_vectors_allocate_nothing() {
    for threads in [1, 8] {
        let (a, b) = (common::rates_plan(false), common::rates_plan(true));
        let mut engine = engine_threads(&a, 48_000, threads, vec![]);
        engine.set_task_hook(Some(guard_workers_with_workers));
        let registry = registry();
        let mut swaps = [Some(b.clone()), None, Some(a), None, Some(b), None].into_iter();
        let mut sequence = 0;
        let mut old = Vec::new();
        for (round, k) in [7usize, 64, 256, 3, 128, 1].into_iter().enumerate() {
            let start = engine.next_frame().0;
            let mut swap = swaps
                .next()
                .flatten()
                .map(|plan| engine.prepare_swap(&plan, &registry).unwrap());
            sequence += 1;
            let event = ext(
                start + k as u64 / 2,
                sequence,
                "/vg.gain",
                EventPayload::Set(0.25 * round as f64 - 0.5),
            );
            // Addresses are made before the guard: a pushed edit owns one.
            let (suspended, resumed) = (addr("/o64"), addr("/o64"));
            let guard = Guard::start();
            engine.push_external(event).unwrap();
            engine
                .push_suspend(suspended, Frame(start + k as u64 / 3))
                .unwrap();
            engine
                .push_resume(resumed, Frame(start + k as u64 / 3 + 1))
                .unwrap();
            assert_eq!(engine.run_block(k), k);
            engine.drain_log(&mut |_| {});
            if let Some(swap) = swap.as_mut() {
                engine.commit_swap(swap, engine.next_frame()).unwrap();
            }
            let counts = guard.counts();
            drop(guard);
            assert!(
                counts.is_zero(),
                "{threads} threads, block {round} (k = {k}) allocated: {counts:?}"
            );
            old.extend(swap);
            drop(engine.reclaim_externals().collect::<Vec<_>>());
            drop(engine.reclaim_edits().collect::<Vec<_>>());
        }
        assert_eq!(engine.version(), 3);
        assert_eq!(engine.input_overflows(), 0);
        if threads > 1 {
            let stats = engine.schedule_stats();
            assert!(stats.worker_tasks > 0, "{stats:?}");
        }
    }
    assert_eq!(WORKER_HEAP_CALLS.load(Ordering::Relaxed), 0);
}

const RATE: u32 = 1_000;
const CALLBACKS: u64 = 3_000;
/// Callbacks up to four times the Runner's block capacity of 256.
const MOST_FRAMES: usize = 1_024;
const CHANNELS: usize = 2;

fn version(n: u64) -> ExecutionPlan {
    let mut instances = vec![
        inst_with("osc", "test.osc", &[("freq", 110.0)]),
        inst("dc", "test.dc"),
        inst_with("out", "test.gained_recorder", &[("gain", 0.5)]),
    ];
    let mut connections = vec![conn("osc.out", "out.l"), conn("dc.out", "out.r")];
    if n % 2 == 1 {
        instances.push(inst_with("osc2", "test.osc", &[("freq", 230.0)]));
        connections.push(scaled("osc2.out", "out.l", 0.5));
    }
    let mut plan = plan(instances, connections);
    plan.version = n;
    plan
}

#[derive(Default)]
struct Tally {
    events: u64,
    edits: u64,
    swaps: u64,
}

impl ControlSink for Tally {
    fn report(&mut self, report: &Report, command: &SentCommand) {
        match (report.outcome, command) {
            (Outcome::Committed { .. }, _) => self.swaps += 1,
            (Outcome::Scheduled, SentCommand::Event(_)) => self.events += 1,
            (Outcome::Scheduled, _) => self.edits += 1,
            _ => {}
        }
    }
}

/// The Runner splits each callback (up to 1024 frames) into blocks of up to
/// its capacity (256) at swap and command frames, while the control thread
/// swaps plans and sends events, suspends and resumes stamped ahead. The
/// audio thread counts every heap call inside `Runner::process`: none.
#[test]
fn runner_with_blocks_allocates_and_frees_nothing() {
    runner_with_blocks(1, RtOptions::default());
}

/// M2.6-2 AC3: the same on 4 threads, every segment on the workers, swaps
/// included: no heap call on the audio thread or on any worker.
#[test]
fn runner_on_workers_allocates_and_frees_nothing() {
    let entries = WORKER_ENTRIES.load(Ordering::Relaxed);
    runner_with_blocks(4, RtOptions::default());
    assert!(WORKER_ENTRIES.load(Ordering::Relaxed) > entries);
    assert_eq!(WORKER_HEAP_CALLS.load(Ordering::Relaxed), 0);
}

/// M2.7-1 C10: the same under every wake policy, with the wake-up trace on:
/// the callback start and end (the policy's wakes, the trace's stamps and
/// folds) are inside the guarded `Runner::process` too. The `semaphore`
/// policy parks here (no waiter in this crate); its Mach semaphores are
/// counted in `signal-audio/tests/no_allocation.rs`.
#[test]
fn runner_on_workers_allocates_and_frees_nothing_under_every_wake_policy() {
    for policy in RtPolicy::ALL {
        let entries = WORKER_ENTRIES.load(Ordering::Relaxed);
        let rt = RtOptions {
            policy,
            trace: true,
            ..RtOptions::default()
        };
        runner_with_blocks(4, rt);
        assert!(WORKER_ENTRIES.load(Ordering::Relaxed) > entries, "{policy}");
        assert_eq!(WORKER_HEAP_CALLS.load(Ordering::Relaxed), 0, "{policy}");
    }
}

fn runner_with_blocks(threads: usize, rt: RtOptions) {
    assert!(alloc_counter::is_installed());
    let mut config = RealtimeConfig::with_buffer(RATE, 42, MOST_FRAMES as u64);
    config.engine = config_threads(RATE, 42, threads);
    config.rt = rt;
    assert_eq!(config.engine.block_capacity, 256);
    let (mut runner, mut controller) = realtime(&version(0), &registry(), config).unwrap();
    runner.set_task_hook(Some(guard_workers));

    let audio = thread::spawn(move || {
        let mut sizes = CallbackSizes::new(5);
        let mut out = vec![0.0f32; MOST_FRAMES * CHANNELS];
        let mut total = Counts::default();
        for n in 0..CALLBACKS {
            let frames = if n % 2 == 0 {
                MOST_FRAMES
            } else {
                sizes.next_size().min(MOST_FRAMES)
            };
            let buffer = &mut out[..frames * CHANNELS];
            let guard = Guard::start();
            runner.process(buffer, CHANNELS);
            let counts = guard.counts();
            drop(guard);
            total.allocations += counts.allocations;
            total.deallocations += counts.deallocations;
        }
        (runner, total)
    });

    let plans: Vec<ExecutionPlan> = (1..=100).map(version).collect();
    let mut next_plan = 0;
    let mut tally = Tally::default();
    let mut step = 0u64;
    while !audio.is_finished() {
        step += 1;
        controller.poll(&mut tally);
        match step % 4 {
            0 => {
                if let Some(plan) = plans.get(next_plan)
                    && controller.swap(plan, Stamp::Default).is_ok()
                {
                    next_plan += 1;
                }
            }
            1 => {
                for k in 0..10 {
                    let value = EventPayload::Set(100.0 + k as f64);
                    let _ = controller.send_event(port_addr("/osc.freq"), value, Stamp::Default);
                }
            }
            2 => {
                let _ = controller.send_suspend(addr("/dc"), Stamp::Default);
            }
            _ => {
                let _ = controller.send_resume(addr("/dc"), Stamp::Default);
            }
        }
        thread::sleep(Duration::from_micros(200));
    }
    let (runner, total) = audio.join().unwrap();
    controller.poll(&mut tally);

    assert_eq!(
        total,
        Counts::default(),
        "heap calls inside Runner::process"
    );
    assert_eq!(runner.status().callbacks, CALLBACKS);
    assert!(tally.swaps >= 2, "swaps committed: {}", tally.swaps);
    assert!(tally.events > 0 && tally.edits > 0);
    if threads > 1 {
        assert!(runner.schedule_stats().worker_tasks > 0);
    }
}
