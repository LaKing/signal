//! AC3 and AC8 of M2.6-4: in real time the engine never waits for the GPU.
//! The null driver runs `lab.gpu_partials` while the service's test hook
//! holds every result back for 60 s: each due result is a `late_result`
//! fault (silence), logged and counted, and the run finishes in a fraction
//! of the hold, so neither the callback nor a worker waited. Its own test
//! binary, because the hook belongs to the process's service. On the GPU,
//! or where none is usable (AC10) on the noop device, which moves jobs the
//! same way.

#![cfg(feature = "lab")]

mod common;

use std::time::{Duration, Instant};

use signal_engine::{
    ControlSink, Driver, FaultReason, LogLine, NullDriver, RealtimeConfig, realtime,
};
use signal_lab::gpu_partials::{BLOCK_FRAMES, LATENCY_BLOCKS};

/// The `late_result` faults the controller hands out, by frame.
#[derive(Default)]
struct Faults(Vec<u64>);

impl ControlSink for Faults {
    fn log(&mut self, line: LogLine<'_>) {
        if let LogLine::Fault {
            frame,
            reason: FaultReason::LateResult,
            address,
            ..
        } = line
        {
            assert_eq!(address.to_string(), "/bank");
            self.0.push(frame.0);
        }
    }
}

#[test]
fn a_late_gpu_result_is_a_counted_logged_fault_and_nothing_waits() {
    let gpu = common::device();
    let hold = Duration::from_secs(60);
    gpu.set_completion_delay(hold);

    let registry = common::registry();
    let mut config = RealtimeConfig::with_buffer(96_000, 42, 128);
    config.engine.threads = 4;
    let (runner, mut controller) = realtime(&common::plan(&registry), &registry, config).unwrap();
    let mut driver = NullDriver::new(96_000, 2, 7);
    driver.start(runner).unwrap();
    let mut faults = Faults::default();
    let frames = 96_000; // one second of audio: 375 blocks of the bank
    let started = Instant::now();
    driver.run(frames, || controller.poll(&mut faults)).unwrap();
    let elapsed = started.elapsed();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut faults);
    gpu.set_completion_delay(Duration::ZERO);

    // Nothing waited for the held results.
    assert!(elapsed < hold / 6, "the run took {elapsed:?}");
    // Every due block (from block L on) missed its result: one fault each,
    // at the block's first frame, counted by the engine and the runner.
    let block = BLOCK_FRAMES as u64;
    let due: Vec<u64> = (u64::from(LATENCY_BLOCKS)..frames.div_ceil(block))
        .map(|m| m * block)
        .collect();
    assert_eq!(faults.0, due);
    assert_eq!(runner.status().late_results, due.len() as u64);
    // The output is silence, and the instance was not muted.
    assert!(driver.output().iter().all(|&x| x == 0.0));
    eprintln!(
        "{} late results in {elapsed:?} for {frames} frames",
        due.len()
    );
}
