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
//!
//! A driver runs code on every worker thread through [`Workers`] (ADR 0019
//! §4: real-time policy, the audio workgroup): a posted request runs once on
//! each worker, between jobs, in posting order. Requests are kept until every
//! live worker has run them, so a [`Workers::wait`] that times out loses none:
//! a late worker runs them when it next loops.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
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

/// A request [`Workers::post`] runs once on each worker thread.
type WorkerFn = Arc<dyn Fn(usize) + Send + Sync>;

/// A posted request and its generation (1, 2, 3, … in posting order).
struct Request {
    generation: u64,
    f: WorkerFn,
}

/// What the workers share with the pool.
struct Shared {
    gate: Arc<Gate>,
    parked: Box<[AtomicBool]>,
    threads: OnceLock<Box<[Thread]>>,
    shutdown: AtomicBool,
    stats: PoolStats,
    /// Posted requests not yet run by every live worker. Locked only by the
    /// control thread and by workers between jobs, never in a job.
    requests: Mutex<VecDeque<Request>>,
    /// The latest posted generation.
    posted: AtomicU64,
    /// Per worker: the last generation it ran.
    done: Box<[AtomicU64]>,
    /// Per worker: false once its thread has ended (or never started).
    alive: Box<[AtomicBool]>,
}

impl Shared {
    /// Runs every request newer than worker `w`'s last, in order. The
    /// request is cloned under the lock and run outside it.
    fn serve(&self, w: usize) {
        let Some(done) = self.done.get(w) else {
            return;
        };
        loop {
            let mine = done.load(Ordering::Relaxed);
            let posted = self.posted.load(Ordering::SeqCst);
            if posted <= mine {
                return;
            }
            let next = self
                .requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .find(|r| r.generation > mine)
                .map(|r| (r.generation, Arc::clone(&r.f)));
            match next {
                Some((generation, f)) => {
                    f(w);
                    done.store(generation, Ordering::Release);
                }
                // Pruned: every live worker had run it, this one included.
                None => done.store(posted, Ordering::Release),
            }
        }
    }

    /// Whether worker `w` has requests to run.
    fn pending(&self, w: usize) -> bool {
        self.done
            .get(w)
            .is_some_and(|d| self.posted.load(Ordering::SeqCst) > d.load(Ordering::Relaxed))
    }

    /// How far the live workers have run `generation`.
    fn progress(&self, generation: u64) -> Progress {
        let mut progress = Progress::default();
        for (alive, done) in self.alive.iter().zip(self.done.iter()) {
            if alive.load(Ordering::Acquire) {
                progress.live += 1;
                if done.load(Ordering::Acquire) >= generation {
                    progress.done += 1;
                }
            }
        }
        progress
    }

    /// Drops the requests every live worker has run.
    fn prune(&self) {
        let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        while let Some(first) = requests.front() {
            if !self.progress(first.generation).complete() {
                break;
            }
            requests.pop_front();
        }
    }

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
            requests: Mutex::new(VecDeque::new()),
            posted: AtomicU64::new(0),
            done: (0..workers).map(|_| AtomicU64::new(0)).collect(),
            alive: (0..workers).map(|_| AtomicBool::new(true)).collect(),
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
        for alive in shared.alive.iter().skip(handles.len()) {
            alive.store(false, Ordering::Release);
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

    /// The handle that runs code on these workers.
    pub(crate) fn handle(&self, threads: usize) -> Workers {
        Workers {
            threads,
            shared: Some(Arc::clone(&self.shared)),
        }
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

/// How far the live workers have run a posted request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Live workers that have run it.
    pub done: usize,
    /// Workers whose thread is running.
    pub live: usize,
}

impl Progress {
    /// Whether every live worker has run it.
    pub fn complete(&self) -> bool {
        self.done >= self.live
    }
}

/// An engine's worker threads as a driver sees them (ADR 0019 §2, §4):
/// [`post`](Workers::post) runs code once on each, between blocks.
/// Control-thread code: it allocates, locks and sleeps. It stays usable while
/// the [`Runner`](crate::Runner) is in a driver, and after the engine is
/// dropped (nothing runs then).
#[derive(Clone)]
pub struct Workers {
    threads: usize,
    shared: Option<Arc<Shared>>,
}

impl fmt::Debug for Workers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Workers")
            .field("threads", &self.threads)
            .field("workers", &self.count())
            .finish()
    }
}

impl Workers {
    /// The handle of an engine without worker threads.
    pub(crate) fn none(threads: usize) -> Workers {
        Workers {
            threads,
            shared: None,
        }
    }

    /// The threads a block runs on, the calling thread included.
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// The worker threads started.
    pub fn count(&self) -> usize {
        self.shared
            .as_ref()
            .and_then(|s| s.threads.get())
            .map_or(0, |t| t.len())
    }

    /// Runs `f(w)` once on each worker thread `w`, between jobs and after
    /// every earlier request, and returns the request's generation; every
    /// worker is woken.
    ///
    /// The request is kept until every live worker has run it, whatever
    /// [`wait`](Workers::wait) returns. A worker whose `f` panics ends, as a
    /// worker whose task panics does: it no longer counts, and the pool runs
    /// one worker short until the engine is rebuilt.
    pub fn post(&self, f: impl Fn(usize) + Send + Sync + 'static) -> u64 {
        let Some(shared) = &self.shared else {
            return 0;
        };
        shared.prune();
        let generation = {
            let mut requests = shared
                .requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let generation = shared.posted.load(Ordering::SeqCst) + 1;
            requests.push_back(Request {
                generation,
                f: Arc::new(f),
            });
            // SeqCst: a worker stores its `parked` flag and then reads
            // `posted`; it either sees this request or is unparked below.
            shared.posted.store(generation, Ordering::SeqCst);
            generation
        };
        if let Some(threads) = shared.threads.get() {
            for thread in threads.iter() {
                thread.unpark();
            }
        }
        generation
    }

    /// Waits up to `timeout` for every live worker to run `generation`, and
    /// says how far they got. A timeout drops nothing.
    pub fn wait(&self, generation: u64, timeout: Duration) -> Progress {
        let Some(shared) = &self.shared else {
            return Progress::default();
        };
        let start = Instant::now();
        loop {
            let progress = shared.progress(generation);
            if progress.complete() || start.elapsed() >= timeout {
                shared.prune();
                return progress;
            }
            thread::sleep(Duration::from_micros(200));
        }
    }
}

/// Clears worker `w`'s `alive` flag when its thread ends, unwinding included.
struct Alive<'a>(&'a Shared, usize);

impl Drop for Alive<'_> {
    fn drop(&mut self) {
        if let Some(alive) = self.0.alive.get(self.1) {
            alive.store(false, Ordering::Release);
        }
    }
}

/// Worker `w`: runs posted requests, joins every new job as lane `w + 1`,
/// spinning briefly between jobs and parking after [`SPIN`].
fn run(shared: &Shared, w: usize) {
    let _alive = Alive(shared, w);
    let mut seen = 0;
    let mut idle = Instant::now();
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        shared.serve(w);
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
        if shared.gate.joinable(seen) || shared.shutdown.load(Ordering::SeqCst) || shared.pending(w)
        {
            parked.store(false, Ordering::SeqCst);
        } else {
            thread::park();
            parked.store(false, Ordering::SeqCst);
        }
        idle = Instant::now();
    }
}
