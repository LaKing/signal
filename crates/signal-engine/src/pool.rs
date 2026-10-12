//! The worker threads of an engine (ADR 0019 §2, `docs/realtime-rules.md`
//! "Worker threads"). Created with the engine on the control thread and kept
//! across plan swaps; dropped (shut down and joined) with it.
//!
//! A worker spins on the gate for [`SPIN`] after its last job, then parks. A
//! parallel segment wakes at most one parked worker from the calling thread;
//! each worker that joins wakes up to two more. The handshake is complete:
//! a parking worker stores its `parked` flag (SeqCst) and then reads the gate
//! (SeqCst); the caller opens the gate (SeqCst) and then reads the flags
//! (SeqCst). In the single order of those operations, either the worker sees
//! the open gate and does not park, or the caller sees the flag and wakes it.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use crate::shared::{Gate, Job, Opener, PoolStats};

/// How long an idle worker spins before it parks.
pub(crate) const SPIN: Duration = Duration::from_micros(50);

/// Worker threads spawned by every pool of this process.
static SPAWNED: AtomicUsize = AtomicUsize::new(0);

/// The number of worker threads every engine of this process has spawned.
pub fn spawned_workers() -> usize {
    SPAWNED.load(Ordering::Relaxed)
}

/// What the workers share with the pool.
struct Shared {
    gate: Arc<Gate>,
    parked: Box<[AtomicBool]>,
    threads: OnceLock<Box<[Thread]>>,
    shutdown: AtomicBool,
    stats: PoolStats,
}

impl Shared {
    /// Wakes up to `n` parked workers. SeqCst: the caller's half of the park
    /// handshake, after the SeqCst open.
    fn wake(&self, n: usize) {
        let Some(threads) = self.threads.get() else {
            return;
        };
        let mut woken = 0;
        for (parked, thread) in self.parked.iter().zip(threads.iter()) {
            if woken >= n {
                break;
            }
            if parked.swap(false, Ordering::SeqCst) {
                thread.unpark();
                woken += 1;
            }
        }
    }
}

/// An engine's worker threads.
pub(crate) struct Pool {
    shared: Arc<Shared>,
    opener: Opener,
    handles: Vec<JoinHandle<()>>,
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("workers", &self.handles.len())
            .finish()
    }
}

impl Pool {
    /// Starts `workers` threads (control-thread code; it allocates). A thread
    /// the system refuses is left out.
    pub(crate) fn new(workers: usize) -> Pool {
        let (gate, opener) = Gate::new();
        let shared = Arc::new(Shared {
            gate,
            parked: (0..workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
            shutdown: AtomicBool::new(false),
            stats: PoolStats::default(),
        });
        let mut handles = Vec::with_capacity(workers);
        for w in 0..workers {
            let worker = shared.clone();
            let spawned = thread::Builder::new()
                .name(format!("signal-worker-{w}"))
                .spawn(move || run(&worker, w));
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(_) => break,
            }
        }
        SPAWNED.fetch_add(handles.len(), Ordering::Relaxed);
        let threads = handles.iter().map(|h| h.thread().clone()).collect();
        let _ = shared.threads.set(threads);
        Pool {
            shared,
            opener,
            handles,
        }
    }

    /// The worker threads running.
    pub(crate) fn workers(&self) -> usize {
        self.handles.len()
    }

    pub(crate) fn stats(&self) -> &PoolStats {
        &self.shared.stats
    }

    /// What runs a job, and the counters a job updates.
    pub(crate) fn split(&mut self) -> (Dispatch<'_>, &PoolStats) {
        let dispatch = Dispatch {
            opener: &mut self.opener,
            shared: &self.shared,
        };
        (dispatch, &self.shared.stats)
    }
}

/// Runs jobs on a pool.
pub(crate) struct Dispatch<'p> {
    opener: &'p mut Opener,
    shared: &'p Shared,
}

impl Dispatch<'_> {
    /// Runs `job` on the calling thread (lane 0) and the workers, and returns
    /// once every task is complete and every worker has left it.
    pub(crate) fn run(&mut self, job: &Job<'_>) {
        let shared = self.shared;
        self.opener
            .serve(job, || shared.wake(1), |job| job.participate(0));
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        for handle in &self.handles {
            handle.thread().unpark();
        }
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

/// Worker `w`: joins every new job as lane `w + 1`, spinning briefly between
/// jobs and parking after [`SPIN`].
fn run(shared: &Shared, w: usize) {
    let mut seen = 0;
    let mut idle = Instant::now();
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        let joined = shared.gate.join(&mut seen, |job| {
            shared.wake(2);
            job.participate(w + 1);
        });
        if joined {
            idle = Instant::now();
            continue;
        }
        if idle.elapsed() < SPIN {
            for _ in 0..64 {
                std::hint::spin_loop();
            }
            continue;
        }
        let Some(parked) = shared.parked.get(w) else {
            return;
        };
        parked.store(true, Ordering::SeqCst);
        if shared.gate.joinable(seen) || shared.shutdown.load(Ordering::SeqCst) {
            parked.store(false, Ordering::SeqCst);
        } else {
            thread::park();
            parked.store(false, Ordering::SeqCst);
        }
        idle = Instant::now();
    }
}
