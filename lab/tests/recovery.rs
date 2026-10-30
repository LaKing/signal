//! Regression (auditor-1-001 of M2.6-4): `lab.gpu_partials` recovers after
//! a stretch of late GPU results. While the service's test hook delays every
//! completion, real-time calls (one `due` per block, `miss` when not ready)
//! exhaust the job pool and later blocks cannot even be submitted. Once the
//! delay ends, the late results are discarded and their jobs reused, so the
//! next blocks are submitted, run and delivered again. On the GPU, or where
//! none is usable on the noop device, which moves jobs the same way.

#![cfg(feature = "lab")]

mod common;

use std::thread;
use std::time::{Duration, Instant};

use signal_core::{Due, Inputs, Outputs, PrepareContext, StateOutputPrimitive};
use signal_lab::gpu_partials::{BLOCK_FRAMES, GpuPartials, JOBS, Params};

/// One block as the real-time engine runs it: `due` once per frame (it
/// decides at the block's first frame), `miss` when not ready, then `emit`
/// and `absorb`. Returns what `due` said at the block's first frame.
fn real_time_block(module: &mut GpuPartials) -> Due {
    let p = Params::default();
    let values = [p.freq, p.stretch, p.tilt, p.gain];
    let mut first = Due::Ready;
    for t in 0..BLOCK_FRAMES {
        let due = module.due();
        if t == 0 {
            first = due;
        }
        if due != Due::Ready {
            module.miss();
        }
        let mut out = [f64::NAN];
        module.emit(&mut Outputs::new(1, &mut out, &mut []));
        module.absorb(&Inputs::new(1, &[], &values, &[], &[]));
    }
    first
}

/// How long a wait below may take before the test fails: far beyond
/// anything the job path needs, so a loaded machine only makes it slower.
const BOUND: Duration = Duration::from_secs(60);

/// Waits (bounded by [`BOUND`]) until `done` holds, saying `what` if never.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < BOUND, "{what} within {BOUND:?}");
        thread::sleep(Duration::from_millis(1));
    }
}

/// One block as an exact replay runs it, the GPU given the time a device
/// would give whatever the machine's load: at the block's first frame `due`
/// is asked again while its result is pending, and before the last
/// `absorb`, `settle` while a job given up on still holds the pool (the
/// engine yields and asks again). Neither wait passes a lost result or a
/// pool that never refills: a block never submitted is still reported, and
/// a stale job never returned fails at the bound. Returns what `due` said
/// at the block's first frame.
fn block_given_time(module: &mut GpuPartials) -> Due {
    let p = Params::default();
    let values = [p.freq, p.stretch, p.tilt, p.gain];
    let mut first = Due::Pending;
    wait_until("the due result ready or lost", || {
        first = module.due();
        first != Due::Pending
    });
    for t in 0..BLOCK_FRAMES {
        if module.due() != Due::Ready {
            module.miss();
        }
        let mut out = [f64::NAN];
        module.emit(&mut Outputs::new(1, &mut out, &mut []));
        if t + 1 == BLOCK_FRAMES {
            wait_until("a free job for the block", || {
                module.settle() != Due::Pending
            });
        }
        module.absorb(&Inputs::new(1, &[], &values, &[], &[]));
    }
    first
}

#[test]
fn after_late_results_the_bank_recovers() {
    let gpu = common::device();
    let mut module = GpuPartials::default();
    module.prepare(&PrepareContext {
        sample_rate: 96_000.0,
        max_frames: 256,
        rng: signal_core::RngStream::derive(1, &"/bank".parse().unwrap()),
        lent: Some(gpu.lent()),
        generation: 0,
    });

    // Every result DELAY late; 80 real-time blocks run in far less (a few
    // ms, even loaded): the pool of JOBS (6) jobs fills, and the blocks
    // after it are never submitted. The precondition, that no result
    // arrives while they run (the slow GPU is slower than the blocks), is
    // checked.
    const DELAY: Duration = Duration::from_secs(2);
    gpu.set_completion_delay(DELAY);
    let started = Instant::now();
    let late: Vec<Due> = (0..80).map(|_| real_time_block(&mut module)).collect();
    assert!(started.elapsed() < DELAY, "nothing waited");
    assert!(late[2..].iter().all(|&d| d != Due::Ready), "{late:?}");
    // Exactly the pool's jobs were submitted (each starts once the one
    // before it completed: the GPU thread's pace, not the test's).
    wait_until("the pool's jobs started", || gpu.jobs_run() >= JOBS as u64);
    let ran = gpu.jobs_run();
    assert_eq!(ran, JOBS as u64, "only the pool's jobs were submitted");

    // The delay ends; the held results arrive (all late, so discarded, and
    // their jobs reused) once their delay has passed. Blocks are submitted
    // again, and each one's result is delivered when due: the first two
    // blocks' due ones were never submitted (L = 2), then every block is
    // Ready.
    gpu.set_completion_delay(Duration::ZERO);
    let seen: Vec<Due> = (0..40).map(|_| block_given_time(&mut module)).collect();
    eprintln!(
        "during the delay: {} of 78 due blocks missed; after it: {:?}; jobs run {} -> {}",
        late[2..].iter().filter(|&&d| d != Due::Ready).count(),
        seen,
        ran,
        gpu.jobs_run()
    );
    assert!(
        seen[..2].iter().all(|&d| d != Due::Ready),
        "never submitted during the delay: {seen:?}"
    );
    assert!(
        seen[2..].iter().all(|&d| d == Due::Ready),
        "results are delivered again after the delay: {seen:?}"
    );
    // Every block's job ran: the last one delivered (block 39's, due seq
    // of block 37) means 38 of them at least.
    assert!(gpu.jobs_run() >= ran + 38, "jobs keep running");
}
