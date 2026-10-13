//! G1–G3: publishing a job, joining and leaving it, a stale worker against
//! a closed gate, and epoch reuse (`docs/testing-and-evaluation.md`).

use std::sync::Arc;

use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use loom::thread;

use super::{all_eq, counters, counting, explore, flags, plan, set, wait};
use crate::gate::GateWord;
use crate::shared::{Gate, Job, PoolStats};

/// G1: the caller and three workers. Each worker joins a job at most once,
/// each task runs once, and once `serve` returns no worker is joined and
/// none touches the job again.
#[test]
fn g1_publish_join_and_leave() {
    explore("G1", 2, || {
        let (gate, mut opener) = Gate::new();
        let live = Arc::new(AtomicBool::new(true));
        let workers: Vec<_> = (0..3)
            .map(|w| {
                let (gate, live) = (Arc::clone(&gate), Arc::clone(&live));
                thread::spawn(move || {
                    let mut seen = 0;
                    let mut joins = 0;
                    for _ in 0..2 {
                        let joined = gate.join(&mut seen, |job| {
                            assert!(live.load(Ordering::Acquire), "a join after serve returned");
                            job.participate(w + 1);
                        });
                        joins += u32::from(joined);
                    }
                    assert!(joins <= 1, "worker {w} joined one epoch twice");
                })
            })
            .collect();
        let runs = counters(2);
        let body = counting(&runs);
        let stats = PoolStats::default();
        let mut cells = plan(&[0, 0], &[&[], &[]], 4);
        let job = Job::new(&mut cells, 1, &body, &stats);
        opener.serve(&job, || {}, |job| job.participate(0));
        live.store(false, Ordering::Release);
        assert_eq!(
            gate.word().state(),
            (0, false),
            "joined or open after serve"
        );
        assert!(all_eq(&runs, 1), "a task did not run exactly once");
        assert_eq!(job.failure(), None);
        for w in workers {
            w.join().unwrap();
        }
    });
}

/// G2: the caller publishes two jobs in turn, and two workers try to join
/// each. A worker that loaded the word of the first job and paused fails its
/// join once the gate has closed (loom puts the pause between the load and
/// the compare-and-swap); every join sees the job of the epoch it joined and
/// the data the caller wrote before publishing it; the job slot's rewrite
/// for the second job races with no read of the first (loom's cell check).
#[test]
fn g2_a_stale_worker_against_a_closed_gate() {
    explore("G2", 2, || {
        let (gate, mut opener) = Gate::new();
        let generation = Arc::new(AtomicU64::new(0));
        let workers: Vec<_> = (0..2)
            .map(|w| {
                let (gate, generation) = (Arc::clone(&gate), Arc::clone(&generation));
                thread::spawn(move || {
                    let mut seen = 0;
                    for _ in 0..2 {
                        gate.join(&mut seen, |job| {
                            assert_eq!(
                                job.epoch(),
                                generation.load(Ordering::Relaxed),
                                "a join saw another epoch's job"
                            );
                            job.participate(w + 1);
                        });
                    }
                })
            })
            .collect();
        let runs = counters(4);
        let stats = PoolStats::default();
        let mut cells = plan(&[0, 0], &[&[], &[]], 3);
        for epoch in 1..=2_u64 {
            generation.store(epoch, Ordering::Relaxed);
            let runs = Arc::clone(&runs);
            let body = move |_: &mut crate::shared::Access<'_>, t: usize| {
                let at = (epoch as usize - 1) * 2 + t;
                if let Some(r) = runs.get(at) {
                    r.fetch_add(1, Ordering::Relaxed);
                }
            };
            let job = Job::new(&mut cells, epoch, &body, &stats);
            opener.serve(&job, || {}, |job| job.participate(0));
            assert_eq!(gate.word().state(), (0, false));
        }
        assert!(all_eq(&runs, 1), "a task did not run exactly once per job");
        for w in workers {
            w.join().unwrap();
        }
    });
}

/// G3: epoch reuse, with a gate whose epoch has one bit, so the caller's
/// three jobs are published at epochs 1, 0 and 1 again. Worker A reads the
/// gate's word while the first job is open and nobody has joined, then
/// pauses. Worker B joins and leaves every job, and the caller closes each
/// only after B has joined it. A's join fails against the second job (its
/// word is stale). Against the third its word matches again, so it joins,
/// as ADR 0020 allows ("the gate is open for a live job"), and it must see
/// the third job and its data, never the first. Each job's tasks run once.
#[test]
fn g3_epoch_reuse() {
    explore("G3", 2, || {
        let (gate, mut opener) = Gate::with_word(GateWord::with_epoch_bits(1));
        let main = thread::current();
        let generation = Arc::new(AtomicU64::new(0));
        // A: go to load, loaded, go to try the second, tried, go to join the
        // third, joined and left. B: go and joined, per job.
        let (a, b) = (flags(6), flags(6));
        let worker_a = {
            let (gate, generation, a, main) = (
                Arc::clone(&gate),
                Arc::clone(&generation),
                Arc::clone(&a),
                main.clone(),
            );
            thread::spawn(move || {
                let mut seen = 0;
                wait(&a[0]);
                let word = gate.word().load();
                set(&a[1], &main);
                wait(&a[2]);
                let stale = gate.join_loaded(word, &mut seen, |_| {});
                assert!(!stale, "a stale word joined the second job");
                set(&a[3], &main);
                wait(&a[4]);
                let joined = gate.join_loaded(word, &mut seen, |job| {
                    assert_eq!(job.epoch(), 3, "the reused epoch joined an old job");
                    assert_eq!(generation.load(Ordering::Relaxed), 3);
                    job.participate(1);
                });
                assert!(joined, "the reused word did not join the live job");
                set(&a[5], &main);
            })
        };
        let worker_b = {
            let (gate, b, main) = (Arc::clone(&gate), Arc::clone(&b), main.clone());
            thread::spawn(move || {
                let mut seen = 0;
                for k in 0..3 {
                    wait(&b[2 * k]);
                    let joined = gate.join(&mut seen, |job| {
                        set(&b[2 * k + 1], &main);
                        job.participate(2);
                    });
                    assert!(joined, "B could not join job {k}");
                }
            })
        };
        let (thread_a, thread_b) = (worker_a.thread().clone(), worker_b.thread().clone());
        let runs = counters(6);
        let stats = PoolStats::default();
        let mut cells = plan(&[0, 0], &[&[], &[]], 3);
        for k in 0..3_usize {
            let epoch = k as u64 + 1;
            generation.store(epoch, Ordering::Relaxed);
            let runs = Arc::clone(&runs);
            let body = move |_: &mut crate::shared::Access<'_>, t: usize| {
                if let Some(r) = runs.get(k * 2 + t) {
                    r.fetch_add(1, Ordering::Relaxed);
                }
            };
            let job = Job::new(&mut cells, epoch, &body, &stats);
            opener.serve(
                &job,
                || {
                    // A acts while nobody has joined, then B joins.
                    set(&a[2 * k], &thread_a);
                    wait(&a[2 * k + 1]);
                    set(&b[2 * k], &thread_b);
                },
                |job| {
                    job.participate(0);
                    wait(&b[2 * k + 1]);
                },
            );
            assert_eq!(gate.word().state(), (0, false));
        }
        assert!(all_eq(&runs, 1), "a task did not run exactly once per job");
        worker_a.join().unwrap();
        worker_b.join().unwrap();
    });
}
