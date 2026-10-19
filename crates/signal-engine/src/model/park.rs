//! P1–P3 and S1–S3: parking and waking with no lost wake, and shutdown
//! while workers are parked or joined (`docs/testing-and-evaluation.md`).
//! The workers run the real loop (`park::run_worker`) and handshake
//! (`Parking`) over the real gate; [`Pool`] stands in for `pool.rs`'s side
//! of it: no spinning (a worker that finds nothing blocks at once), loom's
//! `park`/`unpark` as the blocker, and a request counter for `Workers::post`.
//! M1 adds the worker budget (M2.10-5): the eligible workers, through the
//! same `Gate::join_if` and `Parking::wake` limit as `pool.rs`.

use std::sync::{Arc, OnceLock};

use loom::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use loom::thread::{self, JoinHandle, Thread};

use super::{all_eq, counters, counting, explore, flags, plan, set, wait};
use crate::park::{self, Parking, Worker};
use crate::shared::{Gate, Job, Opener, PoolStats};

/// The model's pool of two workers.
struct Pool {
    gate: Arc<Gate>,
    parking: Parking,
    /// The workers start once every handle is known (so that a wake from a
    /// worker can always reach another).
    start: AtomicBool,
    shutdown: AtomicBool,
    /// The latest request and, per worker, the last it ran.
    posted: AtomicU64,
    done: Box<[AtomicU64]>,
    /// Per worker: it joined a job; it ran a request.
    joined: Arc<[AtomicBool]>,
    served: Arc<[AtomicBool]>,
    /// Whether a job is being served: a worker in a job asserts it.
    live: AtomicBool,
    /// S3: a joined worker stays in the job until shutdown is requested.
    hold: bool,
    /// The workers that may join a job and be woken for it (`pool.rs`'s
    /// `Shared::eligible`): every worker, except in M1.
    eligible: AtomicUsize,
    /// The thread that waits for `joined` and `served`.
    waiter: Thread,
    threads: OnceLock<Vec<Thread>>,
}

const WORKERS: usize = 2;

impl Pool {
    fn new(gate: Arc<Gate>, hold: bool) -> Arc<Pool> {
        Arc::new(Pool {
            gate,
            parking: Parking::new(WORKERS),
            start: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            posted: AtomicU64::new(0),
            done: (0..WORKERS).map(|_| AtomicU64::new(0)).collect(),
            joined: flags(WORKERS),
            served: flags(WORKERS),
            live: AtomicBool::new(false),
            hold,
            eligible: AtomicUsize::new(WORKERS),
            waiter: thread::current(),
            threads: OnceLock::new(),
        })
    }

    /// Wakes worker `w` (or makes its next park return at once).
    fn signal(&self, w: usize) {
        if let Some(thread) = self.threads.get().and_then(|t| t.get(w)) {
            thread.unpark();
        }
    }

    /// The segment's wake after the open: at most one parked worker
    /// (`Dispatch::run`), among the eligible ones (`Shared::wake`).
    fn wake(&self, n: usize) {
        let eligible = self.eligible.load(Ordering::Relaxed);
        self.parking.wake(n, eligible, |w| self.signal(w));
    }

    /// Starts the workers.
    fn spawn(self: &Arc<Pool>) -> Vec<JoinHandle<()>> {
        let workers: Vec<_> = (0..WORKERS)
            .map(|w| {
                let pool = Arc::clone(self);
                thread::spawn(move || {
                    wait(&pool.start);
                    park::run_worker(&*pool, w);
                })
            })
            .collect();
        let _ = self
            .threads
            .set(workers.iter().map(|w| w.thread().clone()).collect());
        self.start.store(true, Ordering::Release);
        for w in &workers {
            w.thread().unpark();
        }
        workers
    }

    /// Shuts the workers down as `Pool::drop` does, and joins them.
    fn stop(&self, workers: Vec<JoinHandle<()>>) {
        self.request_shutdown();
        for w in workers {
            w.join().unwrap();
        }
    }

    /// SeqCst: a blocking worker stores its flag and then reads `shutdown`;
    /// it either sees it or is signalled here after blocking.
    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for w in 0..WORKERS {
            self.signal(w);
        }
    }

    /// Serves one job of `tasks` independent tasks on the caller (lane 0),
    /// with the segment's wake, then `then` on the caller before the close.
    fn serve(&self, opener: &mut Opener, tasks: usize, then: impl FnOnce()) {
        let runs = counters(tasks);
        let body = counting(&runs);
        let stats = PoolStats::default();
        let preds = vec![0; tasks];
        let successors: Vec<&[u32]> = vec![&[]; tasks];
        let mut cells = plan(&preds, &successors, WORKERS + 1);
        let job = Job::new(&mut cells, 1, &body, &stats);
        self.live.store(true, Ordering::Release);
        opener.serve(
            &job,
            || self.wake(1),
            |job| {
                job.participate(0);
                then();
            },
        );
        self.live.store(false, Ordering::Release);
        assert_eq!(
            self.gate.word().state(),
            (0, false),
            "joined or open after serve"
        );
        assert!(all_eq(&runs, 1), "a task did not run exactly once");
    }
}

impl Worker for Pool {
    type Clock = ();

    fn start(&self) {}

    fn shutdown(&self, order: Ordering) -> bool {
        self.shutdown.load(order)
    }

    fn serve(&self, w: usize) {
        let posted = self.posted.load(Ordering::SeqCst);
        if let Some(done) = self.done.get(w)
            && posted > done.load(Ordering::Relaxed)
        {
            done.store(posted, Ordering::Release);
            if let Some(served) = self.served.get(w) {
                set(served, &self.waiter);
            }
        }
    }

    fn pending(&self, w: usize) -> bool {
        self.done
            .get(w)
            .is_some_and(|d| self.posted.load(Ordering::SeqCst) > d.load(Ordering::Relaxed))
    }

    fn join(&self, w: usize, seen: &mut u64) -> bool {
        // As `pool.rs`: the gate's word, then eligibility, then the join.
        let may_join = || w < self.eligible.load(Ordering::Relaxed);
        self.gate.join_if(may_join, seen, |job| {
            assert!(self.live.load(Ordering::Acquire), "a join outside serve");
            if let Some(joined) = self.joined.get(w) {
                set(joined, &self.waiter);
            }
            // As `pool.rs`: a joined worker wakes up to two more.
            self.wake(2);
            if self.hold {
                while !self.shutdown.load(Ordering::SeqCst) {
                    thread::park();
                }
            }
            job.participate(w + 1);
        })
    }

    fn joinable(&self, seen: u64) -> bool {
        self.gate.joinable(seen)
    }

    fn spins(&self, _: &()) -> bool {
        false
    }

    fn tick(&self, _: &mut (), _: bool) {}

    fn parking(&self) -> &Parking {
        &self.parking
    }

    fn block(&self, _: usize) {
        thread::park();
    }

    fn resumed(&self, _: usize) {}
}

/// P1: no lost wake. Two workers start and park; the caller opens a job
/// and wakes one, and each worker that joins wakes another (as `pool.rs`
/// does), so both must join: the caller waits for both before it closes.
/// A lost wake would leave a worker parked and the caller parked, which
/// loom reports as a deadlock.
#[test]
fn p1_no_lost_wake() {
    explore("P1", 2, || {
        let (gate, mut opener) = Gate::new();
        let pool = Pool::new(gate, false);
        let workers = pool.spawn();
        pool.serve(&mut opener, 2, || {
            for joined in pool.joined.iter() {
                wait(joined);
            }
        });
        pool.stop(workers);
    });
}

/// P2: the callback-start wake of `semaphore` (every parked worker) before
/// the segment's open and its wake of one: both workers still join.
///
/// Ignored: loom 0.7.2 reports a deadlock here in an execution the C11
/// model forbids. A worker stores its flag (`true`) and fences; both later
/// swaps of that flag (the caller's wake after the open, the other worker's
/// wake) read `false`, the caller's own earlier swap, although the worker's
/// fence orders its store with the caller's. loom lets a read-modify-write
/// read any store that is maximal in its partial modification order
/// (`rt/atomic.rs`, `match_rmw_to_stores`), without the reader's causality.
/// The handshake is checked apart from loom instead (M2.7-10,
/// `docs/testing-and-evaluation.md`, "The park/wake handshake"): every
/// interleaving of this scenario, with 1–3 workers under sequential consistency (1–2 under the store-buffer mode) and two callbacks, by
/// `tests/handshake.rs` (`a_wake_before_the_open_loses_no_wake`), and the
/// wake before the open forced at each point of the worker's next park on
/// real threads by `park::tests::the_wake_before_the_open_is_never_lost_on_real_threads`
/// (also `pool::tests::every_wake_path_reaches_workers_blocked_on_the_waiter`).
/// Run it with `-- --ignored` to see the report.
#[test]
#[ignore = "loom 0.7.2 false alarm, see the doc comment; checked by tests/handshake.rs \
            and park::tests::the_wake_before_the_open_is_never_lost_on_real_threads"]
fn p2_a_wake_before_the_open_is_not_lost() {
    explore("P2", 2, || {
        let (gate, mut opener) = Gate::new();
        let pool = Pool::new(gate, false);
        let workers = pool.spawn();
        pool.wake(usize::MAX);
        pool.serve(&mut opener, 2, || {
            for joined in pool.joined.iter() {
                wait(joined);
            }
        });
        pool.stop(workers);
    });
}

/// P3: a request posted while the workers decide whether to park
/// (`Workers::post`: SeqCst, then every worker signalled): both run it.
#[test]
fn p3_a_posted_request_reaches_every_worker() {
    explore("P3", 2, || {
        let (gate, _opener) = Gate::new();
        let pool = Pool::new(gate, false);
        let workers = pool.spawn();
        pool.posted.store(1, Ordering::SeqCst);
        for w in 0..WORKERS {
            pool.signal(w);
        }
        for served in pool.served.iter() {
            wait(served);
        }
        pool.stop(workers);
    });
}

/// M1 (M2.10-5, the worker budget): job 1 may use one worker (eligible 1,
/// stored before the open as `Dispatch::run` does), job 2 both. Worker 1
/// joins job 1 and worker 2 never does: it skips it or parks with its flag
/// set, which job 1's wakes leave alone. Job 2 must still reach worker 2,
/// through the production path (the gate's word, then eligibility, then
/// `join_loaded`; the wake limit): the caller waits for its join before it
/// closes, so a lost wake or a stale eligibility leaves every thread
/// parked, which loom reports as a deadlock.
#[test]
fn m1_a_larger_budget_reaches_a_skipped_worker() {
    // Two jobs: a bound of 1 keeps it within the time budget (25 s on the
    // seat; 2 had not finished after 30 minutes).
    explore("M1", 1, || {
        let (gate, mut opener) = Gate::new();
        let pool = Pool::new(gate, false);
        let workers = pool.spawn();
        pool.eligible.store(1, Ordering::Relaxed);
        pool.serve(&mut opener, 2, || wait(&pool.joined[0]));
        assert!(
            !pool.joined[1].load(Ordering::Acquire),
            "worker 2 joined a job it was not eligible for"
        );
        pool.eligible.store(2, Ordering::Relaxed);
        pool.serve(&mut opener, 2, || wait(&pool.joined[1]));
        pool.stop(workers);
    });
}

/// S1: shutdown while the workers are parked, or about to: both return.
#[test]
fn s1_shutdown_while_parked() {
    explore("S1", 2, || {
        let (gate, _opener) = Gate::new();
        let pool = Pool::new(gate, false);
        let workers = pool.spawn();
        pool.stop(workers);
    });
}

/// S2: shutdown straight after a job, as the real pool can only do it
/// (`Pool::drop` needs the pool, which `serve` borrows): workers that joined
/// have left before `serve` returns, none joins afterwards, and both exit.
#[test]
fn s2_shutdown_after_a_job() {
    explore("S2", 2, || {
        let (gate, mut opener) = Gate::new();
        let pool = Pool::new(gate, false);
        let workers = pool.spawn();
        pool.serve(&mut opener, 2, || {});
        pool.stop(workers);
    });
}

/// S3: shutdown while workers are joined, which is stricter than the real
/// pool allows. The caller serves a job on its own thread; each joined
/// worker stays in the job until the control thread, which waits for both
/// joins, requests shutdown. Every task still runs once (none is abandoned), the
/// caller's drain ends, `serve` returns only after every worker has left
/// (no join and no job access after it, see `live`), and both workers exit.
#[test]
fn s3_shutdown_while_joined() {
    // Four threads: a bound of 1 keeps it within the time budget (2 takes
    // about 100 s on the seat).
    explore("S3", 1, || {
        let (gate, opener) = Gate::new();
        let pool = Pool::new(gate, true);
        let workers = pool.spawn();
        let caller = {
            let pool = Arc::clone(&pool);
            thread::spawn(move || {
                let mut opener = opener;
                // The gate stays open until shutdown is requested, so a
                // worker joins.
                pool.serve(&mut opener, 1, || {
                    while !pool.shutdown.load(Ordering::SeqCst) {
                        thread::park();
                    }
                });
            })
        };
        // The control thread: once both workers are in the job, shut down.
        // (Both, so that no join unparks this thread while it joins the
        // others below, which loom's join does not tolerate.)
        for joined in pool.joined.iter() {
            wait(joined);
        }
        pool.request_shutdown();
        caller.thread().unpark();
        caller.join().unwrap();
        for w in workers {
            w.join().unwrap();
        }
    });
}
