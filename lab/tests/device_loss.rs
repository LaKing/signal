//! AC3 of M2.7-5 in the engine: the device is lost while the null driver
//! runs `lab.gpu_partials` in real time. With replacement held (as before
//! M2.7-2, lost for good) the run goes on to its end and every due block
//! from the first `device_lost` fault on is a `late_result` fault with that
//! cause (silence, logged and counted, never a mute or a delivery). On the
//! GPU, or where none is usable on the noop device.

#![cfg(feature = "lab")]

mod common;

use std::time::Duration;

use signal_core::{
    Due, Inputs, LossCause, Outputs, PrepareContext, RngStream, StateOutputPrimitive,
};
use signal_engine::{
    BoundaryLine, BoundaryOutcome, ControlSink, FaultReason, LogLine, RealtimeConfig,
};
use signal_lab::gpu_partials::{BLOCK_FRAMES, GpuPartials, LATENCY_BLOCKS};
use signal_output::{Driver, NullDriver, realtime};

/// The due outcomes, in order: `Some(cause)` missed, `None` delivered.
#[derive(Default)]
struct Outcomes {
    order: Vec<Option<Option<LossCause>>>,
    faults: u64,
    others: u64,
}

impl ControlSink for Outcomes {
    fn log(&mut self, line: LogLine<'_>) {
        match line {
            LogLine::Fault {
                reason: FaultReason::LateResult,
                ..
            } => self.faults += 1,
            LogLine::Fault { .. } => self.others += 1,
            _ => {}
        }
    }
    fn boundary(&mut self, line: BoundaryLine<'_>) {
        self.order.push(match line.outcome {
            BoundaryOutcome::Missed { cause } => Some(cause),
            BoundaryOutcome::Delivered { .. } => None,
        });
    }
}

#[test]
fn a_lost_device_is_a_fault_per_block_and_the_engine_keeps_running() {
    let gpu = common::device();
    gpu.pool.hold_replacement(true);
    let registry = gpu.registry();
    let config = RealtimeConfig::with_buffer(96_000, 42, 128);
    let (runner, mut controller) = realtime(&common::plan(&registry), &registry, config).unwrap();
    let mut driver = NullDriver::new(96_000, 2, 7);
    driver.start(runner).unwrap();
    let mut outcomes = Outcomes::default();
    let frames = 96_000;
    let mut callbacks = 0;
    driver
        .run(frames, || {
            controller.poll(&mut outcomes);
            callbacks += 1;
            if callbacks == 40 {
                gpu.lose_device();
            }
            if callbacks >= 40 {
                // Give the GPU thread time to hand back the lost jobs, as a
                // device's clock would.
                std::thread::sleep(Duration::from_micros(500));
            }
        })
        .unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut outcomes);

    assert_eq!(runner.status().next_frame.0, frames, "the run went on");
    assert_eq!(outcomes.others, 0, "nothing was muted");
    assert_eq!(runner.status().late_results, outcomes.faults);
    let first = outcomes
        .order
        .iter()
        .position(|o| *o == Some(Some(LossCause::DeviceLost)))
        .expect("a fault with the cause device_lost");
    let after = &outcomes.order[first..];
    eprintln!(
        "{} due blocks, the first device_lost at {first}; {} after it",
        outcomes.order.len(),
        after.len()
    );
    // auditor-2-003, auditor-2-009: every miss after the loss carries its
    // cause, also once the job pool is spent on the lost device.
    assert!(
        after
            .iter()
            .all(|o| *o == Some(Some(LossCause::DeviceLost))),
        "a delivery or another cause after the loss: {after:?}"
    );
}

/// auditor-1-008: the draining race. The device is lost while its service
/// still holds the module's jobs (they are published late), so the due
/// block's result is neither back nor known lost: a real-time miss then
/// carries the cause `device_lost` all the same, every time it is asked.
#[test]
fn a_miss_while_the_lost_service_drains_is_device_lost() {
    let gpu = common::noop(1);
    gpu.pool.hold_replacement(true);
    gpu.set_completion_delay(Duration::from_secs(2));
    let mut module = GpuPartials::default();
    module.prepare(&PrepareContext {
        sample_rate: 96_000.0,
        max_frames: 256,
        rng: RngStream::derive(1, &"/bank".parse().unwrap()),
        lent: Some(gpu.lent()),
    });
    // The first L blocks have no due result; each submits one job.
    for _ in 0..LATENCY_BLOCKS as usize * BLOCK_FRAMES {
        assert_eq!(module.due(), Due::Ready);
        module.emit(&mut Outputs::new(1, &mut [0.0], &mut []));
        module.absorb(&Inputs::new(1, &[], &[440.0, 0.0, 0.0, 0.5], &[], &[]));
    }
    assert_eq!(module.due(), Due::Pending, "block 0 is still held");
    assert_eq!(module.lost_cause(), None, "not lost yet");
    gpu.lose_device();
    for _ in 0..3 {
        assert_eq!(module.due(), Due::Pending, "still held by the service");
        assert_eq!(module.lost_cause(), Some(LossCause::DeviceLost));
    }
}
