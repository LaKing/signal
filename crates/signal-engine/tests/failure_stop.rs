//! M2.7-3 (`docs/semantics.md` §14): a failure of the engine itself stops it
//! with an error and never continues silently. Something that unwinds in a
//! task outside a primitive's call (here the test hook) fails the task on
//! the task board: no successor runs or reads its rows, the block publishes
//! nothing, the render ends with `EngineFailed` and the real-time runner
//! plays silence and says why; the same on a worker and on the calling
//! thread, and no worker is lost. A broken access or board invariant (armed
//! by the test-only `Engine::inject`) stops it the same way, and never hangs.

mod common;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use common::{addr, config_threads, conn, inst, inst_with, plan, registry};
use signal_core::Frame;
use signal_engine::{
    Engine, EngineConfig, EngineFailure, FailureReason, HookPoint, Injection, RealtimeConfig,
    RenderError, Schedule, realtime, render,
};
use signal_graph::ExecutionPlan;

const RATE: u32 = 1_000;

/// The hook's mode and state are shared: one test at a time.
static SERIAL: Mutex<()> = Mutex::new(());
static MODE: AtomicU8 = AtomicU8::new(0);
static FIRED: AtomicBool = AtomicBool::new(false);
/// `Task` hook calls after the failure.
static AFTER: AtomicU64 = AtomicU64::new(0);

const TASK_ON_WORKER: u8 = 1;
const TASK_ON_CALLER: u8 = 2;
const ENTER_ON_WORKER: u8 = 3;
const ENTER_ON_CALLER: u8 = 4;
const LEAVE_ON_WORKER: u8 = 5;
const LEAVE_ON_CALLER: u8 = 6;
/// As `TASK_ON_WORKER`, with a panic payload whose destructor panics.
const TASK_ON_WORKER_DROP: u8 = 7;
/// The calling thread's second job (the second block) fails.
const SECOND_ENTER_ON_CALLER: u8 = 8;

/// Jobs the calling thread entered since `arm`.
static ENTERS: AtomicU64 = AtomicU64::new(0);

/// A panic payload whose destructor panics again (audit probe, M2.7-3).
struct DropPanics;

impl Drop for DropPanics {
    fn drop(&mut self) {
        panic!("test: the caught payload panics as it is dropped");
    }
}

/// A panic payload whose destructor panics with another one, `0` times more.
struct Cascade(u8);

impl Drop for Cascade {
    fn drop(&mut self) {
        if self.0 > 0 {
            std::panic::panic_any(Cascade(self.0 - 1));
        }
    }
}

/// A module that panics in `process` with a [`Cascade`] payload.
#[derive(Default)]
struct CascadingPanic;

impl signal_core::DirectPrimitive for CascadingPanic {
    fn describe() -> signal_core::ModuleDescriptor {
        signal_core::ModuleDescriptor {
            type_name: "test.cascading_panic",
            ..common::CONST
        }
    }
    fn prepare(&mut self, _: &signal_core::PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, _: &signal_core::Inputs<'_>, _: &mut signal_core::Outputs<'_>) {
        std::panic::panic_any(Cascade(2));
    }
}

fn fire() {
    if !FIRED.swap(true, Ordering::SeqCst) {
        panic!("test: the task hook panics");
    }
}

/// Waits (bounded) until the hook has fired on the other side.
fn wait_for_fire() {
    let start = Instant::now();
    while !FIRED.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(2) {
        std::thread::yield_now();
    }
}

fn hook(point: HookPoint) {
    let mode = MODE.load(Ordering::SeqCst);
    match point {
        HookPoint::Enter { lane } => match mode {
            // Every worker case: the calling thread waits (bounded) until a
            // worker has fired, so a worker takes part in the job whatever
            // the build's speed.
            TASK_ON_WORKER | TASK_ON_WORKER_DROP | ENTER_ON_WORKER | LEAVE_ON_WORKER
                if lane == 0 =>
            {
                wait_for_fire()
            }
            SECOND_ENTER_ON_CALLER if lane == 0 => {
                if ENTERS.fetch_add(1, Ordering::SeqCst) == 1 {
                    fire();
                }
            }
            TASK_ON_CALLER if lane > 0 => wait_for_fire(),
            ENTER_ON_WORKER if lane > 0 => fire(),
            ENTER_ON_CALLER if lane == 0 => fire(),
            _ => {}
        },
        HookPoint::Task { lane, .. } => {
            if FIRED.load(Ordering::SeqCst) {
                AFTER.fetch_add(1, Ordering::SeqCst);
            }
            match mode {
                TASK_ON_WORKER if lane > 0 => fire(),
                TASK_ON_WORKER_DROP if lane > 0 && !FIRED.swap(true, Ordering::SeqCst) => {
                    std::panic::panic_any(DropPanics)
                }
                TASK_ON_CALLER if lane == 0 => fire(),
                _ => {}
            }
        }
        HookPoint::Leave { lane } => match mode {
            LEAVE_ON_WORKER if lane > 0 => fire(),
            LEAVE_ON_CALLER if lane == 0 => fire(),
            _ => {}
        },
        HookPoint::Done { .. } => {}
    }
}

fn arm(mode: u8) {
    MODE.store(mode, Ordering::SeqCst);
    FIRED.store(false, Ordering::SeqCst);
    AFTER.store(0, Ordering::SeqCst);
    ENTERS.store(0, Ordering::SeqCst);
}

/// Runs `f` on its own thread and fails the test if it takes longer than
/// 10 s: a stopped engine must never hang.
fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(f());
    });
    result
        .recv_timeout(Duration::from_secs(10))
        .expect("the engine hung")
}

/// `a` → `g` → the sink: `a` is the only task without predecessors, so a
/// failure of its task leaves no other task runnable. With `wide`, eight
/// more sources beside it, so that workers join the jobs.
fn chain(wide: bool) -> ExecutionPlan {
    let mut instances = vec![
        inst_with("a", "test.const", &[("value", 0.25)]),
        inst("g", "test.gain"),
        inst("rec", "test.recorder"),
    ];
    let mut connections = vec![conn("a.out", "g.in"), conn("g.out", "rec.zeta")];
    for n in 0..if wide { 8 } else { 0 } {
        let name = format!("o{n}");
        instances.push(inst_with(&name, "test.osc", &[("freq", 10.0 + n as f64)]));
        connections.push(conn(&format!("{name}.out"), "rec.alpha"));
    }
    plan(instances, connections)
}

/// What a render of `plan` with the hook in `mode` (or `injection`) ended
/// with, and the engine's counts.
struct Stopped {
    error: Option<EngineFailure>,
    failure: Option<EngineFailure>,
    task_failures: u64,
    workers: usize,
    next_block: usize,
    after: u64,
}

fn stop(threads: usize, mode: u8, injection: Option<Injection>, wide: bool) -> Stopped {
    arm(mode);
    within(move || {
        let config = EngineConfig {
            block_capacity: 16,
            ..config_threads(RATE, 42, threads)
        };
        let mut engine = Engine::new(&chain(wide), &registry(), config, Vec::new()).unwrap();
        engine.set_task_hook(Some(hook));
        if let Some(injection) = injection {
            engine.inject(injection);
        }
        let error = match render(&mut engine, 4_096) {
            Err(RenderError::EngineFailed(failure)) => Some(failure),
            Err(other) => panic!("another error: {other}"),
            Ok(_) => None,
        };
        Stopped {
            error,
            failure: engine.failure(),
            task_failures: engine.schedule_stats().task_failures,
            workers: engine.workers().count(),
            next_block: engine.run_block(16),
            after: AFTER.load(Ordering::SeqCst),
        }
    })
}

fn assert_stopped(stopped: &Stopped, reason: FailureReason, at: &str) {
    let failure = stopped
        .error
        .unwrap_or_else(|| panic!("{at}: the render went on"));
    assert_eq!(failure.reason, reason, "{at}");
    assert_eq!(stopped.failure, Some(failure), "{at}: Engine::failure");
    assert_eq!(stopped.next_block, 0, "{at}: it runs no further frame");
}

/// AC-TASK, AC-THREADS, AC-POLICY: a task that fails on a worker and the same
/// on the calling thread stop the engine at the same frame with the same
/// error; no successor of the failed task ran; the failure is counted and
/// no worker is lost.
#[test]
fn a_failed_task_stops_the_engine_on_a_worker_and_on_the_caller() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let on_worker = stop(2, TASK_ON_WORKER, None, false);
    let on_caller = stop(2, TASK_ON_CALLER, None, false);
    for (stopped, at) in [(&on_worker, "worker"), (&on_caller, "caller")] {
        assert_stopped(stopped, FailureReason::TaskPanic, at);
        assert_eq!(stopped.error.map(|f| f.frame), Some(Frame(0)), "{at}");
        assert_eq!(stopped.after, 0, "{at}: a successor ran");
        assert_eq!(stopped.task_failures, 1, "{at}");
        assert_eq!(stopped.workers, 1, "{at}: a worker was lost");
    }
    assert_eq!(on_worker.error, on_caller.error);
    for threads in [4, 8] {
        let stopped = stop(threads, TASK_ON_CALLER, None, false);
        assert_stopped(&stopped, FailureReason::TaskPanic, "8 threads");
        assert_eq!(stopped.after, 0, "{threads} threads: a successor ran");
        assert_eq!(stopped.error, on_caller.error, "{threads} threads");
    }
}

/// A panic as a thread enters or leaves a job (no task in hand) stops the
/// engine too, on a worker and on the calling thread. Which block a worker
/// first joins depends on timing, so only the stop is asserted.
#[test]
fn a_failure_entering_or_leaving_a_job_stops_the_engine() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (mode, at) in [
        (ENTER_ON_WORKER, "enter, worker"),
        (ENTER_ON_CALLER, "enter, caller"),
        (LEAVE_ON_WORKER, "leave, worker"),
        (LEAVE_ON_CALLER, "leave, caller"),
    ] {
        let stopped = stop(4, mode, None, true);
        assert_stopped(&stopped, FailureReason::TaskPanic, at);
        assert_eq!(stopped.task_failures, 1, "{at}");
        assert_eq!(stopped.workers, 3, "{at}");
    }
}

/// The real-time runner of a stopped engine plays silence and its status
/// says why.
#[test]
fn a_stopped_runner_plays_silence_and_says_why() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    arm(ENTER_ON_CALLER);
    let status = within(|| {
        let mut config = RealtimeConfig::new(RATE, 42);
        config.engine.threads = 2;
        config.engine.schedule = Schedule::FORCED;
        let (mut runner, _controller) = realtime(&chain(true), &registry(), config).unwrap();
        runner.set_task_hook(Some(hook));
        let mut out = vec![1.0_f32; 2 * 64];
        for _ in 0..4 {
            runner.process(&mut out, 2);
            assert!(out.iter().all(|&x| x == 0.0), "silence");
        }
        runner.status()
    });
    let failure = status.failure.expect("the status says it stopped");
    assert_eq!(failure.reason, FailureReason::TaskPanic);
    assert_eq!(failure.frame, Frame(0));
}

/// AC-POLICY: each broken invariant stops the engine with
/// `InvariantViolation` at the same frame on every thread count, never
/// hanging: a refused view (on the calling thread alone, parts not found),
/// a refused read of another task's rows, a ready task with no slot, and a
/// claim that finds its slot empty.
#[test]
fn a_broken_invariant_stops_the_engine() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let cases: [(Injection, &[usize]); 4] = [
        (Injection::RefuseView(addr("/g")), &[1, 2, 8]),
        (Injection::RefuseRead(addr("/a")), &[2, 8]),
        (Injection::DropReady, &[2, 8]),
        (Injection::MissingSlot, &[2, 8]),
    ];
    for (injection, threads) in cases {
        for &threads in threads {
            let at = format!("{injection:?}, {threads} threads");
            let stopped = stop(threads, 0, Some(injection.clone()), true);
            assert_stopped(&stopped, FailureReason::InvariantViolation, &at);
            assert_eq!(stopped.error.map(|f| f.frame), Some(Frame(0)), "{at}");
            assert_eq!(stopped.task_failures, 0, "{at}");
        }
    }
}

/// auditor-1-payload-unwind: a caught panic whose payload panics again as it
/// is dropped still fails the task, settles its claim and stops the engine;
/// the worker is not lost and the calling thread does not hang.
#[test]
fn a_panic_payload_that_panics_when_dropped_still_stops_the_engine() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let stopped = stop(2, TASK_ON_WORKER_DROP, None, false);
    assert_stopped(&stopped, FailureReason::TaskPanic, "payload");
    assert_eq!(stopped.error.map(|f| f.frame), Some(Frame(0)));
    assert_eq!(stopped.task_failures, 1);
    assert_eq!(stopped.workers, 1, "a worker was lost");
}

/// auditor-1-payload-unwind: a module whose panic payloads cascade as they
/// are dropped is still only faulted, on the calling thread alone and on
/// the workers: nothing unwinds out of the render and the engine runs on.
#[test]
fn a_module_whose_panic_payload_panics_is_only_faulted() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    arm(0);
    for threads in [1, 2] {
        let (unwound, failure, muted, faults) = within(move || {
            let mut registry = registry();
            registry
                .register(signal_core::ModuleType {
                    descriptor: <CascadingPanic as signal_core::DirectPrimitive>::describe(),
                    create: || signal_core::Primitive::Direct(Box::new(CascadingPanic)),
                })
                .unwrap();
            let definition = signal_graph::CompositeDef {
                name: "project".to_owned(),
                instances: vec![
                    inst("a", "test.cascading_panic"),
                    inst_with("b", "test.const", &[("value", 0.5)]),
                    inst("rec", "test.recorder"),
                ],
                connections: vec![conn("a.out", "rec.zeta"), conn("b.out", "rec.alpha")],
                exports: signal_graph::Exports::default(),
            };
            let plan = signal_graph::build_plan(&definition, &[], &registry, 1).unwrap();
            let config = EngineConfig {
                block_capacity: 16,
                ..config_threads(RATE, 42, threads)
            };
            let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
            let rendered =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(&mut engine, 64)));
            let faults = match &rendered {
                Ok(Ok(rendered)) => String::from_utf8_lossy(&rendered.log)
                    .matches(r#""reason":"panic""#)
                    .count(),
                _ => 0,
            };
            (
                rendered.is_err(),
                engine.failure(),
                engine.is_muted(&addr("/a")),
                faults,
            )
        });
        assert!(!unwound, "{threads} threads: the render unwound");
        assert_eq!(failure, None, "{threads} threads");
        assert_eq!(muted, Some(true), "{threads} threads");
        assert_eq!(faults, 1, "{threads} threads");
    }
}

/// auditor-1-callback-silence-policy: when a later block of a callback
/// fails, the frames of its earlier blocks stand and the callback is silent
/// from the failed block's first frame on.
#[test]
fn a_callback_keeps_the_blocks_before_the_failed_one() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    arm(SECOND_ENTER_ON_CALLER);
    let (out, status) = within(|| {
        let mut config = RealtimeConfig::with_buffer(RATE, 42, 16);
        config.engine.threads = 2;
        config.engine.schedule = Schedule::FORCED;
        let plan = plan(
            vec![
                inst_with("a", "test.const", &[("value", 0.5)]),
                inst_with("b", "test.const", &[("value", 0.25)]),
                inst("rec", "test.recorder"),
            ],
            vec![conn("a.out", "rec.zeta"), conn("b.out", "rec.alpha")],
        );
        let (mut runner, _controller) = realtime(&plan, &registry(), config).unwrap();
        runner.set_task_hook(Some(hook));
        let mut out = vec![1.0_f32; 2 * 64];
        runner.process(&mut out, 2);
        (out, runner.status())
    });
    let failure = status.failure.expect("the status says it stopped");
    assert_eq!(failure.frame, Frame(16));
    let (kept, silent) = out.split_at(2 * 16);
    assert!(kept.chunks(2).all(|f| f == [0.5, 0.25]), "{kept:?}");
    assert!(silent.iter().all(|&x| x == 0.0), "{silent:?}");
}
