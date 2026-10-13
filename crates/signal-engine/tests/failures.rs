//! M2.7-3 (`docs/semantics.md` §14): a module that panics is a failure of
//! that instance, never silent wrong output. The audit's reproduction (two
//! summed sources, one of them failing) gives the documented result on a
//! worker and on the calling thread, at 1, 2, 4 and 8 threads, bit for bit:
//! the faulted source counts as 0, a `panic` fault is logged and the
//! instance is muted. A panic in a feedback cycle and in a state-output
//! `emit` (and `absorb`) acts exactly as a non-finite output at that frame.
//!
//! Written against the engine's API as it was before M2.7-3 too, so that it
//! shows the defect on that revision (finding 1 of the audit of 986cc16).

mod common;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::{FAILED_ON, addr, config_threads, conn, inst, inst_with, plan, registry, scaled};
use signal_engine::{Engine, EngineConfig, HookPoint, Render, render};
use signal_graph::ExecutionPlan;

const RATE: u32 = 1_000;

/// The tests that steer where a module fails share `FAILED_ON` and the hook
/// statics: one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

/// The longest a steering hook waits for the other side.
const STEER: Duration = Duration::from_secs(2);

/// Where the failing module should run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// On a worker: the calling thread waits, at each job's start, until
    /// the module has failed.
    Worker,
    /// On the calling thread: every worker waits, at its start in a job,
    /// until the module has failed.
    Caller,
    /// Wherever the scheduler puts it (no hook).
    Any,
}

fn failed() -> bool {
    FAILED_ON.lock().is_ok_and(|on| on.is_some())
}

fn wait_for_failure() {
    let start = Instant::now();
    while !failed() && start.elapsed() < STEER {
        std::thread::yield_now();
    }
}

fn caller_waits(point: HookPoint) {
    if let HookPoint::Enter { lane: 0 } = point {
        wait_for_failure();
    }
}

fn workers_wait(point: HookPoint) {
    if let HookPoint::Enter { lane } = point
        && lane > 0
    {
        wait_for_failure();
    }
}

/// Renders `plan` on `threads` threads in blocks of `k`, with the failing
/// module steered to `place` (threads > 1), and says on which thread it
/// failed (`Some(true)`: the calling thread).
fn run(
    plan: &ExecutionPlan,
    threads: usize,
    k: usize,
    frames: u64,
    place: Place,
) -> (Render, Engine, Option<bool>) {
    *FAILED_ON.lock().unwrap() = None;
    let config = EngineConfig {
        block_capacity: k,
        ..config_threads(RATE, 42, threads)
    };
    let mut engine = Engine::new(plan, &registry(), config, Vec::new()).unwrap();
    engine.set_task_hook(match place {
        Place::Worker => Some(caller_waits),
        Place::Caller => Some(workers_wait),
        Place::Any => None,
    });
    let rendered = render(&mut engine, frames).unwrap();
    let on = *FAILED_ON.lock().unwrap();
    let caller = on.map(|on| on == std::thread::current().id());
    (rendered, engine, caller)
}

fn log(render: &Render) -> String {
    String::from_utf8(render.log.clone()).unwrap()
}

/// The `fault` lines of a log.
fn faults(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|l| l.contains(r#""type":"fault""#))
        .collect()
}

/// The audit's plan: two constants summed into the sink's `zeta` input, the
/// first (0.25) failing at its first frame, the second 0.5.
fn summed_sources() -> ExecutionPlan {
    plan(
        vec![
            inst_with("a", "test.fail", &[("value", 0.25), ("at", 0.0)]),
            inst_with("b", "test.const", &[("value", 0.5)]),
            inst("rec", "test.recorder"),
        ],
        vec![conn("a.out", "rec.zeta"), conn("b.out", "rec.zeta")],
    )
}

/// AC1, AC-THREADS: the faulted source counts as 0 (the sum is 0.5, never
/// a partial value published as if it had succeeded), its fault is logged
/// with reason `panic`, it is muted and the other source is not, the render
/// goes on; the same bit for bit at 1, 2, 4 and 8 threads with the failure
/// on a worker or on the calling thread.
#[test]
fn a_panicking_source_counts_as_zero_on_every_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let plan = summed_sources();
    let (reference, _, _) = run(&plan, 1, 256, 256, Place::Caller);
    let reference_log = log(&reference);
    for (frame, pair) in reference.samples.chunks(2).enumerate() {
        assert_eq!(pair, [0.5, 0.0], "frame {frame}: a counts as 0, b is 0.5");
    }
    assert_eq!(
        faults(&reference_log),
        [r#"{"type":"fault","frame":0,"address":"/a","port":"/a.out","reason":"panic"}"#],
    );
    for threads in [1, 2, 4, 8] {
        let places: &[Place] = if threads == 1 {
            &[Place::Caller]
        } else {
            &[Place::Worker, Place::Caller]
        };
        for &place in places {
            let at = format!("{threads} threads, {place:?}");
            let (rendered, engine, caller) = run(&plan, threads, 256, 256, place);
            assert_eq!(
                caller,
                Some(place == Place::Caller),
                "{at}: where it failed"
            );
            assert!(rendered.samples == reference.samples, "{at}: the audio");
            assert_eq!(log(&rendered), reference_log, "{at}: the log");
            assert_eq!(engine.is_muted(&addr("/a")), Some(true), "{at}");
            assert_eq!(engine.is_muted(&addr("/b")), Some(false), "{at}");
        }
    }
}

/// A module without outputs (here the audio sink) that panics still has its
/// fault logged, with no port; what it records is unaffected.
#[test]
fn a_panicking_sink_is_logged_without_a_port() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let plan = plan(
        vec![
            inst_with("b", "test.const", &[("value", 0.5)]),
            inst_with("rec", "test.fail_sink", &[("at", 3.0)]),
        ],
        vec![conn("b.out", "rec.in")],
    );
    for threads in [1, 2] {
        let (rendered, engine, _) = run(&plan, threads, 1, 8, Place::Any);
        assert!(rendered.samples.iter().all(|&x| x == 0.5), "{threads}");
        assert_eq!(
            faults(&log(&rendered)),
            [r#"{"type":"fault","frame":3,"address":"/rec","reason":"panic"}"#],
        );
        assert_eq!(engine.is_muted(&addr("/rec")), Some(true));
    }
}

/// AC2: `plan(0.0)` gives the same audio at every block size and thread
/// count, equal bit for bit to its oracle `plan(1.0)` (the failing module
/// outputs NaN at that frame instead of panicking), and a log equal to the
/// oracle's but for the reason (`panic` for `non_finite`).
fn acts_as_a_fault_at_its_frame(plan: impl Fn(f64) -> ExecutionPlan) {
    let frames = 128;
    let (reference, _, _) = run(&plan(0.0), 1, 1, frames, Place::Any);
    let (oracle, _, _) = run(&plan(1.0), 1, 1, frames, Place::Any);
    assert!(
        reference.samples == oracle.samples,
        "as a non-finite output"
    );
    assert!(
        reference.samples.iter().any(|&x| x != 0.0),
        "the plan makes sound"
    );
    let expected = log(&oracle).replace(r#""reason":"non_finite""#, r#""reason":"panic""#);
    assert_eq!(log(&reference), expected);
    assert_eq!(faults(&expected).len(), 1);
    let plan = plan(0.0);
    for k in [1, 64, 256] {
        for threads in [1, 2, 4, 8] {
            let (rendered, _, _) = run(&plan, threads, k, frames, Place::Any);
            let at = format!("k = {k}, {threads} threads");
            assert!(rendered.samples == reference.samples, "{at}: the audio");
            assert_eq!(log(&rendered), log(&reference), "{at}: the log");
        }
    }
}

/// AC2: a panic inside a feedback cycle unit (a direct member, frame 37).
#[test]
fn a_panic_inside_a_feedback_cycle() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    acts_as_a_fault_at_its_frame(|nan| {
        plan(
            vec![
                inst_with("dc", "test.dc", &[("value", 0.75)]),
                inst_with("f", "test.fail", &[("at", 37.0), ("nan", nan)]),
                inst("delay", "test.unit_delay"),
                inst("rec", "test.recorder"),
            ],
            vec![
                conn("dc.out", "f.in"),
                scaled("delay.out", "f.in", 0.5),
                conn("f.out", "delay.in"),
                conn("f.out", "rec.zeta"),
                conn("delay.out", "rec.alpha"),
            ],
        )
    });
}

/// AC2: a panic in a state-output `emit` (frame 37).
#[test]
fn a_panic_in_a_state_output_emit() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    acts_as_a_fault_at_its_frame(|nan| {
        plan(
            vec![
                inst_with("dc", "test.dc", &[("value", 0.75)]),
                inst_with("s", "test.fail_state", &[("at", 37.0), ("nan", nan)]),
                inst("rec", "test.recorder"),
            ],
            vec![conn("dc.out", "s.in"), conn("s.out", "rec.zeta")],
        )
    });
}

/// A panic in a state-output `absorb` at frame 37: frame 37's output stands,
/// the instance is muted from 38 (as a non-finite `emit` at 38 would be),
/// and the fault is logged at 37.
#[test]
fn a_panic_in_a_state_output_absorb() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let absorbing = |at: f64, phase: f64, nan: f64| {
        plan(
            vec![
                inst_with("dc", "test.dc", &[("value", 0.75)]),
                inst_with(
                    "s",
                    "test.fail_state",
                    &[("at", at), ("phase", phase), ("nan", nan)],
                ),
                inst("rec", "test.recorder"),
            ],
            vec![conn("dc.out", "s.in"), conn("s.out", "rec.zeta")],
        )
    };
    let frames = 128;
    let (panicked, engine, _) = run(&absorbing(37.0, 1.0, 0.0), 1, 1, frames, Place::Any);
    let (oracle, _, _) = run(&absorbing(38.0, 0.0, 1.0), 1, 1, frames, Place::Any);
    assert!(panicked.samples == oracle.samples);
    assert_eq!(
        faults(&log(&panicked)),
        [r#"{"type":"fault","frame":37,"address":"/s","port":"/s.out","reason":"panic"}"#],
    );
    assert_eq!(engine.is_muted(&addr("/s")), Some(true));
    for (k, threads) in [(64, 4), (256, 8)] {
        let (rendered, _, _) = run(&absorbing(37.0, 1.0, 0.0), threads, k, frames, Place::Any);
        assert!(rendered.samples == panicked.samples, "k = {k}, {threads}");
        assert_eq!(log(&rendered), log(&panicked), "k = {k}, {threads}");
    }
}
