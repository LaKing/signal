//! The task board of a parallel segment (ADR 0019 §2, ADR 0020,
//! `docs/semantics.md` §14): the tasks' dependency counters, the ready queue,
//! each task's completion and its outcome, and the loop every thread of a job
//! runs over them ([`TaskBoard::participate`]). Atomics only; it knows tasks
//! and epochs, nothing of the engine's data. The model tests
//! (`crate::model`, `docs/testing-and-evaluation.md`) run this code under loom.

use std::fmt;

use crate::sync::SetExclusive;
use crate::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use crate::sync::hint::spin_loop;

/// What the task board says when asked for work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    Task(usize),
    /// Some task is not claimed yet but none is ready.
    NotYet,
    /// Every task is claimed.
    AllClaimed,
}

/// The tasks of a plan and their dependency counters (atomics only), and
/// whether the job failed (`docs/semantics.md` §14).
#[derive(Default)]
pub(crate) struct TaskBoard {
    preds: Box<[u32]>,
    succ_start: Box<[usize]>,
    succ: Box<[u32]>,
    pending: Box<[AtomicU64]>,
    /// The epoch in which each task last completed successfully; a failed
    /// task's stays older, so its rows are never read.
    done: Box<[AtomicU64]>,
    /// Ready tasks in the order they became ready, with the epoch each was
    /// pushed in.
    ready: Box<[AtomicU64]>,
    ready_epoch: Box<[AtomicU64]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    /// Claims settled: one per claim that advanced `head`, by the task's
    /// completion or, for a claim that found no task, by the claim itself.
    completed: AtomicUsize,
    /// Why this job failed, the first reason only: 0 (it has not),
    /// [`FAILED_PANIC`] or [`FAILED_INVARIANT`].
    failed: AtomicU8,
    /// Test hooks only: a one-shot broken invariant (`PlanCells::inject`).
    #[cfg(feature = "test-hooks")]
    inject: AtomicU8,
}

/// Something unwound in a task outside a primitive's call.
pub(crate) const FAILED_PANIC: u8 = 1;
/// The board found itself inconsistent.
pub(crate) const FAILED_INVARIANT: u8 = 2;

/// Test hooks only: [`TaskBoard::arm`]'s values.
#[cfg(feature = "test-hooks")]
pub(crate) const INJECT_DROP_READY: u8 = 1;
#[cfg(feature = "test-hooks")]
pub(crate) const INJECT_MISSING_SLOT: u8 = 2;

impl fmt::Debug for TaskBoard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskBoard")
            .field("tasks", &self.preds.len())
            .finish()
    }
}

impl TaskBoard {
    pub(crate) fn new(preds: Vec<u32>, successors: Vec<Vec<u32>>) -> TaskBoard {
        let n = preds.len();
        let mut succ_start = Vec::with_capacity(n + 1);
        let mut succ = Vec::new();
        for list in &successors {
            succ_start.push(succ.len());
            succ.extend_from_slice(list);
        }
        succ_start.resize(n + 1, succ.len());
        let atomics = |_| AtomicU64::new(0);
        TaskBoard {
            preds: preds.into_boxed_slice(),
            succ_start: succ_start.into_boxed_slice(),
            succ: succ.into_boxed_slice(),
            pending: (0..n).map(atomics).collect(),
            done: (0..n).map(atomics).collect(),
            ready: (0..n).map(atomics).collect(),
            ready_epoch: (0..n).map(atomics).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            failed: AtomicU8::new(0),
            #[cfg(feature = "test-hooks")]
            inject: AtomicU8::new(0),
        }
    }

    /// Resets the counters for a segment of epoch `epoch` and pushes the
    /// tasks without predecessors, in task order. Exclusive: before the job
    /// is published.
    pub(crate) fn reset(&mut self, epoch: u64) {
        for (pending, &preds) in self.pending.iter_mut().zip(self.preds.iter()) {
            pending.set_exclusive(u64::from(preds));
        }
        self.head.set_exclusive(0);
        self.tail.set_exclusive(0);
        self.completed.set_exclusive(0);
        self.failed.set_exclusive(0);
        for t in 0..self.preds.len() {
            if self.preds.get(t) == Some(&0) {
                self.push(t, epoch);
            }
        }
    }

    /// Test hooks only: arms the one-shot broken invariant `which`.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn arm(&mut self, which: u8) {
        self.inject.set_exclusive(which);
    }

    /// Test hooks only: whether the armed one-shot invariant `which` fires.
    #[cfg(feature = "test-hooks")]
    fn injected(&self, which: u8) -> bool {
        self.inject
            .compare_exchange(which, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    fn push(&self, t: usize, epoch: u64) {
        #[cfg(feature = "test-hooks")]
        if self.injected(INJECT_DROP_READY) {
            self.fail(FAILED_INVARIANT);
            return;
        }
        let at = self.tail.fetch_add(1, Ordering::AcqRel);
        if let (Some(slot), Some(slot_epoch)) = (self.ready.get(at), self.ready_epoch.get(at)) {
            slot.store(t as u64, Ordering::Relaxed);
            // Release: a claimant that sees this epoch sees the task index.
            slot_epoch.store(epoch, Ordering::Release);
        } else {
            // No slot: the task would never run. Nothing waits for it after
            // the failure (it is never claimed).
            self.fail(FAILED_INVARIANT);
        }
    }

    pub(crate) fn claim(&self, epoch: u64) -> Next {
        loop {
            let head = self.head.load(Ordering::Acquire);
            if head >= self.preds.len() {
                return Next::AllClaimed;
            }
            let ready = self
                .ready_epoch
                .get(head)
                .is_some_and(|e| e.load(Ordering::Acquire) == epoch);
            if !ready {
                return Next::NotYet;
            }
            let task = self.ready.get(head).map(|s| s.load(Ordering::Relaxed));
            if self
                .head
                .compare_exchange(head, head + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                #[cfg(feature = "test-hooks")]
                let task = task.filter(|_| !self.injected(INJECT_MISSING_SLOT));
                return match task {
                    Some(t) => Next::Task(t as usize),
                    None => {
                        // A claim with no task: it settles itself, as a
                        // failed completion, so a drain never waits for it.
                        self.fail(FAILED_INVARIANT);
                        self.completed.fetch_add(1, Ordering::AcqRel);
                        Next::NotYet
                    }
                };
            }
        }
    }

    /// Task `t` is complete: Release its writes (if it succeeded), then
    /// release its successors. A failed task (`ok` false) fails the job and
    /// never marks itself done, so no successor reads its rows.
    pub(crate) fn complete(&self, t: usize, epoch: u64, ok: bool) {
        if !ok {
            self.fail(FAILED_PANIC);
        } else if let Some(done) = self.done.get(t) {
            done.store(epoch, Ordering::Release);
        }
        let start = self.succ_start.get(t).copied().unwrap_or(0);
        let end = self.succ_start.get(t + 1).copied().unwrap_or(start);
        for &s in self.succ.get(start..end).unwrap_or(&[]) {
            let s = s as usize;
            // AcqRel: the last predecessor's decrement sees every other
            // predecessor's completion before it pushes the successor.
            match self.pending.get(s) {
                Some(p) => {
                    if p.fetch_sub(1, Ordering::AcqRel) == 1 {
                        self.push(s, epoch);
                    }
                }
                None => self.fail(FAILED_INVARIANT),
            }
        }
        self.completed.fetch_add(1, Ordering::AcqRel);
    }

    /// Whether task `t` completed successfully in epoch `epoch`. Acquire:
    /// pairs with the Release store of its completion, so its writes happen
    /// before what the caller reads next.
    pub(crate) fn done_in(&self, t: usize, epoch: u64) -> bool {
        self.done
            .get(t)
            .is_some_and(|d| d.load(Ordering::Acquire) == epoch)
    }

    pub(crate) fn all_complete(&self) -> bool {
        self.completed.load(Ordering::Acquire) >= self.preds.len()
    }

    /// The job failed for `why` (Release: before any claimant that sees it
    /// skips its task); the first reason stays.
    pub(crate) fn fail(&self, why: u8) {
        let _ = self
            .failed
            .compare_exchange(0, why, Ordering::AcqRel, Ordering::Acquire);
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire) != 0
    }

    /// Why the job failed: 0, [`FAILED_PANIC`] or [`FAILED_INVARIANT`].
    pub(crate) fn failure(&self) -> u8 {
        self.failed.load(Ordering::Acquire)
    }

    /// Every claim made so far is settled: read `head`, then `completed`.
    /// After a failure the caller waits only for this, never for tasks that
    /// were never claimed.
    pub(crate) fn drained(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        self.completed.load(Ordering::Acquire) >= head
    }

    /// Takes part as lane `lane` in the job of epoch `epoch` until every
    /// task is claimed (a worker) or complete (the caller, lane 0), or,
    /// after a failure, until it has nothing in hand (a worker) or every
    /// claim is settled (the caller). `run(t)` runs task `t` and says whether
    /// it succeeded; it is not called once the job has failed. Every claimed
    /// task is completed exactly once, with that outcome, before anything
    /// else, so no thread waits for this claim (and lane 0 never waits on its
    /// own).
    pub(crate) fn participate(&self, epoch: u64, lane: usize, mut run: impl FnMut(usize) -> bool) {
        loop {
            let next = self.claim(epoch);
            if let Next::Task(t) = next {
                let ran = !self.failed() && run(t);
                self.complete(t, epoch, ran);
            }
            if self.failed() {
                if lane == 0 {
                    while !self.drained() {
                        spin_loop();
                    }
                }
                break;
            }
            match next {
                Next::Task(_) => {}
                Next::NotYet => {
                    if lane == 0 && self.all_complete() {
                        break;
                    }
                    spin_loop();
                }
                Next::AllClaimed => {
                    if lane == 0 {
                        while !self.all_complete() && !(self.failed() && self.drained()) {
                            spin_loop();
                        }
                    }
                    break;
                }
            }
        }
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    /// AC-TASK: a failed task fails the job and is never marked done.
    #[test]
    fn a_failed_task_is_never_done() {
        let mut board = TaskBoard::new(vec![0, 1], vec![vec![1], Vec::new()]);
        board.reset(3);
        assert_eq!(board.claim(3), Next::Task(0));
        board.complete(0, 3, false);
        assert!(board.failed());
        assert_eq!(board.failure(), FAILED_PANIC);
        assert!(!board.done_in(0, 3));
        assert!(board.drained());
    }

    /// Path 11: a claim that advances `head` and finds its slot empty settles
    /// itself as a failed completion, so a drain never waits for it. The
    /// board is reset whole (the slot's epoch published), then only `ready`
    /// is cut short.
    #[test]
    fn a_claim_with_an_empty_slot_settles_itself() {
        let mut board = TaskBoard::new(vec![0], vec![Vec::new()]);
        board.reset(1);
        board.ready = Box::new([]);
        assert_eq!(board.claim(1), Next::NotYet);
        assert!(board.failed());
        assert_eq!(board.head.load(Ordering::Relaxed), 1);
        assert_eq!(board.completed.load(Ordering::Relaxed), 1);
        assert!(board.drained());
        assert_eq!(board.claim(1), Next::AllClaimed);
    }
}
