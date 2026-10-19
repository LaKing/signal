//! The worker threads of an engine (ADR 0019 §2, `docs/realtime-rules.md`
//! "Worker threads"). Created with the engine on the control thread and kept
//! across plan swaps; dropped (shut down and joined) with it.
//!
//! A worker spins on the gate for the pool's spin (`Schedule::spin_ns`, 50 µs;
//! under `budget-spin` until 0.9 × its computation budget has passed since it
//! last resumed; at most [`MAX_SPIN`]) after its last job or wake, then
//! blocks: an engine with no parallel jobs blocks every worker within
//! [`MAX_SPIN`], so an idle engine never keeps a core busy. A parallel
//! segment wakes at most one blocked worker from the calling thread; each
//! worker that joins wakes up to two more. Under `semaphore` and `fanout` the
//! calling thread also wakes workers at the start of a callback
//! ([`Pool::begin_callback`], `docs/realtime-rules.md` "Wake policies").
//! Only the running plan's eligible workers (its worker budget, M2.10-5) are
//! woken by these paths and join its jobs; the others stay parked. The
//! worker loop, the flags and the handshake are `park.rs`'s, which this
//! pool drives ([`park::Worker`]); the handshake loses no wake (see there).
//!
//! Every wake goes through `Shared::signal` and every block through
//! `Shared::block`: `Thread::unpark` and `thread::park`, or the policy's
//! [`Waiter`] (a semaphore per worker). Requests ([`Workers::post`]) and the
//! shutdown signal every worker unconditionally; a signal the worker did not
//! need (a park token or a semaphore count) only makes its next block return
//! at once, and the loop re-checks everything after any return.
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

use crate::engine::Schedule;
use crate::park::{self, Parking};
use crate::shared::{Gate, Job, Opener, PoolStats};
use crate::wake::{Histogram, RtPolicy, Spread, Waiter, WakeStats};

/// The longest an idle worker spins before it parks, whatever the schedule
/// asks: a little over two device periods of 128 frames at 96 kHz. It bounds
/// what a worker burns after the last job of a run or after a wake that
/// brought no job.
pub(crate) const MAX_SPIN: Duration = Duration::from_millis(3);

/// Worker threads spawned by every pool of this process.
static SPAWNED: AtomicUsize = AtomicUsize::new(0);

/// The number of worker threads every engine of this process has spawned.
/// Tests only (feature `test-hooks`).
#[cfg(feature = "test-hooks")]
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

/// How workers block and are woken.
enum Blocker {
    /// `thread::park` and `Thread::unpark`.
    Park,
    /// A semaphore per worker (`semaphore`).
    Waiter(Arc<dyn Waiter>),
}

/// The wake-up trace's state (`Schedule::trace`): stamps in ns since
/// `epoch`, plus one (0 is none), and histograms. Atomics only.
struct Trace {
    epoch: Instant,
    /// Per worker: when its pending wake was sent.
    signalled_at: Box<[AtomicU64]>,
    /// Per worker: from a wake to the worker running again.
    wake: Box<[Histogram]>,
    callback_start: AtomicU64,
    /// This callback's first and last join (`u64::MAX` and 0 when none).
    first_join: AtomicU64,
    last_join: AtomicU64,
    first: Histogram,
    last: Histogram,
    slowest: Histogram,
    callbacks: AtomicU64,
    without_join: AtomicU64,
    signalled: AtomicU64,
    observed: AtomicU64,
    /// Per worker (M2.10-5): when it last became idle (after a job or a
    /// resume), plus one (0 is not idle); the time it spent in jobs, idle
    /// and not blocked (spinning), and its blocks.
    idle_since: Box<[AtomicU64]>,
    job_ns: Box<[AtomicU64]>,
    spin_ns: Box<[AtomicU64]>,
    parks: Box<[AtomicU64]>,
}

impl Trace {
    fn new(workers: usize) -> Trace {
        Trace {
            epoch: Instant::now(),
            signalled_at: (0..workers).map(|_| AtomicU64::new(0)).collect(),
            wake: (0..workers).map(|_| Histogram::new()).collect(),
            callback_start: AtomicU64::new(0),
            first_join: AtomicU64::new(u64::MAX),
            last_join: AtomicU64::new(0),
            first: Histogram::new(),
            last: Histogram::new(),
            slowest: Histogram::new(),
            callbacks: AtomicU64::new(0),
            without_join: AtomicU64::new(0),
            signalled: AtomicU64::new(0),
            observed: AtomicU64::new(0),
            idle_since: (0..workers).map(|_| AtomicU64::new(0)).collect(),
            job_ns: (0..workers).map(|_| AtomicU64::new(0)).collect(),
            spin_ns: (0..workers).map(|_| AtomicU64::new(0)).collect(),
            parks: (0..workers).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// Worker `w`'s idle time ends at `now`: added to its spin.
    fn idle_ends(&self, w: usize, now: u64) {
        if let (Some(since), Some(spin)) = (self.idle_since.get(w), self.spin_ns.get(w)) {
            let since = since.swap(0, Ordering::Relaxed);
            if since != 0 {
                spin.fetch_add(now.saturating_sub(since), Ordering::Relaxed);
            }
        }
    }

    /// Worker `w` is idle from `now`. An idle time still open (the worker
    /// decided not to block, so `block` did not end it) is counted first.
    fn idle_starts(&self, w: usize, now: u64) {
        self.idle_ends(w, now);
        if let Some(since) = self.idle_since.get(w) {
            since.store(now, Ordering::Relaxed);
        }
    }

    /// Now, in ns since the epoch, plus one.
    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos())
            .unwrap_or(u64::MAX - 1)
            .saturating_add(1)
    }
}

/// What the workers share with the pool.
struct Shared {
    gate: Arc<Gate>,
    /// How long an idle worker spins before it blocks (at most [`MAX_SPIN`]).
    spin: Duration,
    /// `budget-spin`: how long after it last resumed a worker may spin.
    budget_spin: Option<Duration>,
    policy: RtPolicy,
    blocker: Blocker,
    /// The workers the running plan may use (M2.10-5): workers `0..eligible`
    /// are woken and join its jobs, the others skip them and stay parked.
    /// Written only by the calling thread, before it opens a job or wakes at
    /// a callback's start; timing only, since the calling thread runs every
    /// task no worker takes.
    eligible: AtomicUsize,
    /// The callback generation ([`Pool::begin_callback`]).
    generation: AtomicU64,
    /// Per worker: the last generation it passed a wake on for (`fanout`).
    propagated: Box<[AtomicU64]>,
    trace: Option<Trace>,
    parking: Parking,
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

    /// Wakes up to `n` parked eligible workers. SeqCst: the caller's half of
    /// the park handshake, after the SeqCst open.
    fn wake(&self, n: usize) {
        let eligible = self.eligible.load(Ordering::Relaxed);
        self.parking.wake(n, eligible, |w| {
            if let Some(trace) = &self.trace {
                if let Some(at) = trace.signalled_at.get(w) {
                    at.store(trace.now(), Ordering::SeqCst);
                }
                trace.signalled.fetch_add(1, Ordering::Relaxed);
            }
            self.signal(w);
        });
    }

    /// Wakes worker `w`, or lets its next block return at once: the only
    /// way any path (segment open, callback start, requests, shutdown)
    /// wakes a worker.
    fn signal(&self, w: usize) {
        match &self.blocker {
            Blocker::Park => {
                if let Some(thread) = self.threads.get().and_then(|t| t.get(w)) {
                    thread.unpark();
                }
            }
            Blocker::Waiter(waiter) => waiter.signal(w),
        }
    }

    /// Blocks worker `w` (its own thread) until it is signalled.
    fn block(&self, w: usize) {
        if let Some(trace) = &self.trace {
            trace.idle_ends(w, trace.now());
            if let Some(parks) = trace.parks.get(w) {
                parks.fetch_add(1, Ordering::Relaxed);
            }
        }
        match &self.blocker {
            Blocker::Park => thread::park(),
            Blocker::Waiter(waiter) => waiter.wait(w),
        }
    }

    /// Worker `w` runs again after a block (or decided not to block):
    /// records its wake's latency against the wake that sent it, and under
    /// `fanout` passes the wake on, once per callback generation.
    fn resumed(&self, w: usize) {
        if let Some(trace) = &self.trace {
            let at = trace
                .signalled_at
                .get(w)
                .map_or(0, |at| at.swap(0, Ordering::SeqCst));
            if at != 0 {
                if let Some(histogram) = trace.wake.get(w) {
                    histogram.record(trace.now().saturating_sub(at));
                }
                trace.observed.fetch_add(1, Ordering::Relaxed);
            }
            trace.idle_starts(w, trace.now());
        }
        if self.policy == RtPolicy::Fanout
            && let Some(propagated) = self.propagated.get(w)
        {
            let generation = self.generation.load(Ordering::SeqCst);
            if propagated.load(Ordering::Relaxed) < generation {
                propagated.store(generation, Ordering::Relaxed);
                self.wake(2);
            }
        }
    }

    /// A worker joined a job: this callback's first and last join.
    fn joined(&self) {
        if let Some(trace) = &self.trace {
            let now = trace.now();
            trace.first_join.fetch_min(now, Ordering::Relaxed);
            trace.last_join.fetch_max(now, Ordering::Relaxed);
        }
    }

    /// Whether an idle worker still spins: `idle` is its last job or wake,
    /// `resumed` when it last ran again after a block.
    fn spins(&self, idle: Instant, resumed: Instant) -> bool {
        match self.budget_spin {
            Some(budget) => resumed.elapsed() < budget,
            None => idle.elapsed() < self.spin,
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
    /// Starts `workers` threads that wait as `schedule`'s policy says
    /// (control-thread code; it allocates): they spin for its `spin_ns` (at
    /// most [`MAX_SPIN`]) or, under `budget-spin`, until 0.9 × its
    /// `budget_ns` has passed since they last resumed, then block on
    /// `waiter` under `semaphore` when one is given, else park. A thread the
    /// system refuses is left out.
    pub(crate) fn new(
        workers: usize,
        schedule: &Schedule,
        waiter: Option<Arc<dyn Waiter>>,
    ) -> Pool {
        let (gate, opener) = Gate::new();
        let budget_spin = (schedule.policy == RtPolicy::BudgetSpin && schedule.budget_ns > 0)
            .then(|| Duration::from_nanos(schedule.budget_ns / 10 * 9).min(MAX_SPIN));
        let blocker = match waiter {
            Some(waiter) if schedule.policy == RtPolicy::Semaphore => Blocker::Waiter(waiter),
            _ => Blocker::Park,
        };
        let shared = Arc::new(Shared {
            gate,
            spin: Duration::from_nanos(schedule.spin_ns).min(MAX_SPIN),
            budget_spin,
            policy: schedule.policy,
            blocker,
            eligible: AtomicUsize::new(workers),
            generation: AtomicU64::new(0),
            propagated: (0..workers).map(|_| AtomicU64::new(0)).collect(),
            trace: schedule.trace.then(|| Trace::new(workers)),
            parking: Parking::new(workers),
            threads: OnceLock::new(),
            shutdown: AtomicBool::new(false),
            stats: PoolStats {
                trace: schedule.trace,
                ..PoolStats::default()
            },
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

    /// Whether workers block on the policy's [`Waiter`].
    pub(crate) fn waits_on_waiter(&self) -> bool {
        matches!(self.shared.blocker, Blocker::Waiter(_))
    }

    /// A callback starts (the audio thread; allocates, locks and blocks
    /// nowhere): a new generation, the trace's start and, under `semaphore`
    /// and `fanout` when `parallel` (the plan can run on the workers in this
    /// callback), the callback-start wake: every parked worker of the
    /// `eligible` first, or two of them that pass it on.
    pub(crate) fn begin_callback(&self, parallel: bool, eligible: usize) {
        let shared = &*self.shared;
        shared.eligible.store(eligible, Ordering::Relaxed);
        shared.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(trace) = &shared.trace {
            trace.callback_start.store(trace.now(), Ordering::Relaxed);
            trace.first_join.store(u64::MAX, Ordering::Relaxed);
            trace.last_join.store(0, Ordering::Relaxed);
            shared.stats.slowest_task_ns.store(0, Ordering::Relaxed);
        }
        if parallel {
            match shared.policy {
                RtPolicy::Semaphore => shared.wake(usize::MAX),
                RtPolicy::Fanout => shared.wake(2),
                RtPolicy::Park50 | RtPolicy::BudgetSpin => {}
            }
        }
    }

    /// The callback ends (the audio thread): folds its joins and its
    /// slowest task into the trace. A callback without a join adds none.
    pub(crate) fn end_callback(&self) {
        let shared = &*self.shared;
        let Some(trace) = &shared.trace else {
            return;
        };
        trace.callbacks.fetch_add(1, Ordering::Relaxed);
        let start = trace.callback_start.load(Ordering::Relaxed);
        let first = trace.first_join.load(Ordering::Relaxed);
        if first == u64::MAX {
            trace.without_join.fetch_add(1, Ordering::Relaxed);
        } else {
            trace.first.record(first.saturating_sub(start));
            let last = trace.last_join.load(Ordering::Relaxed);
            trace.last.record(last.saturating_sub(start));
        }
        let slowest = shared.stats.slowest_task_ns.swap(0, Ordering::Relaxed);
        if slowest > 0 {
            trace.slowest.record(slowest);
        }
    }

    /// The wake-up trace so far (control-thread code), if kept.
    pub(crate) fn wake_stats(&self) -> Option<WakeStats> {
        let trace = self.shared.trace.as_ref()?;
        let all = Histogram::new();
        for histogram in trace.wake.iter() {
            histogram.add_to(&all);
        }
        Some(WakeStats {
            callbacks: trace.callbacks.load(Ordering::Relaxed),
            callbacks_without_join: trace.without_join.load(Ordering::Relaxed),
            wakes_signalled: trace.signalled.load(Ordering::Relaxed),
            wakes_observed: trace.observed.load(Ordering::Relaxed),
            wake: all.spread(),
            per_worker: trace
                .wake
                .iter()
                .map(Histogram::spread)
                .collect::<Vec<Spread>>(),
            first_join: trace.first.spread(),
            last_join: trace.last.spread(),
            slowest_task: trace.slowest.spread(),
            worker_task_ns: self.shared.stats.worker_task_ns.load(Ordering::Relaxed),
            job_ns: load_all(&trace.job_ns),
            spin_ns: load_all(&trace.spin_ns),
            parks: load_all(&trace.parks),
            worker_budget: 0,
        })
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
    /// Runs `job` on the calling thread (lane 0) and the first `eligible`
    /// workers, and returns once every task is complete and every worker
    /// has left it.
    pub(crate) fn run(&mut self, job: &Job<'_>, eligible: usize) {
        let shared = self.shared;
        // Before the open, whose SeqCst swap publishes it to every joiner.
        shared.eligible.store(eligible, Ordering::Relaxed);
        self.opener
            .serve(job, || shared.wake(1), |job| job.participate(0));
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SeqCst: a blocking worker stores its flag and then reads
        // `shutdown`; it either sees it or is signalled here after blocking.
        self.shared.shutdown.store(true, Ordering::SeqCst);
        for w in 0..self.handles.len() {
            self.shared.signal(w);
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
    /// [`wait`](Workers::wait) returns. A worker whose `f` panics ends: it no
    /// longer counts, and the pool runs one worker short until the engine is
    /// rebuilt. (A task's failure never ends a worker; it stops the engine,
    /// `docs/semantics.md` §14.)
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
        for w in 0..shared.done.len() {
            shared.signal(w);
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

/// The values of per-worker counters (control-thread code).
fn load_all(counters: &[AtomicU64]) -> Vec<u64> {
    counters.iter().map(|c| c.load(Ordering::Relaxed)).collect()
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
/// spinning between jobs and blocking after the pool's spin
/// ([`park::run_worker`]).
fn run(shared: &Shared, w: usize) {
    let _alive = Alive(shared, w);
    park::run_worker(shared, w);
}

/// The pool's side of the worker loop: its requests, gate, timing and
/// blocker.
impl park::Worker for Shared {
    /// When the worker last ran a job or was woken, and when it last ran
    /// again after blocking.
    type Clock = (Instant, Instant);

    fn start(&self) -> (Instant, Instant) {
        let now = Instant::now();
        (now, now)
    }

    fn shutdown(&self, order: Ordering) -> bool {
        self.shutdown.load(order)
    }

    fn serve(&self, w: usize) {
        Shared::serve(self, w);
    }

    fn pending(&self, w: usize) -> bool {
        Shared::pending(self, w)
    }

    fn join(&self, w: usize, seen: &mut u64) -> bool {
        // `eligible` is read after the gate's word (`Gate::join_if`), and the
        // caller stores it before opening: a worker that sees a job open sees
        // that job's value or a later one. A value that is stale around a
        // plan swap only lets a worker join or skip one job it should not
        // have: timing only, since the calling thread runs every task no
        // worker takes. A worker that may not join marks the job seen, so
        // that it parks instead of spinning until it closes.
        let may_join = || w < self.eligible.load(Ordering::Relaxed);
        self.gate.join_if(may_join, seen, |job| {
            let began = self.trace.as_ref().map(|trace| {
                let now = trace.now();
                trace.idle_ends(w, now);
                now
            });
            self.joined();
            self.wake(2);
            job.participate(w + 1);
            if let (Some(trace), Some(began)) = (&self.trace, began) {
                let now = trace.now();
                if let Some(job) = trace.job_ns.get(w) {
                    job.fetch_add(now.saturating_sub(began), Ordering::Relaxed);
                }
                trace.idle_starts(w, now);
            }
        })
    }

    fn joinable(&self, seen: u64) -> bool {
        self.gate.joinable(seen)
    }

    fn spins(&self, &(idle, resumed): &(Instant, Instant)) -> bool {
        Shared::spins(self, idle, resumed)
    }

    fn tick(&self, clock: &mut (Instant, Instant), resumed: bool) {
        if resumed {
            clock.1 = Instant::now();
        } else {
            clock.0 = Instant::now();
        }
    }

    fn parking(&self) -> &Parking {
        &self.parking
    }

    fn block(&self, w: usize) {
        Shared::block(self, w);
    }

    fn resumed(&self, w: usize) {
        Shared::resumed(self, w);
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;
    use crate::shared::{Access, PlanCells};

    use std::sync::Condvar;
    use std::sync::mpsc;

    /// Waits (polling, up to a ceiling far above any bound, so a loaded
    /// machine cannot fail a test by being slow) until `done`.
    fn eventually(done: impl Fn() -> bool) -> bool {
        let ceiling = Instant::now() + Duration::from_secs(5);
        while !done() && Instant::now() < ceiling {
            thread::sleep(Duration::from_millis(1));
        }
        done()
    }

    fn all_parked(shared: &Shared) -> bool {
        shared
            .alive
            .iter()
            .enumerate()
            .all(|(w, a)| shared.parking.is_parked(w) || !a.load(Ordering::SeqCst))
    }

    /// Runs one (empty) job on the pool.
    fn run_job(pool: &mut Pool, epoch: u64) {
        let mut cells = PlanCells::default();
        let body = |_: &mut Access<'_>, _: usize| {};
        let (mut dispatch, stats) = pool.split();
        let job = Job::new(&mut cells, epoch, &body, stats);
        dispatch.run(&job, usize::MAX);
    }

    /// A counting semaphore per worker (a `Mutex` and `Condvar`: tests only),
    /// as the `semaphore` policy's [`Waiter`], counting every signal and
    /// every return from a wait.
    struct Counting {
        state: Mutex<(Vec<u64>, usize)>,
        changed: Condvar,
        signals: AtomicU64,
        returns: AtomicU64,
    }

    impl Counting {
        fn new(workers: usize) -> Arc<Counting> {
            Arc::new(Counting {
                state: Mutex::new((vec![0; workers], 0)),
                changed: Condvar::new(),
                signals: AtomicU64::new(0),
                returns: AtomicU64::new(0),
            })
        }

        /// Every worker waits and no signal is pending.
        fn quiet(&self, workers: usize) -> bool {
            let state = self.state.lock().unwrap();
            state.1 == workers && state.0.iter().all(|&n| n == 0)
        }
    }

    impl Waiter for Counting {
        fn wait(&self, w: usize) {
            let mut state = self.state.lock().unwrap();
            state.1 += 1;
            while state.0[w] == 0 {
                state = self.changed.wait(state).unwrap();
            }
            state.0[w] -= 1;
            state.1 -= 1;
            self.returns.fetch_add(1, Ordering::SeqCst);
        }

        fn signal(&self, w: usize) {
            let mut state = self.state.lock().unwrap();
            state.0[w] += 1;
            self.signals.fetch_add(1, Ordering::SeqCst);
            self.changed.notify_all();
        }
    }

    /// AC-idle (M2.6-5, M2.7-1 C12): a spin longer than [`MAX_SPIN`] is
    /// clamped to it, and under every policy, after a job and a callback
    /// start, every worker parks once its bound has passed.
    #[test]
    fn idle_workers_park_after_the_spin_bound() {
        for policy in RtPolicy::ALL {
            let schedule = Schedule {
                spin_ns: 1_000_000_000,
                budget_ns: 1_000_000_000,
                policy,
                ..Schedule::DEFAULT
            };
            let mut pool = Pool::new(3, &schedule, None);
            assert_eq!(pool.shared.spin, MAX_SPIN);
            let budget = (policy == RtPolicy::BudgetSpin).then_some(MAX_SPIN);
            assert_eq!(pool.shared.budget_spin, budget);
            run_job(&mut pool, 1);
            pool.begin_callback(true, usize::MAX);
            pool.end_callback();
            assert!(
                eventually(|| all_parked(&pool.shared)),
                "{policy}: a worker still spins 5 s after its last job"
            );
        }
        // budget-spin: 0.9 × the budget, when that is below the bound.
        let schedule = Schedule {
            policy: RtPolicy::BudgetSpin,
            budget_ns: 333_333,
            ..Schedule::DEFAULT
        };
        let pool = Pool::new(1, &schedule, None);
        assert_eq!(pool.shared.budget_spin, Some(Duration::from_nanos(299_997)));
    }

    /// M2.10-5: under every policy, workers beyond a job's `eligible` never
    /// join it and are never woken for it, and every worker parks after its
    /// bound. Eligibility is stored before the open, and a joiner reads it
    /// after the open, so this holds whatever the timing.
    #[test]
    fn ineligible_workers_never_join_and_park() {
        for policy in RtPolicy::ALL {
            let schedule = Schedule {
                policy,
                trace: true,
                ..Schedule::DEFAULT
            };
            let mut pool = Pool::new(3, &schedule, None);
            for n in 0..500u64 {
                pool.begin_callback(true, 1);
                let mut cells = PlanCells::default();
                let body = |_: &mut Access<'_>, _: usize| {};
                let (mut dispatch, stats) = pool.split();
                let job = Job::new(&mut cells, n + 1, &body, stats);
                dispatch.run(&job, 1);
                pool.end_callback();
                if n % 50 == 0 {
                    thread::sleep(Duration::from_micros(200));
                }
            }
            assert!(
                eventually(|| all_parked(&pool.shared)),
                "{policy}: a worker still spins 5 s after its last job"
            );
            let wake = pool.wake_stats().unwrap();
            assert_eq!(&wake.job_ns[1..], &[0, 0], "{policy}: {wake:?}");
            let woken: Vec<u64> = wake.per_worker.iter().map(|s| s.count).collect();
            assert_eq!(&woken[1..], &[0, 0], "{policy}: {wake:?}");
        }
    }

    /// C12 (auditor-2): every wake path reaches workers blocked on the
    /// `semaphore` policy's waiter: the callback-start wake, a segment's,
    /// posted requests and the shutdown. No wake is lost: once every worker
    /// waits again, the waits that returned equal the signals sent (requests
    /// and the shutdown included, whose surplus counts are spurious wakes);
    /// the trace saw every wake it stamped; a callback that cannot go
    /// parallel wakes nobody; and the pool drops while every worker waits.
    #[test]
    fn every_wake_path_reaches_workers_blocked_on_the_waiter() {
        const WORKERS: usize = 3;
        let waiter = Counting::new(WORKERS);
        let schedule = Schedule {
            policy: RtPolicy::Semaphore,
            trace: true,
            ..Schedule::DEFAULT
        };
        let mut pool = Pool::new(WORKERS, &schedule, Some(waiter.clone()));
        assert!(pool.waits_on_waiter());
        assert!(eventually(|| waiter.quiet(WORKERS)), "the workers wait");

        // Posted requests, while every worker waits.
        let workers = pool.handle(WORKERS + 1);
        let ran = Arc::new(AtomicU64::new(0));
        for _ in 0..3 {
            let ran = Arc::clone(&ran);
            let generation = workers.post(move |_| {
                ran.fetch_add(1, Ordering::SeqCst);
            });
            assert!(workers.wait(generation, Duration::from_secs(5)).complete());
            assert!(eventually(|| waiter.quiet(WORKERS)));
        }
        assert_eq!(ran.load(Ordering::SeqCst), 3 * WORKERS as u64);

        // A callback that cannot go parallel wakes nobody.
        let before = waiter.signals.load(Ordering::SeqCst);
        pool.begin_callback(false, usize::MAX);
        pool.end_callback();
        assert_eq!(waiter.signals.load(Ordering::SeqCst), before);

        // Callback starts, with jobs in some, at varying gaps.
        let mut rng = 1u64;
        for n in 0..2_000u64 {
            pool.begin_callback(true, usize::MAX);
            if n % 3 == 0 {
                run_job(&mut pool, n + 1);
            }
            pool.end_callback();
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            if rng >> 62 == 0 {
                thread::sleep(Duration::from_micros(rng >> 54 & 127));
            }
        }
        assert!(
            eventually(|| waiter.quiet(WORKERS)),
            "the workers wait again"
        );
        assert_eq!(
            waiter.returns.load(Ordering::SeqCst),
            waiter.signals.load(Ordering::SeqCst),
            "every signal was consumed"
        );
        let wake = pool.wake_stats().unwrap();
        assert!(wake.wakes_signalled > 0);
        assert_eq!(wake.wakes_observed, wake.wakes_signalled, "{wake:?}");
        assert_eq!(wake.callbacks, 2_001);
        assert_eq!(wake.wake.count, wake.wakes_observed);
        assert_eq!(wake.per_worker.len(), WORKERS);

        // The shutdown, while every worker waits.
        let (done, dropped) = mpsc::channel();
        thread::spawn(move || {
            drop(pool);
            let _ = done.send(());
        });
        assert!(
            dropped.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the pool did not stop"
        );
    }

    /// C12 (auditor-1-02): under `fanout` a callback start reaches the
    /// parked workers through a tree that ends: each worker passes the wake
    /// on at most once per callback, so a callback sends at most 2 + 2W
    /// wakes, and the workers all park again.
    #[test]
    fn fanout_propagation_ends() {
        const WORKERS: usize = 6;
        let schedule = Schedule {
            policy: RtPolicy::Fanout,
            trace: true,
            ..Schedule::DEFAULT
        };
        let pool = Pool::new(WORKERS, &schedule, None);
        for _ in 0..200 {
            assert!(eventually(|| all_parked(&pool.shared)));
            let before = pool.wake_stats().unwrap().wakes_signalled;
            pool.begin_callback(true, usize::MAX);
            pool.end_callback();
            assert!(eventually(|| all_parked(&pool.shared)), "the tree ended");
            let sent = pool.wake_stats().unwrap().wakes_signalled - before;
            assert!((2..=2 + 2 * WORKERS as u64).contains(&sent), "{sent}");
        }
        let wake = pool.wake_stats().unwrap();
        assert!(eventually(
            || pool.wake_stats().unwrap().wakes_observed == wake.wakes_signalled
        ));
    }
}
