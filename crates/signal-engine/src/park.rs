//! Parking and waking the worker threads (ADR 0020, "Waking";
//! `docs/realtime-rules.md` "Worker threads"): the `parked` flags, the wake
//! that clears them, and each worker's loop ([`run_worker`]). It knows
//! flags and the steps of the loop, nothing of the pool's threads, timing or
//! requests, which `pool.rs` supplies through [`Worker`]. The model tests
//! (`crate::model`, `docs/testing-and-evaluation.md`) run this code under
//! loom.
//!
//! The handshake loses no wake: a blocking worker stores its flag (SeqCst),
//! fences (SeqCst) and then reads the gate (SeqCst); the caller opens the
//! gate (SeqCst), fences (SeqCst) and then reads the flags (SeqCst). In the
//! single order of those fences, either the worker sees the open gate and
//! does not block, or the caller sees the flag and wakes it. The SeqCst
//! accesses alone already give that order; the fences (M2.7-6) make it hold
//! in loom's model too, which treats SeqCst accesses as AcqRel and found
//! the lost wake without them (`docs/testing-and-evaluation.md`, P1).
//! M2.7-10 checks it apart from loom: an explorer of every interleaving
//! (`tests/handshake.rs`, which maps each of its steps to a line here) and a
//! real-thread reproducer of the wake before the open (`tests` below; both
//! in `docs/testing-and-evaluation.md`, "The park/wake handshake").

use crate::sync::atomic::{AtomicBool, Ordering, fence};
use crate::sync::hint::spin_loop;

/// One `parked` flag per worker.
#[derive(Debug, Default)]
pub(crate) struct Parking {
    parked: Box<[AtomicBool]>,
}

impl Parking {
    pub(crate) fn new(workers: usize) -> Parking {
        Parking {
            parked: (0..workers).map(|_| AtomicBool::new(false)).collect(),
        }
    }

    /// Wakes up to `n` parked workers among the first `limit` (the eligible
    /// ones, M2.10-5), calling `signal(w)` for each worker `w` it takes the
    /// flag from; the flag of a worker at or beyond `limit` is left set, so
    /// the first wake that counts it eligible wakes it. SeqCst: the waker's
    /// half of the park handshake, after its SeqCst open.
    pub(crate) fn wake(&self, n: usize, limit: usize, mut signal: impl FnMut(usize)) {
        // Between the waker's open (or post) and its reads of the flags, the
        // fence's half of the handshake (see the module documentation).
        fence(Ordering::SeqCst);
        let mut woken = 0;
        for (w, parked) in self.parked.iter().enumerate().take(limit) {
            if woken >= n {
                break;
            }
            if parked.swap(false, Ordering::SeqCst) {
                signal(w);
                woken += 1;
            }
        }
    }

    /// Tests only: whether worker `w`'s flag is set.
    #[cfg(all(test, not(loom)))]
    pub(crate) fn is_parked(&self, w: usize) -> bool {
        self.parked.get(w).is_some_and(|p| p.load(Ordering::SeqCst))
    }
}

/// What a pool supplies to [`run_worker`].
pub(crate) trait Worker {
    /// The worker's idle timing (when it last ran a job or resumed).
    type Clock;
    /// The clock when the worker starts.
    fn start(&self) -> Self::Clock;
    /// Whether the pool shuts down, read with `order`.
    fn shutdown(&self, order: Ordering) -> bool;
    /// Runs worker `w`'s pending requests, between jobs.
    fn serve(&self, w: usize);
    /// Whether worker `w` has requests to run.
    fn pending(&self, w: usize) -> bool;
    /// Joins a job newer than `seen`, if one is open, runs it as worker `w`
    /// and leaves; whether it joined.
    fn join(&self, w: usize, seen: &mut u64) -> bool;
    /// Whether a job newer than `seen` is open (SeqCst).
    fn joinable(&self, seen: u64) -> bool;
    /// Whether an idle worker still spins.
    fn spins(&self, clock: &Self::Clock) -> bool;
    /// Stamps the clock: `resumed` when the worker runs again after blocking,
    /// else idle (after a job, a wake or a decision not to block).
    fn tick(&self, clock: &mut Self::Clock, resumed: bool);
    /// The flags.
    fn parking(&self) -> &Parking;
    /// Blocks worker `w` (its own thread) until it is signalled.
    fn block(&self, w: usize);
    /// Worker `w` runs again after a block, or decided not to block.
    fn resumed(&self, w: usize);
}

/// Worker `w`'s loop: runs requests, joins every new job, spins between
/// jobs, and blocks after the spin until signalled; returns on shutdown.
pub(crate) fn run_worker<P: Worker>(pool: &P, w: usize) {
    let mut seen = 0;
    let mut clock = pool.start();
    loop {
        if pool.shutdown(Ordering::Acquire) {
            return;
        }
        pool.serve(w);
        if pool.join(w, &mut seen) {
            pool.tick(&mut clock, false);
            continue;
        }
        if pool.spins(&clock) {
            for _ in 0..64 {
                spin_loop();
            }
            continue;
        }
        let Some(parked) = pool.parking().parked.get(w) else {
            return;
        };
        parked.store(true, Ordering::SeqCst);
        // Between the flag and the re-reads, the worker's fence (see the
        // module documentation).
        fence(Ordering::SeqCst);
        let blocks = !(pool.joinable(seen) || pool.shutdown(Ordering::SeqCst) || pool.pending(w));
        if blocks {
            pool.block(w);
        }
        parked.store(false, Ordering::SeqCst);
        if blocks {
            pool.tick(&mut clock, true);
        }
        // A wake sent while it decided not to block is answered too.
        pool.resumed(w);
        pool.tick(&mut clock, false);
    }
}

/// M2.7-10: the wake before the open on real threads. The worker runs the
/// real loop over the real flags and gate; its pool ([`Forced`]) pauses it
/// at the calls the loop makes around the handshake, so the caller's open
/// and wake land at a chosen point of the worker's park after the
/// callback-start wake took its flag (loom's P2, `crate::model`).
#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;
    use crate::gate::GateWord;

    use std::sync::atomic::{AtomicU8, AtomicU64};
    use std::sync::{Arc, Condvar, Mutex, OnceLock};
    use std::thread::{self, Thread};
    use std::time::{Duration, Instant};

    /// Where the caller's open and wake land in the worker's park.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum At {
        /// Before the flag store (`parking`): the re-read must see the open.
        BeforeFlag = 1,
        /// After the flag store and the fence (`joinable`): the caller's
        /// wake takes the flag and the re-read sees the open.
        BeforeReread = 2,
        /// After the re-read saw the gate closed (`pending`): the caller's
        /// wake takes the flag; the park returns on the kept token.
        BeforePark = 3,
        /// In the park (`block`, once the worker has entered it).
        InPark = 4,
    }

    const CLOSED: u8 = 1;
    const OPEN: u8 = 2;

    /// How the worker blocks: `park`'s token, or a counting semaphore (a
    /// `Mutex` and `Condvar`, standing in for `semaphore`'s Mach waiter).
    enum Blocker {
        Park(OnceLock<Thread>),
        Count(Mutex<u64>, Condvar),
    }

    struct Forced {
        gate: GateWord,
        parking: Parking,
        shutdown: AtomicBool,
        blocker: Blocker,
        /// The worker's passes towards a park (counted in `parking`), the
        /// last pass before the callback-start wake took its flag, and the
        /// pass in which it last entered `block`.
        passes: AtomicU64,
        after: AtomicU64,
        blocked: AtomicU64,
        /// The hold armed by that wake for a later pass, and the one held.
        armed: AtomicU8,
        held: AtomicU8,
        release: AtomicBool,
        /// The next re-read is recorded in `read` (CLOSED or OPEN).
        watch: AtomicBool,
        read: AtomicU8,
        joins: AtomicU64,
    }

    impl Forced {
        fn signal(&self) {
            match &self.blocker {
                Blocker::Park(thread) => thread.get().unwrap().unpark(),
                Blocker::Count(count, changed) => {
                    *count.lock().unwrap() += 1;
                    changed.notify_one();
                }
            }
        }

        /// Wakes up to `n` parked workers; how many it took. With `arm`,
        /// a flag it takes arms that hold for a pass that starts later.
        fn wake(&self, n: usize, arm: Option<At>) -> usize {
            let mut took = 0;
            self.parking.wake(n, usize::MAX, |_| {
                if let Some(at) = arm {
                    self.after
                        .store(self.passes.load(Ordering::SeqCst), Ordering::SeqCst);
                    self.armed.store(at as u8, Ordering::SeqCst);
                }
                took += 1;
                self.signal();
            });
            took
        }

        /// Holds the worker here, until released, if this hold is armed and
        /// its pass started after the wake that armed it.
        fn hold(&self, at: At) -> bool {
            if self.armed.load(Ordering::SeqCst) != at as u8
                || self.passes.load(Ordering::SeqCst) <= self.after.load(Ordering::SeqCst)
            {
                return false;
            }
            self.armed.store(0, Ordering::SeqCst);
            self.held.store(at as u8, Ordering::SeqCst);
            while !self.release.swap(false, Ordering::SeqCst)
                && !self.shutdown.load(Ordering::SeqCst)
            {
                thread::yield_now();
            }
            true
        }
    }

    impl Worker for Forced {
        type Clock = ();

        fn start(&self) {}

        fn shutdown(&self, order: Ordering) -> bool {
            self.shutdown.load(order)
        }

        fn serve(&self, _: usize) {}

        fn pending(&self, _: usize) -> bool {
            if self.hold(At::BeforePark) {
                self.read.store(CLOSED, Ordering::SeqCst);
            }
            false
        }

        fn join(&self, _: usize, seen: &mut u64) -> bool {
            let joined = self.gate.join_loaded(self.gate.load(), seen).is_some();
            if joined {
                self.joins.fetch_add(1, Ordering::SeqCst);
            }
            joined
        }

        fn joinable(&self, seen: u64) -> bool {
            let held = self.hold(At::BeforeReread);
            let open = self.gate.joinable(seen);
            if held || self.watch.swap(false, Ordering::SeqCst) {
                self.read
                    .store(if open { OPEN } else { CLOSED }, Ordering::SeqCst);
            }
            open
        }

        fn spins(&self, _: &()) -> bool {
            false
        }

        fn tick(&self, _: &mut (), _: bool) {}

        fn parking(&self) -> &Parking {
            self.passes.fetch_add(1, Ordering::SeqCst);
            if self.hold(At::BeforeFlag) {
                self.watch.store(true, Ordering::SeqCst);
            }
            &self.parking
        }

        fn block(&self, _: usize) {
            self.blocked
                .store(self.passes.load(Ordering::SeqCst), Ordering::SeqCst);
            match &self.blocker {
                Blocker::Park(_) => thread::park(),
                Blocker::Count(count, changed) => {
                    let mut count = count.lock().unwrap();
                    while *count == 0 {
                        count = changed.wait(count).unwrap();
                    }
                    *count -= 1;
                }
            }
        }

        fn resumed(&self, _: usize) {}
    }

    /// Stops the worker when dropped, a failed assertion included.
    struct Stop(Arc<Forced>);

    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.shutdown.store(true, Ordering::SeqCst);
            self.0.signal();
        }
    }

    /// Waits up to a ceiling far above any scheduling delay until `done`.
    /// Whether `done` was seen (it may not hold any more by the return).
    fn within(done: impl Fn() -> bool) -> bool {
        let ceiling = Instant::now() + Duration::from_secs(5);
        loop {
            if done() {
                return true;
            }
            if Instant::now() >= ceiling {
                return false;
            }
            thread::yield_now();
        }
    }

    /// Runs `callbacks` callbacks per position on one worker; per position,
    /// those in which the callback-start wake took the flag and the open
    /// and wake then landed where forced.
    fn force(blocker: Blocker, callbacks: u64) -> [u64; 4] {
        let pool = Arc::new(Forced {
            gate: GateWord::new(),
            parking: Parking::new(1),
            shutdown: AtomicBool::new(false),
            blocker,
            passes: AtomicU64::new(0),
            after: AtomicU64::new(0),
            blocked: AtomicU64::new(0),
            armed: AtomicU8::new(0),
            held: AtomicU8::new(0),
            release: AtomicBool::new(false),
            watch: AtomicBool::new(false),
            read: AtomicU8::new(0),
            joins: AtomicU64::new(0),
        });
        let worker = {
            let pool = Arc::clone(&pool);
            thread::spawn(move || run_worker(&*pool, 0))
        };
        if let Blocker::Park(thread) = &pool.blocker {
            let _ = thread.set(worker.thread().clone());
        }
        let stop = Stop(Arc::clone(&pool));
        let mut forced = [0; 4];
        let positions = [At::BeforeFlag, At::BeforeReread, At::BeforePark, At::InPark];
        for n in 0..callbacks * 4 {
            let at = positions[(n % 4) as usize];
            let parked = || {
                let pass = pool.passes.load(Ordering::SeqCst);
                pool.parking.is_parked(0) && pool.blocked.load(Ordering::SeqCst) == pass
            };
            assert!(within(parked), "the worker parks");
            pool.armed.store(0, Ordering::SeqCst);
            pool.held.store(0, Ordering::SeqCst);
            pool.read.store(0, Ordering::SeqCst);
            // The callback-start wake (`semaphore`), before the open. If it
            // took the flag, the worker's next pass is held at `at`, or, for
            // `InPark`, the caller waits until that pass entered `block`.
            let took = pool.wake(usize::MAX, Some(at));
            if took == 1 {
                let after = pool.after.load(Ordering::SeqCst);
                let reached = within(|| match at {
                    At::InPark => pool.blocked.load(Ordering::SeqCst) > after,
                    _ => pool.held.load(Ordering::SeqCst) == at as u8,
                });
                assert!(reached, "{at:?}, callback {n}: lost wake before the open");
            }
            // The segment's open and its wake of one.
            let close = pool.gate.open();
            let took_after = pool.wake(1, None);
            pool.release.store(true, Ordering::SeqCst);
            assert!(
                within(|| pool.joins.load(Ordering::SeqCst) == n + 1),
                "{at:?}, callback {n}: lost wake (took {took}, then {took_after})"
            );
            drop(close);
            pool.release.store(false, Ordering::SeqCst);
            if took == 1 {
                let read = pool.read.load(Ordering::SeqCst);
                match at {
                    At::BeforeFlag => assert_eq!(read, OPEN, "{at:?}, callback {n}"),
                    At::BeforeReread => assert_eq!((read, took_after), (OPEN, 1), "{at:?}"),
                    At::BeforePark => assert_eq!((read, took_after), (CLOSED, 1), "{at:?}"),
                    At::InPark => {}
                }
                forced[at as usize - 1] += 1;
            }
        }
        drop(stop);
        worker.join().unwrap();
        forced
    }

    /// AC-repro: the wake of every parked worker before the open (loom's P2)
    /// at each point of the worker's next park, thousands of times, with
    /// `park`'s token and with a counting semaphore: the worker always joins.
    #[test]
    fn the_wake_before_the_open_is_never_lost_on_real_threads() {
        const CALLBACKS: u64 = 3_000;
        for (name, blocker) in [
            ("park", Blocker::Park(OnceLock::new())),
            ("semaphore", Blocker::Count(Mutex::new(0), Condvar::new())),
        ] {
            let start = Instant::now();
            let forced = force(blocker, CALLBACKS);
            println!(
                "reproducer {name}: {} callbacks, forced per position (before flag, before \
                 re-read, before park, in park) {forced:?}, no lost wake, {:.2?}",
                CALLBACKS * 4,
                start.elapsed()
            );
            for (at, &n) in forced.iter().enumerate() {
                assert!(n >= 2_000, "{name}: position {at} forced only {n} times");
            }
        }
    }
}
