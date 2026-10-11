//! The counting allocator counts the current thread's heap calls inside a guard
//! and nothing outside it (ADR 0017).

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use signal_contract::alloc_counter::{self, Counts, Guard};

signal_contract::install_counting_allocator!();

#[test]
fn counts_allocations_reallocations_and_frees_inside_a_guard() {
    assert!(alloc_counter::is_installed());

    let guard = Guard::start();
    let mut v = black_box(Vec::<u64>::with_capacity(1));
    v.push(1);
    v.push(2); // beyond capacity: a reallocation
    drop(black_box(v));
    let counts = guard.counts();
    drop(guard);
    assert_eq!(
        counts,
        Counts {
            allocations: 2,
            deallocations: 1
        }
    );
}

#[test]
fn counts_nothing_outside_a_guard() {
    // Allocations before and after the guard are not in its count.
    let before = black_box(vec![0u8; 64]);
    let mut buffer = black_box(Vec::<f64>::with_capacity(16));
    let guard = Guard::start();
    for i in 0..16 {
        buffer.push(f64::from(i) * 0.5); // within capacity: no allocation
    }
    let sum: f64 = black_box(&buffer).iter().sum();
    let counts = guard.counts();
    drop(guard);
    let after = black_box(String::from("after"));
    drop((before, after));
    assert_eq!(sum, 60.0);
    assert!(counts.is_zero(), "{counts:?}");

    // With no guard active, nothing is recorded: a guard started afterwards
    // still sees zero.
    let guard = Guard::start();
    assert!(guard.counts().is_zero());
}

#[test]
fn another_threads_allocations_are_not_counted() {
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicU64::new(0));
    let worker = {
        let (stop, done) = (Arc::clone(&stop), Arc::clone(&done));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                drop(black_box(vec![0u8; 32]));
                done.fetch_add(1, Ordering::Relaxed);
            }
        })
    };
    let guard = Guard::start();
    let start = done.load(Ordering::Relaxed);
    while done.load(Ordering::Relaxed) < start + 1000 {
        std::hint::spin_loop();
    }
    let counts = guard.counts();
    drop(guard);
    stop.store(true, Ordering::Relaxed);
    worker.join().expect("worker thread");
    assert!(counts.is_zero(), "{counts:?}");
}

#[test]
fn nested_guards_count_their_own_span() {
    let outer = Guard::start();
    drop(black_box(Box::new(1u32)));
    let inner = Guard::start();
    let inner_counts = inner.counts();
    drop(inner);
    drop(black_box(Box::new(2u32)));
    let outer_counts = outer.counts();
    drop(outer);
    assert!(inner_counts.is_zero());
    assert_eq!(
        outer_counts,
        Counts {
            allocations: 2,
            deallocations: 2
        }
    );
}
