//! The gate word through which the caller publishes a job to the workers and
//! takes it back (ADR 0020, "Publication"): one atomic word holding the
//! epoch above bit 16, then the `open` bit, then the number of joined
//! workers. It knows epochs and joins, nothing of the job, which
//! `shared::Gate` keeps beside it. The model tests (`crate::model`,
//! `docs/testing-and-evaluation.md`) run this code under loom.

use std::fmt;

use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sync::hint::spin_loop;

const OPEN: u64 = 1 << 15;
const JOINED: u64 = OPEN - 1;
const EPOCH_SHIFT: u32 = 16;

/// The gate's word: opened for a new epoch by its one opener, joined and
/// left by workers, closed by the opener, which then waits until every
/// joined worker has left.
pub(crate) struct GateWord {
    word: AtomicU64,
    /// The epochs the word holds before they wrap: 48 bits, fewer in the
    /// model tests of epoch reuse.
    epoch_mask: u64,
}

impl fmt::Debug for GateWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GateWord")
            .field("word", &self.word.load(Ordering::Relaxed))
            .finish()
    }
}

impl GateWord {
    /// A closed gate at epoch 0.
    pub(crate) fn new() -> GateWord {
        GateWord {
            word: AtomicU64::new(0),
            epoch_mask: u64::MAX >> EPOCH_SHIFT,
        }
    }

    /// Model tests only: a gate whose epoch wraps after `bits` bits.
    #[cfg(all(test, loom))]
    pub(crate) fn with_epoch_bits(bits: u32) -> GateWord {
        GateWord {
            word: AtomicU64::new(0),
            epoch_mask: (1 << bits) - 1,
        }
    }

    fn epoch(word: u64) -> u64 {
        word >> EPOCH_SHIFT
    }

    /// Whether a job newer than `seen` is open (SeqCst: the worker's half of
    /// the park handshake).
    pub(crate) fn joinable(&self, seen: u64) -> bool {
        let word = self.word.load(Ordering::SeqCst);
        word & OPEN != 0 && GateWord::epoch(word) != seen
    }

    /// The word, as a join reads it first (Acquire).
    pub(crate) fn load(&self) -> u64 {
        self.word.load(Ordering::Acquire)
    }

    /// Joins the open epoch if `word`, read by [`load`](GateWord::load), is
    /// still the gate's word and its epoch is newer than `seen`, recording
    /// it in `seen`; `None` if it did not join. The worker leaves when the
    /// returned guard drops (also while unwinding). A join is `load` then
    /// this; the model tests put a worker's pause between the two.
    pub(crate) fn join_loaded(&self, word: u64, seen: &mut u64) -> Option<Joined<'_>> {
        if word & OPEN == 0 || GateWord::epoch(word) == *seen || word & JOINED == JOINED {
            return None;
        }
        // AcqRel: Acquire pairs with the opener's SeqCst (so Release) open,
        // so everything the opener wrote before opening is visible after
        // this. It succeeds only if the gate is still open for this epoch: a
        // worker that paused after loading `word` fails here once the gate
        // has closed. If the epoch wrapped and the word matches again, the
        // gate is open for a live job, which the worker then joins.
        self.word
            .compare_exchange(word, word + 1, Ordering::AcqRel, Ordering::Relaxed)
            .ok()?;
        *seen = GateWord::epoch(word);
        Some(Joined(&self.word))
    }

    /// Opens the gate for the next epoch and returns the guard that closes
    /// it. Only the gate's one opener calls it, with the gate closed and no
    /// worker joined (every guard waits for that before it drops).
    pub(crate) fn open(&self) -> Close<'_> {
        let word = self.word.load(Ordering::Acquire);
        let epoch = GateWord::epoch(word).wrapping_add(1) & self.epoch_mask;
        // SeqCst: publishes what the opener wrote before (Release), and
        // orders the open before the opener's SeqCst reads of the parked
        // flags (the park handshake).
        self.word
            .swap((epoch << EPOCH_SHIFT) | OPEN, Ordering::SeqCst);
        Close(&self.word)
    }

    /// Marks the epoch of `word`, read by [`load`](GateWord::load), seen if
    /// it is open, without joining it: a worker that may not join this job
    /// (M2.10-5, an ineligible worker) then parks instead of re-reading it.
    pub(crate) fn skip_loaded(&self, word: u64, seen: &mut u64) {
        if word & OPEN != 0 {
            *seen = GateWord::epoch(word);
        }
    }

    /// Tests only: the number of joined workers and whether the gate is open.
    #[cfg(test)]
    pub(crate) fn state(&self) -> (u64, bool) {
        let word = self.word.load(Ordering::SeqCst);
        (word & JOINED, word & OPEN != 0)
    }
}

/// A joined worker; it leaves when dropped (also while unwinding).
pub(crate) struct Joined<'a>(&'a AtomicU64);

impl Drop for Joined<'_> {
    fn drop(&mut self) {
        // Release: every access this worker made to the job happens before
        // the opener's Acquire load that sees `joined == 0`.
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// Closes the gate and waits until every joined worker has left, when
/// dropped: on the opener's return and while it unwinds.
pub(crate) struct Close<'a>(&'a AtomicU64);

impl Drop for Close<'_> {
    fn drop(&mut self) {
        self.0.fetch_and(!OPEN, Ordering::AcqRel);
        // Acquire: pairs with every worker's Release leave.
        while self.0.load(Ordering::Acquire) & JOINED != 0 {
            spin_loop();
        }
    }
}
