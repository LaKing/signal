//! AC3 of M2.7-5 in the engine: the device is lost while the null driver
//! runs `lab.gpu_partials` in real time. The run goes on to its end; every
//! due block from the first `device_lost` fault on is a `late_result` fault
//! (silence, logged and counted, never a mute or a delivery). Its own test
//! binary, because the loss is the process's service's, for good. On the
//! GPU, or where none is usable on the noop device.

#![cfg(feature = "lab")]

mod common;

use std::time::Duration;

use signal_core::LossCause;
use signal_engine::{
    BoundaryLine, BoundaryOutcome, ControlSink, FaultReason, LogLine, RealtimeConfig,
};
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
    let registry = common::registry();
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
    assert!(
        after
            .iter()
            .all(|o| matches!(o, Some(None | Some(LossCause::DeviceLost)))),
        "a delivery or another cause after the loss: {after:?}"
    );
}
