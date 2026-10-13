//! Model tests of the scheduler's synchronisation (M2.7-6): loom runs the
//! real gate (`gate.rs`, `shared::Gate`), task board (`board.rs`, through
//! `shared::Job`) and worker loop (`park.rs`) in every interleaving of the
//! caller and two or three workers, within a preemption bound of 2 (unless
//! `LOOM_MAX_PREEMPTIONS` says otherwise), with loom's memory model for
//! their atomics and its race detection on the gate's job slot.
//!
//! Only built with `--cfg loom` (`docs/testing-and-evaluation.md`,
//! "Synchronisation model tests"):
//!
//! ```text
//! RUSTFLAGS="--cfg loom" cargo test -p signal-engine --lib --release model::
//! ```
//!
//! Threads that wait for each other here park (`loom::thread::park`), so
//! that a lost wake ends every thread parked, which loom reports as a
//! deadlock.

mod board;
mod gate;
mod park;

use std::sync::Arc;

use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use loom::thread::{self, Thread};

use crate::shared::{Access, PlanCells};

/// Runs `scenario` in every interleaving loom explores with at most
/// `preemptions` preemptions (unless `LOOM_MAX_PREEMPTIONS` says otherwise),
/// and reports how long that took.
///
/// The gate and park scenarios use 2. The board's scenarios use 1: at 2,
/// loom's conservative backtracking for bounded preemption explores
/// schedules in which two workers waiting for a successor (`spin_loop`, a
/// yield in loom) hand the processor to each other while the thread holding
/// its predecessor never runs, and one execution exceeds 100 000 branches
/// (`docs/testing-and-evaluation.md`, "Synchronisation model tests").
fn explore(name: &str, preemptions: usize, scenario: impl Fn() + Sync + Send + 'static) {
    let mut builder = loom::model::Builder::new();
    if builder.preemption_bound.is_none() {
        builder.preemption_bound = Some(preemptions);
    }
    // The board's and the gate's waits spin (`spin_loop`, a yield in loom):
    // an execution may take many more branches than loom's default 1 000.
    builder.max_branches = builder.max_branches.max(100_000);
    let start = std::time::Instant::now();
    builder.check(scenario);
    eprintln!("model {name}: explored in {:.2?}", start.elapsed());
}

/// Sets `flag` and unparks `waiter`, which waits for it with [`wait`].
fn set(flag: &AtomicBool, waiter: &Thread) {
    flag.store(true, Ordering::Release);
    waiter.unpark();
}

/// Parks until `flag` is set.
fn wait(flag: &AtomicBool) {
    while !flag.load(Ordering::Acquire) {
        thread::park();
    }
}

/// `n` flags, all clear.
fn flags(n: usize) -> Arc<[AtomicBool]> {
    (0..n).map(|_| AtomicBool::new(false)).collect()
}

/// `n` counters at 0.
fn counters(n: usize) -> Arc<[AtomicU64]> {
    (0..n).map(|_| AtomicU64::new(0)).collect()
}

/// A plan of independent tasks (`preds`, `successors`) on `lanes` lanes.
fn plan(preds: &[u32], successors: &[&[u32]], lanes: usize) -> PlanCells {
    PlanCells::for_tasks(
        preds.to_vec(),
        successors.iter().map(|s| s.to_vec()).collect(),
        lanes,
    )
}

/// A job body that counts each task's runs in `runs`.
fn counting(runs: &Arc<[AtomicU64]>) -> impl Fn(&mut Access<'_>, usize) + Sync + use<> {
    let runs = Arc::clone(runs);
    move |_: &mut Access<'_>, t: usize| {
        if let Some(r) = runs.get(t) {
            r.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Every counter is `n`.
fn all_eq(runs: &[AtomicU64], n: u64) -> bool {
    runs.iter().all(|r| r.load(Ordering::Relaxed) == n)
}
