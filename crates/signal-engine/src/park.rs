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

    /// Wakes up to `n` parked workers, calling `signal(w)` for each worker
    /// `w` it takes the flag from. SeqCst: the waker's half of the park
    /// handshake, after its SeqCst open.
    pub(crate) fn wake(&self, n: usize, mut signal: impl FnMut(usize)) {
        // Between the waker's open (or post) and its reads of the flags, the
        // fence's half of the handshake (see the module documentation).
        fence(Ordering::SeqCst);
        let mut woken = 0;
        for (w, parked) in self.parked.iter().enumerate() {
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
