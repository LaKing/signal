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
use signal_lab::gpu_partials::{BLOCK_FRAMES, GpuPartials, Params};

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

#[test]
fn after_late_results_the_bank_recovers() {
    let gpu = common::device();
    let mut module = GpuPartials::default();
    module.prepare(&PrepareContext {
        sample_rate: 96_000.0,
        max_frames: 256,
        rng: signal_core::RngStream::derive(1, &"/bank".parse().unwrap()),
        lent: Some(gpu.lent()),
    });

    // Every result 200 ms late; 80 blocks run in far less: the pool of
    // JOBS (6) jobs fills, and the blocks after it are never submitted.
    gpu.set_completion_delay(Duration::from_millis(200));
    let started = Instant::now();
    let late: Vec<Due> = (0..80).map(|_| real_time_block(&mut module)).collect();
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "nothing waited"
    );
    assert!(late[2..].iter().all(|&d| d != Due::Ready), "{late:?}");
    let ran = gpu.jobs_run();

    // The delay ends; the held results arrive (all late, so discarded).
    gpu.set_completion_delay(Duration::ZERO);
    thread::sleep(Duration::from_millis(400));

    // Blocks are submitted again, and each one's result is delivered when
    // due: one more block than L for the first submission to come due, then
    // every block is Ready (the GPU is given the time a device would give).
    let mut seen = Vec::new();
    for _ in 0..40 {
        let due = real_time_block(&mut module);
        seen.push(due);
        thread::sleep(Duration::from_millis(5));
    }
    eprintln!(
        "during the delay: {} of 78 due blocks missed; after it: {:?}; jobs run {} -> {}",
        late[2..].iter().filter(|&&d| d != Due::Ready).count(),
        seen,
        ran,
        gpu.jobs_run()
    );
    assert!(
        seen[3..].iter().all(|&d| d == Due::Ready),
        "results are delivered again after the delay: {seen:?}"
    );
    assert!(gpu.jobs_run() >= ran + 30, "jobs keep running");
}
