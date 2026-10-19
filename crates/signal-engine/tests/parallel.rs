//! M2.6-2 AC2 (ADR 0019 §2): 1 000 blocks on 8 threads, with a random delay
//! of 0–50 µs at the start of every task (the engine's test hook), give the
//! same audio, log and counters bit for bit as one thread, with the checked
//! mode's exclusivity tracker on. The plan sums several signal sources into
//! one input, modulates parameters, merges events from three sources into
//! one input, runs a feedback loop and a fault (with the undo of the frames
//! after it), and takes pushed external events in every block. The tracker's
//! own figures are checked only where it is compiled in (debug builds); the
//! results and the refusals are checked in every build (M2.7-3, finding 6).
//! A failing task: `failure_stop.rs`.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use common::{config_threads, conn, ext, inst, inst_with, plan, registry, scaled};
use signal_core::{EventPayload, Frame, SplitMix64};
use signal_engine::{Engine, EngineConfig, HookPoint, RtPolicy, Schedule};
use signal_graph::ExecutionPlan;
use signal_output::{Render, render_blocks};

const RATE: u32 = 1_000;
const BLOCK: usize = 256;
const BLOCKS: u64 = 1_000;

fn stress_plan() -> ExecutionPlan {
    plan(
        vec![
            inst_with("f1", "test.flood", &[("count", 4.0)]),
            inst_with("f2", "test.flood", &[("count", 3.0)]),
            inst_with("t", "test.trigger", &[("period", 3.0)]),
            inst("c", "test.counter"),
            inst_with("s", "test.setter", &[("value", 0.25)]),
            inst_with("k", "test.const", &[("value", 0.125)]),
            inst("p", "test.probe_add"),
            inst("e", "test.event_delay"),
            inst_with("o1", "test.osc", &[("freq", 110.0)]),
            inst_with("o2", "test.osc", &[("freq", 230.0)]),
            inst("n", "test.noise"),
            inst("sum", "test.gain"),
            inst("impulse", "test.impulse"),
            inst("loop", "test.gain"),
            inst("delay", "test.unit_delay"),
            inst("d", "test.dc"),
            inst("x", "test.faulty"),
            inst("rec", "test.recorder"),
        ],
        vec![
            // Three event sources into one input.
            conn("f1.out", "c.in"),
            conn("f2.out", "c.in"),
            conn("t.out", "c.in"),
            // Events and modulation into one parameter.
            conn("s.out", "p.value"),
            conn("k.out", "p.value"),
            conn("t.out", "p.value"),
            conn("t.out", "e.in"),
            // Three signal sources into one input, its gain modulated.
            conn("o1.out", "sum.in"),
            scaled("o2.out", "sum.in", 0.5),
            scaled("n.out", "sum.in", 0.25),
            scaled("d.out", "sum.gain", 0.5),
            // A feedback loop.
            conn("impulse.out", "loop.in"),
            scaled("delay.out", "loop.in", 0.5),
            conn("loop.out", "delay.in"),
            // The sink.
            conn("sum.out", "rec.zeta"),
            conn("x.out", "rec.zeta"),
            conn("loop.out", "rec.alpha"),
            conn("p.out", "rec.alpha"),
            conn("c.out", "rec.alpha"),
            conn("e.out", "rec.alpha"),
        ],
    )
}

/// The hook's random stream (any order of draws is fine: only timing).
static DELAY_STATE: AtomicU64 = AtomicU64::new(7);
static DELAYS: AtomicU64 = AtomicU64::new(0);

/// Spins 0–50 µs before every task.
fn random_delay(point: HookPoint) {
    if let HookPoint::Task { .. } = point {
        let draw = DELAY_STATE.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        let micros = SplitMix64::new(draw).next_u64() % 51;
        let until = Instant::now() + Duration::from_micros(micros);
        while Instant::now() < until {
            std::hint::spin_loop();
        }
        DELAYS.fetch_add(1, Ordering::Relaxed);
    }
}

fn render(threads: usize, delays: bool) -> (Render, Engine) {
    render_with(config_threads(RATE, 42, threads), delays)
}

fn render_with(config: EngineConfig, delays: bool) -> (Render, Engine) {
    render_callbacks(config, delays, false)
}

/// [`render_with`], and with `callbacks` each block is a callback of a wake
/// policy (`begin_callback`/`end_callback`), after a random gap of 0 to
/// 255 µs (from the same stream as the delays), so that idle workers spin,
/// park and are woken at every phase of their wait.
fn render_callbacks(config: EngineConfig, delays: bool, callbacks: bool) -> (Render, Engine) {
    let hook: Option<fn(HookPoint)> = delays.then_some(random_delay);
    render_hooked(config, hook, callbacks)
}

/// [`render_callbacks`] with `hook` as the task hook.
fn render_hooked(
    config: EngineConfig,
    hook: Option<fn(HookPoint)>,
    callbacks: bool,
) -> (Render, Engine) {
    let mut engine = Engine::new(&stress_plan(), &registry(), config, Vec::new()).unwrap();
    engine.set_task_hook(hook);
    let mut sequence = 0;
    let rendered = render_blocks(&mut engine, BLOCKS * BLOCK as u64, BLOCK, |engine| {
        if callbacks {
            engine.end_callback();
            let draw = DELAY_STATE.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
            let gap = SplitMix64::new(draw).next_u64() % 256;
            std::thread::sleep(Duration::from_micros(gap));
            engine.begin_callback(BLOCK);
        }
        engine.reclaim_externals().for_each(drop);
        let start = engine.next_frame().0;
        for (n, (target, payload)) in [
            ("/c.in", EventPayload::Trigger),
            ("/p.value", EventPayload::Set(0.75)),
            (
                "/sum.gain",
                EventPayload::Set(0.5 + (start % 7) as f64 / 10.0),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            sequence += 1;
            let at = start + (n as u64 * 97 + start / BLOCK as u64 * 13) % BLOCK as u64;
            engine
                .push_external(ext(at, sequence, target, payload))
                .unwrap();
        }
        Ok(Some(Frame(start + BLOCK as u64)))
    })
    .unwrap();
    (rendered, engine)
}

#[test]
fn a_thousand_blocks_on_eight_threads_with_random_delays_match_one_thread() {
    let (reference, one) = render(1, false);
    let (stressed, eight) = render(8, true);
    assert_eq!(stressed.samples.len(), reference.samples.len());
    assert!(
        stressed.samples == reference.samples,
        "the audio differs on 8 threads"
    );
    assert!(
        stressed.log == reference.log,
        "the log differs on 8 threads"
    );
    assert_eq!(eight.log_overflows(), one.log_overflows());
    assert_eq!(eight.input_overflows(), one.input_overflows());
    assert_eq!(eight.input_discards(), one.input_discards());
    assert_eq!(one.log_overflows(), 0);
    assert_eq!(one.input_overflows(), 0);
    assert_eq!(
        eight.is_muted(&common::addr("/x")),
        Some(true),
        "the fault fired"
    );

    let stats = eight.schedule_stats();
    assert_eq!(stats.threads, 8);
    assert_eq!(stats.parallel_segments, BLOCKS);
    assert!(stats.worker_tasks > 0, "{stats:?}");
    assert_eq!(stats.task_failures, 0);
    assert!(DELAYS.load(Ordering::Relaxed) >= BLOCKS * stats.tasks as u64);
    let exclusivity = signal_engine::exclusivity();
    assert_eq!(exclusivity.violations, 0);
    // Instrumentation: the tracker exists in debug builds only.
    if exclusivity.checked {
        assert!(exclusivity.claims > 0);
    }
}

/// M2.6-5: the real-time spin changes only when a worker runs a task. With
/// idle workers spinning for the longest the pool allows (so they rarely
/// park between blocks), 8 threads give the same audio, log and counters as
/// one thread, with the random delays and the exclusivity tracker on.
#[test]
fn spinning_workers_match_one_thread() {
    let (reference, one) = render(1, false);
    let mut config = config_threads(RATE, 42, 8);
    config.schedule = Schedule {
        spin_ns: u64::MAX,
        ..config.schedule
    };
    let (stressed, eight) = render_with(config, true);
    assert!(
        stressed.samples == reference.samples,
        "the audio differs with spinning workers"
    );
    assert!(
        stressed.log == reference.log,
        "the log differs with spinning workers"
    );
    assert_eq!(eight.log_overflows(), one.log_overflows());
    assert_eq!(eight.input_overflows(), one.input_overflows());
    assert_eq!(eight.input_discards(), one.input_discards());
    let stats = eight.schedule_stats();
    assert_eq!(stats.parallel_segments, BLOCKS);
    assert!(stats.worker_tasks > 0, "{stats:?}");
    assert_eq!(stats.task_failures, 0);
    assert_eq!(signal_engine::exclusivity().violations, 0);
}

/// M2.7-1 C12: the stress test under every wake policy. 1 000 blocks on 8
/// threads as callbacks with random gaps between them (so workers are
/// spinning, parking or parked when the next one starts) and random task
/// delays give the same audio, log and counters as one thread; nothing
/// deadlocks, and the trace sees every wake it sent once the workers rest.
/// The `semaphore` policy here parks (no waiter in the engine crate); its
/// Mach semaphores are tested in `signal-audio/tests/rt_workers.rs`.
#[test]
fn every_wake_policy_matches_one_thread_under_random_delays_and_gaps() {
    let (reference, one) = render(1, false);
    for policy in RtPolicy::ALL {
        let mut config = config_threads(RATE, 42, 8);
        config.schedule = Schedule {
            policy,
            budget_ns: 400_000,
            trace: true,
            ..config.schedule
        };
        let (stressed, eight) = render_callbacks(config, true, true);
        assert!(
            stressed.samples == reference.samples,
            "{policy}: the audio differs"
        );
        assert!(stressed.log == reference.log, "{policy}: the log differs");
        assert_eq!(eight.log_overflows(), one.log_overflows());
        assert_eq!(eight.input_overflows(), one.input_overflows());
        assert_eq!(eight.input_discards(), one.input_discards());
        let stats = eight.schedule_stats();
        assert_eq!((stats.policy, stats.waiter), (policy, "park"));
        assert_eq!(stats.parallel_segments, BLOCKS);
        assert!(stats.worker_tasks > 0, "{policy}: {stats:?}");
        assert_eq!(stats.task_failures, 0);
        let wake = eight.wake_stats().unwrap();
        assert!(wake.callbacks >= BLOCKS - 1, "{policy}: {wake:?}");
        assert!(wake.wakes_observed <= wake.wakes_signalled, "{wake:?}");
        assert!(
            wake.slowest_task.count > 0 && wake.last_join.count > 0,
            "{wake:?}"
        );
    }
    assert_eq!(signal_engine::exclusivity().violations, 0);
}

/// The highest lane that ran a task under [`lane_and_delay`], plus one.
static LANES: AtomicU64 = AtomicU64::new(0);

/// [`random_delay`], recording the lane of every task.
fn lane_and_delay(point: HookPoint) {
    if let HookPoint::Task { lane, .. } = point {
        LANES.fetch_max(lane as u64 + 1, Ordering::Relaxed);
    }
    random_delay(point);
}

/// M2.10-5 (AC2, AC3): with the worker budget on (`Schedule::plan_budget`),
/// under every wake policy, 1 000 callbacks on 8 threads with random gaps
/// and delays run tasks only on the budget's lanes, on some worker, and give
/// the same audio, log and counters as one thread.
#[test]
fn the_worker_budget_bounds_the_lanes_and_matches_one_thread() {
    let (reference, one) = render(1, false);
    for policy in RtPolicy::ALL {
        let mut config = config_threads(RATE, 42, 8);
        config.schedule = Schedule {
            policy,
            plan_budget: true,
            trace: true,
            ..config.schedule
        };
        LANES.store(0, Ordering::Relaxed);
        let (stressed, eight) = render_hooked(config, Some(lane_and_delay), true);
        assert!(
            stressed.samples == reference.samples,
            "{policy}: the audio differs"
        );
        assert!(stressed.log == reference.log, "{policy}: the log differs");
        assert_eq!(eight.log_overflows(), one.log_overflows());
        assert_eq!(eight.input_overflows(), one.input_overflows());
        assert_eq!(eight.input_discards(), one.input_discards());
        let budget = eight.schedule_stats().worker_budget;
        assert!((2..8).contains(&budget), "{policy}: budget {budget}");
        let lanes = LANES.load(Ordering::Relaxed);
        assert!(
            lanes as usize <= budget,
            "{policy}: lane {lanes} of {budget}"
        );
        let stats = eight.schedule_stats();
        assert_eq!(stats.parallel_segments, BLOCKS);
        assert!(stats.worker_tasks > 0, "{policy}: {stats:?}");
        assert_eq!(stats.task_failures, 0);
        let wake = eight.wake_stats().unwrap();
        assert_eq!(wake.worker_budget, budget);
        assert!(
            wake.job_ns[budget - 1..].iter().all(|&ns| ns == 0),
            "{policy}: a worker beyond the budget joined: {wake:?}"
        );
    }
    assert_eq!(signal_engine::exclusivity().violations, 0);
}

/// M2.7-1: the callback-start wake of `semaphore` and `fanout` comes only
/// when the running plan can run on the workers in the callback: never for a
/// callback shorter than the schedule's `min_frames`.
#[test]
fn the_callback_start_wake_needs_a_callback_that_can_go_parallel() {
    for policy in [RtPolicy::Semaphore, RtPolicy::Fanout] {
        let mut config = config_threads(RATE, 42, 4);
        config.schedule = Schedule {
            policy,
            min_frames: 16,
            trace: true,
            ..config.schedule
        };
        let mut engine = Engine::new(&stress_plan(), &registry(), config, Vec::new()).unwrap();
        // Idle since their start: every worker parks within its 50 µs spin.
        std::thread::sleep(Duration::from_millis(100));
        engine.begin_callback(15);
        engine.end_callback();
        assert_eq!(engine.wake_stats().unwrap().wakes_signalled, 0, "{policy}");
        engine.begin_callback(16);
        engine.end_callback();
        let sent = engine.wake_stats().unwrap().wakes_signalled;
        assert!(sent >= 2, "{policy}: {sent} wakes");
    }
}

/// M2.7-1 C6 (auditor-1-04): a traced callback whose segments all run on the
/// calling thread (on 4 threads below `min_frames`, and on 1 thread, with no
/// worker at all) still records its slowest task, under every policy; the
/// wake and join components are absent counts, never zeros.
#[test]
fn a_traced_callback_without_parallel_segments_records_its_slowest_task() {
    for (threads, policy) in [1, 4]
        .into_iter()
        .flat_map(|t| RtPolicy::ALL.map(|p| (t, p)))
    {
        let mut config = config_threads(RATE, 42, threads);
        config.schedule = Schedule {
            policy,
            min_frames: 1_000_000,
            trace: true,
            ..config.schedule
        };
        let mut engine = Engine::new(&stress_plan(), &registry(), config, Vec::new()).unwrap();
        for _ in 0..3 {
            engine.begin_callback(BLOCK);
            assert_eq!(engine.run_block(BLOCK), BLOCK);
            engine.end_callback();
        }
        let at = format!("{threads} threads, {policy}");
        let stats = engine.schedule_stats();
        assert_eq!(stats.parallel_segments, 0, "{at}");
        assert_eq!(stats.workers, threads - 1, "{at}");
        assert_eq!(engine.workers().count(), threads - 1, "{at}");
        let wake = engine.wake_stats().expect("traced");
        assert_eq!(wake.callbacks, 3, "{at}");
        assert_eq!(wake.callbacks_without_join, 3, "{at}");
        assert_eq!(wake.slowest_task.count, 3, "{at}: {wake:?}");
        assert!(wake.slowest_task.max_ns > 0, "{at}: {wake:?}");
        // No wake and no join observed: counts of 0, empty distributions.
        assert_eq!((wake.wakes_signalled, wake.wakes_observed), (0, 0), "{at}");
        assert_eq!(
            wake.wake.count + wake.first_join.count + wake.last_join.count,
            0
        );
        assert_eq!(wake.per_worker.len(), threads - 1, "{at}");
    }
    // Without the trace, one thread keeps no pool and spawns nothing.
    let config = config_threads(RATE, 42, 1);
    let engine = Engine::new(&stress_plan(), &registry(), config, Vec::new()).unwrap();
    assert!(engine.wake_stats().is_none());
}
