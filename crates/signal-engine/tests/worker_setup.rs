//! M2.6-3 (ADR 0019 §4): the driver's hook on the worker pool. A posted
//! request runs once on each worker thread, on that thread, between blocks
//! and in posting order; a wait that times out drops nothing; a worker whose
//! request panics stops counting; and requests change no output. The real-time
//! count of `threads = 0` is two thirds of the performance cores.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use common::{config_threads, conn, inst, plan, registry, scaled};
use signal_engine::{Engine, RealtimeConfig, Workers, performance_cores, realtime_threads};
use signal_graph::ExecutionPlan;
use signal_output::{NullDriver, realtime, render};

const RATE: u32 = 1_000;
const LONG: Duration = Duration::from_secs(10);

/// Several sources summed into the sink: tasks for the workers.
fn sources() -> ExecutionPlan {
    plan(
        vec![
            inst("o1", "test.osc"),
            inst("o2", "test.osc"),
            inst("n", "test.noise"),
            inst("sum", "test.gain"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("o1.out", "sum.in"),
            scaled("o2.out", "sum.in", 0.5),
            scaled("n.out", "sum.in", 0.25),
            conn("sum.out", "rec.zeta"),
        ],
    )
}

fn workers(threads: usize) -> (Engine, Workers) {
    let engine = Engine::new(
        &sources(),
        &registry(),
        config_threads(RATE, 42, threads),
        Vec::new(),
    )
    .unwrap();
    let workers = engine.workers();
    (engine, workers)
}

#[test]
fn a_request_runs_once_on_each_worker_thread_in_posting_order() {
    let (_engine, workers) = workers(4);
    assert_eq!((workers.threads(), workers.count()), (4, 3));
    // Let the workers park first.
    thread::sleep(Duration::from_millis(20));
    let seen = Arc::new(Mutex::new(Vec::new()));
    for request in 0..2 {
        let seen = Arc::clone(&seen);
        let generation = workers.post(move |w| {
            let name = thread::current().name().map(str::to_owned);
            seen.lock().unwrap().push((request, w, name));
        });
        let progress = workers.wait(generation, LONG);
        assert!(progress.complete(), "{progress:?}");
        assert_eq!((progress.done, progress.live), (3, 3));
    }
    let mut seen = seen.lock().unwrap().clone();
    // Per worker: request 0, then request 1, on its own thread.
    seen.sort_by_key(|&(request, w, _)| (w, request));
    let expected: Vec<_> = (0..3)
        .flat_map(|w| (0..2).map(move |r| (r, w, Some(format!("signal-worker-{w}")))))
        .collect();
    assert_eq!(seen, expected);
}

#[test]
fn a_wait_that_times_out_drops_no_request() {
    let (_engine, workers) = workers(3);
    let release = Arc::new(AtomicBool::new(false));
    let held = Arc::clone(&release);
    // Worker 0 is held inside the first request.
    let first = workers.post(move |w| {
        while w == 0 && !held.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
    });
    let teardowns = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&teardowns);
    let second = workers.post(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    let progress = workers.wait(second, Duration::from_millis(50));
    assert_eq!((progress.done, progress.live), (1, 2), "worker 0 is held");
    assert!(workers.wait(first, Duration::ZERO).done < 2);
    // Released later, it still runs the second request.
    release.store(true, Ordering::Release);
    assert!(workers.wait(second, LONG).complete());
    assert_eq!(teardowns.load(Ordering::SeqCst), 2);
}

#[test]
fn a_worker_whose_request_panics_stops_counting() {
    let (_engine, workers) = workers(3);
    let generation = workers.post(|w| {
        if w == 1 {
            panic!("test: a request panics on worker 1");
        }
    });
    let progress = workers.wait(generation, LONG);
    assert_eq!((progress.done, progress.live), (1, 1));
    let ran = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ran);
    let next = workers.post(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    assert!(workers.wait(next, LONG).complete());
    assert_eq!(ran.load(Ordering::SeqCst), 1, "only the live worker");
}

#[test]
fn one_thread_has_no_workers_to_run_on() {
    let (_engine, workers) = workers(1);
    assert_eq!((workers.threads(), workers.count()), (1, 0));
    let generation = workers.post(|_| panic!("no worker runs it"));
    assert!(workers.wait(generation, Duration::ZERO).complete());
}

#[test]
fn requests_between_blocks_change_no_output() {
    let frames = 4_096;
    let (mut reference, _) = workers(1);
    let reference = render(&mut reference, frames).unwrap();
    let (mut engine, workers) = workers(4);
    let ran = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ran);
    let generation = workers.post(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    let rendered = render(&mut engine, frames).unwrap();
    assert!(workers.wait(generation, LONG).complete());
    assert_eq!(ran.load(Ordering::SeqCst), 3);
    assert!(rendered.samples == reference.samples);
    assert!(rendered.log == reference.log);
    assert!(engine.schedule_stats().worker_tasks > 0);
    let exclusivity = signal_engine::exclusivity();
    assert_eq!(exclusivity.violations, 0);
    // Instrumentation: the tracker exists in debug builds only.
    if exclusivity.checked {
        assert!(exclusivity.claims > 0);
    }
}

#[test]
fn the_runner_hands_out_its_workers_and_auto_is_two_thirds_of_the_cores_in_real_time() {
    let mut config = RealtimeConfig::new(RATE, 42);
    config.engine = config_threads(RATE, 42, 3);
    let (runner, _controller) = realtime(&sources(), &registry(), config).unwrap();
    let workers = runner.workers();
    assert_eq!((workers.threads(), workers.count()), (3, 2));
    // The handle keeps working while the Runner is in a driver.
    let mut driver = NullDriver::new(RATE, 1, 5);
    signal_output::Driver::start(&mut driver, runner).unwrap();
    let generation = workers.post(|_| {});
    assert!(workers.wait(generation, LONG).complete());
    drop(driver);

    let cores = performance_cores().0;
    assert_eq!(realtime_threads(), (cores * 2 / 3).max(1));
    config.engine.threads = 0;
    let (runner, _controller) = realtime(&sources(), &registry(), config).unwrap();
    assert_eq!(runner.workers().threads(), realtime_threads());
    // Offline, auto is every performance core.
    let offline = Engine::new(
        &sources(),
        &registry(),
        config_threads(RATE, 42, 0),
        Vec::new(),
    )
    .unwrap();
    assert_eq!(offline.config().threads, cores);
}
