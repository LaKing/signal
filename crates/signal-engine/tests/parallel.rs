//! M2.6-2 AC2 (ADR 0019 §2): 1 000 blocks on 8 threads, with a random delay
//! of 0–50 µs at the start of every task (the engine's test hook), give the
//! same audio, log and counters bit for bit as one thread, with the checked
//! mode's exclusivity tracker on. The plan sums several signal sources into
//! one input, modulates parameters, merges events from three sources into
//! one input, runs a feedback loop and a fault (with the undo of the frames
//! after it), and takes pushed external events in every block.

mod common;

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use common::{config_threads, conn, ext, inst, inst_with, plan, registry, scaled};
use signal_core::{EventPayload, Frame, SplitMix64};
use signal_engine::{Engine, HookPoint, Render, render_blocks};
use signal_graph::ExecutionPlan;

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
    let mut engine = Engine::new(
        &stress_plan(),
        &registry(),
        config_threads(RATE, 42, threads),
        Vec::new(),
    )
    .unwrap();
    if delays {
        engine.set_task_hook(Some(random_delay));
    }
    let mut sequence = 0;
    let rendered = render_blocks(&mut engine, BLOCKS * BLOCK as u64, BLOCK, |engine| {
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
    assert_eq!(stats.worker_panics, 0);
    assert!(DELAYS.load(Ordering::Relaxed) >= BLOCKS * stats.tasks as u64);
    let exclusivity = signal_engine::exclusivity();
    assert!(exclusivity.checked && exclusivity.claims > 0);
    assert_eq!(exclusivity.violations, 0);
}

static CALLER_PANICKED: AtomicBool = AtomicBool::new(false);

/// Panics once, on the calling thread, at the start of a task.
fn caller_panics(point: HookPoint) {
    if let HookPoint::Task { lane: 0, .. } = point
        && !CALLER_PANICKED.swap(true, Ordering::SeqCst)
    {
        panic!("test: the calling thread panics inside a block");
    }
}

/// AC11: when the calling thread unwinds out of a block, the job it published
/// is closed and every worker has left it before the unwinding goes on; the
/// engine then drops (and joins its workers) without hanging.
#[test]
fn a_panicking_caller_leaves_no_worker_in_the_job() {
    let mut engine = Engine::new(
        &stress_plan(),
        &registry(),
        config_threads(RATE, 42, 4),
        Vec::new(),
    )
    .unwrap();
    engine.set_task_hook(Some(caller_panics));
    let mut unwound = false;
    for _ in 0..100 {
        let block = panic::catch_unwind(AssertUnwindSafe(|| engine.run_block(BLOCK)));
        if block.is_err() {
            unwound = true;
            break;
        }
    }
    assert!(unwound, "the caller took a task and panicked");
    drop(engine);
}

static WORKER_PANICKED: AtomicBool = AtomicBool::new(false);

/// Panics once, on a worker, at the start of a task (after its claim).
fn worker_panics(point: HookPoint) {
    if let HookPoint::Task { lane, .. } = point
        && lane > 0
        && !WORKER_PANICKED.swap(true, Ordering::SeqCst)
    {
        panic!("test: a worker panics inside a block");
    }
}

/// A task hook that panics on a worker after the claim still completes the
/// task: the block returns, the panic is counted, and later blocks run on
/// the remaining workers.
#[test]
fn a_task_hook_panic_on_a_worker_completes_the_task() {
    let mut engine = Engine::new(
        &stress_plan(),
        &registry(),
        config_threads(RATE, 42, 4),
        Vec::new(),
    )
    .unwrap();
    engine.set_task_hook(Some(worker_panics));
    for _ in 0..200 {
        assert_eq!(engine.run_block(BLOCK), BLOCK);
    }
    let stats = engine.schedule_stats();
    assert!(WORKER_PANICKED.load(Ordering::SeqCst), "{stats:?}");
    assert_eq!(stats.worker_panics, 1);
    assert_eq!(stats.parallel_segments, 200);
}
