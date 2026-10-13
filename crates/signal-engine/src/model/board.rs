//! B1–B4: task completion and failure, with successors
//! (`docs/testing-and-evaluation.md`).

use std::sync::Arc;

use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use loom::thread;

use super::{counters, explore, flags, plan, set, wait};
use crate::board::{FAILED_PANIC, TaskBoard};
use crate::shared::{Access, Gate, Job, PoolStats};

/// Tasks 0 and 1, both feeding task 2.
const PREDS: [u32; 3] = [0, 0, 2];
const SUCCESSORS: [&[u32]; 3] = [&[2], &[2], &[]];

/// The board of [`PREDS`] reset for epoch 1.
fn board() -> TaskBoard {
    let mut board = TaskBoard::new(
        PREDS.to_vec(),
        SUCCESSORS.iter().map(|s| s.to_vec()).collect(),
    );
    board.reset(1);
    board
}

/// Runs `board` on the caller (lane 0) and two workers, each calling
/// `run(lane, t)` for the tasks it claims. Without a failure the caller
/// returns only once every task is complete. After a failure it returns once
/// the claims it saw are settled; a worker may still claim a successor the
/// failure released and settle it without running it (the gate's close, not
/// the board, waits for that), so every claim is settled once every thread
/// has returned.
fn run_on_three(board: &Arc<TaskBoard>, run: Arc<dyn Fn(usize, usize) -> bool + Send + Sync>) {
    let workers: Vec<_> = (1..=2)
        .map(|lane| {
            let (board, run) = (Arc::clone(board), Arc::clone(&run));
            thread::spawn(move || board.participate(1, lane, |t| run(lane, t)))
        })
        .collect();
    board.participate(1, 0, |t| run(0, t));
    assert!(
        board.failed() || board.all_complete(),
        "the caller returned before every task completed"
    );
    for w in workers {
        w.join().unwrap();
    }
    assert!(board.drained(), "a claim was never settled");
}

/// B1: successors. Each task runs exactly once, task 2 only after tasks 0
/// and 1 have completed and with their writes visible (Relaxed writes,
/// ordered only by the board's Release completion and Acquire claim).
#[test]
fn b1_completion_releases_successors() {
    explore("B1", 1, || {
        let board = Arc::new(board());
        let runs = counters(3);
        let written = counters(2);
        let run = {
            let (runs, written) = (Arc::clone(&runs), Arc::clone(&written));
            Arc::new(move |_lane: usize, t: usize| {
                if let Some(w) = written.get(t) {
                    w.store(10 + t as u64, Ordering::Relaxed);
                } else {
                    let seen: Vec<u64> =
                        written.iter().map(|w| w.load(Ordering::Relaxed)).collect();
                    assert_eq!(seen, [10, 11], "task 2 ran before its producers' writes");
                }
                if let Some(r) = runs.get(t) {
                    r.fetch_add(1, Ordering::Relaxed);
                }
                true
            })
        };
        run_on_three(&board, run);
        assert!(!board.failed());
        assert!(runs.iter().all(|r| r.load(Ordering::Relaxed) == 1));
        assert!((0..3).all(|t| board.done_in(t, 1)));
    });
}

/// B2 (a): failure with successors. Task 0 fails (its run reports false):
/// the job fails with a task panic, task 0 is never marked done, its
/// successor never runs, every claim is settled exactly once, and every
/// thread returns.
#[test]
fn b2_a_failed_task_releases_nobody() {
    explore("B2a", 1, || {
        let board = Arc::new(board());
        let runs = counters(3);
        let run = {
            let runs = Arc::clone(&runs);
            Arc::new(move |_lane: usize, t: usize| {
                if let Some(r) = runs.get(t) {
                    r.fetch_add(1, Ordering::Relaxed);
                }
                t != 0
            })
        };
        run_on_three(&board, run);
        check_failed(&board, &runs);
    });
}

/// What B2 checks after the job.
fn check_failed(board: &TaskBoard, runs: &[AtomicU64]) {
    assert_eq!(board.failure(), FAILED_PANIC, "the job did not fail");
    assert!(!board.done_in(0, 1), "a failed task was marked done");
    let ran = |t: usize| runs.get(t).map_or(0, |r| r.load(Ordering::Relaxed));
    assert_eq!(ran(0), 1);
    assert!(ran(1) <= 1);
    assert_eq!(ran(2), 0, "the failed task's successor ran");
    assert!(board.drained());
}

/// B2 (b): the same through the gate and a real job on the caller and two
/// workers: task 0's body panics, and `Job::run` catches it.
#[test]
fn b2_b_a_panicking_task_through_a_job() {
    quiet_task_0();
    explore("B2b", 1, || {
        let (gate, mut opener) = Gate::new();
        let go = flags(2);
        let workers: Vec<_> = (0..2)
            .map(|w| {
                let (gate, go) = (Arc::clone(&gate), Arc::clone(&go));
                thread::spawn(move || {
                    wait(&go[w]);
                    let mut seen = 0;
                    gate.join(&mut seen, |job| job.participate(w + 1));
                })
            })
            .collect();
        let threads: Vec<_> = workers.iter().map(|w| w.thread().clone()).collect();
        let runs = counters(3);
        let body = {
            let runs = Arc::clone(&runs);
            move |_: &mut Access<'_>, t: usize| {
                if let Some(r) = runs.get(t) {
                    r.fetch_add(1, Ordering::Relaxed);
                }
                assert_ne!(t, 0, "task 0 panics");
            }
        };
        let stats = PoolStats::default();
        let mut cells = plan(&PREDS, &SUCCESSORS, 3);
        let job = Job::new(&mut cells, 1, &body, &stats);
        opener.serve(
            &job,
            || {
                for (w, thread) in threads.iter().enumerate() {
                    if let Some(go) = go.get(w) {
                        set(go, thread);
                    }
                }
            },
            |job| job.participate(0),
        );
        assert_eq!(gate.word().state(), (0, false));
        assert_eq!(job.failure(), Some(crate::log::FailureReason::TaskPanic));
        assert_eq!(
            stats
                .task_failures
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        check_failed(cells.board(), &runs);
        for w in workers {
            w.join().unwrap();
        }
    });
}

/// The panics B2b's body raises by design are caught by the job; leaves
/// them out of the test output, which every explored execution would fill.
fn quiet_task_0() {
    static QUIET: std::sync::Once = std::sync::Once::new();
    QUIET.call_once(|| {
        let report = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !info.to_string().contains("task 0 panics") {
                report(info);
            }
        }));
    });
}

/// B3: two epochs on one board. After a job of epoch 1 the board is reset
/// for epoch 2: the ready slots it left are never claimed as epoch 2's, and
/// each task runs once per epoch, the chain 0 → 1 in order.
#[test]
fn b3_epochs_reuse_the_board() {
    explore("B3", 1, || {
        let mut board = Arc::new({
            let mut board = TaskBoard::new(vec![0, 1], vec![vec![1], Vec::new()]);
            board.reset(1);
            board
        });
        let runs = counters(4);
        for epoch in 1..=2_u64 {
            if epoch == 2 {
                Arc::get_mut(&mut board).unwrap().reset(2);
            }
            let first = Arc::new(AtomicBool::new(false));
            let run = {
                let (runs, first) = (Arc::clone(&runs), Arc::clone(&first));
                move |t: usize| {
                    if t == 0 {
                        first.store(true, Ordering::Relaxed);
                    } else {
                        assert!(first.load(Ordering::Relaxed), "task 1 ran before task 0");
                    }
                    if let Some(r) = runs.get((epoch as usize - 1) * 2 + t) {
                        r.fetch_add(1, Ordering::Relaxed);
                    }
                    true
                }
            };
            let workers: Vec<_> = (1..=2)
                .map(|lane| {
                    let (board, run) = (Arc::clone(&board), run.clone());
                    thread::spawn(move || board.participate(epoch, lane, run))
                })
                .collect();
            board.participate(epoch, 0, run);
            assert!(board.all_complete());
            for w in workers {
                w.join().unwrap();
            }
        }
        assert!(runs.iter().all(|r| r.load(Ordering::Relaxed) == 1));
    });
}

/// B4 (test hooks): a claim that finds its ready slot empty fails the job
/// as an invariant and settles itself, so the caller's drain ends and the
/// workers return.
#[cfg(feature = "test-hooks")]
#[test]
fn b4_a_missing_slot_ends_the_job() {
    explore("B4", 1, || {
        let mut board = board();
        board.arm(crate::board::INJECT_MISSING_SLOT);
        let board = Arc::new(board);
        let run: Arc<dyn Fn(usize, usize) -> bool + Send + Sync> = Arc::new(|_, _| true);
        run_on_three(&board, run);
        assert_eq!(board.failure(), crate::board::FAILED_INVARIANT);
    });
}
