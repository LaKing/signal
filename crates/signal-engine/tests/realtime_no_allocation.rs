//! V11 (M2-3 AC3): `Runner::process` allocates and frees nothing, measured
//! with the counting allocator of `signal-contract` (ADR 0017) on the audio
//! thread only: its guard counts the thread that started it, and a guard is
//! started around each `process` call on the thread that runs the Runner.

signal_contract::install_counting_allocator!();

mod common;

use std::thread;
use std::time::Duration;

use common::{addr, conn, inst, inst_with, plan, port_addr, registry, scaled};
use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_core::EventPayload;
use signal_engine::{
    CallbackSizes, ControlSink, Outcome, RealtimeConfig, Report, SentCommand, Stamp, realtime,
};
use signal_graph::ExecutionPlan;

const RATE: u32 = 1_000;
const CALLBACKS: u64 = 12_000;
/// Callback sizes are drawn from the null driver's stream and capped here to
/// keep the test short; the frame count does not change what can allocate.
const MOST_FRAMES: usize = 256;
const CHANNELS: usize = 2;

fn version(n: u64) -> ExecutionPlan {
    let mut instances = vec![
        inst_with("osc", "test.osc", &[("freq", 110.0)]),
        inst("dc", "test.dc"),
        inst_with("out", "test.gained_recorder", &[("gain", 0.5)]),
    ];
    let mut connections = vec![conn("osc.out", "out.l"), conn("dc.out", "out.r")];
    if n % 2 == 1 {
        instances.push(inst_with("osc2", "test.osc", &[("freq", 230.0)]));
        connections.push(scaled("osc2.out", "out.l", 0.5));
    }
    let mut plan = plan(instances, connections);
    plan.version = n;
    plan
}

/// Counts what the reports say happened.
#[derive(Default)]
struct Tally {
    events: u64,
    edits: u64,
    swaps: u64,
}

impl ControlSink for Tally {
    fn report(&mut self, report: &Report, command: &SentCommand) {
        match (report.outcome, command) {
            (Outcome::Committed { .. }, _) => self.swaps += 1,
            (Outcome::Scheduled, SentCommand::Event(_)) => self.events += 1,
            (Outcome::Scheduled, _) => self.edits += 1,
            _ => {}
        }
    }
}

/// Over 12 000 callbacks the control thread swaps plans, sends bursts of
/// events and suspends and resumes, and drains only every fourth step, so
/// that the 4-record log queue and 2-meter queue overflow. The audio thread
/// counts every heap call inside `Runner::process`: none.
#[test]
fn runner_process_allocates_and_frees_nothing() {
    assert!(alloc_counter::is_installed());
    let config = RealtimeConfig {
        log: 4,
        meters: 2,
        ..RealtimeConfig::with_buffer(RATE, 42, 64)
    };
    let (mut runner, mut controller) = realtime(&version(0), &registry(), config).unwrap();

    let audio = thread::spawn(move || {
        let mut sizes = CallbackSizes::new(9);
        let mut out = vec![0.0f32; MOST_FRAMES * CHANNELS];
        let mut total = Counts::default();
        for _ in 0..CALLBACKS {
            let frames = sizes.next_size().min(MOST_FRAMES);
            let buffer = &mut out[..frames * CHANNELS];
            let guard = Guard::start();
            runner.process(buffer, CHANNELS);
            let counts = guard.counts();
            drop(guard);
            total.allocations += counts.allocations;
            total.deallocations += counts.deallocations;
        }
        // The guard does count on this thread.
        let guard = Guard::start();
        drop(std::hint::black_box(Box::new(0u8)));
        assert_eq!(guard.counts().allocations, 1);
        (runner, total)
    });

    let plans: Vec<ExecutionPlan> = (1..=200).map(version).collect();
    let mut next_plan = 0;
    let mut tally = Tally::default();
    let mut step = 0u64;
    while !audio.is_finished() {
        step += 1;
        if step.is_multiple_of(4) {
            controller.poll(&mut tally);
        }
        // Refusals (a full queue) are fine: the command comes back here.
        match step % 4 {
            0 => {
                if let Some(plan) = plans.get(next_plan)
                    && controller.swap(plan, Stamp::Default).is_ok()
                {
                    next_plan += 1;
                }
            }
            1 => {
                for k in 0..10 {
                    let value = EventPayload::Set(100.0 + k as f64);
                    let _ = controller.send_event(port_addr("/osc.freq"), value, Stamp::Default);
                }
            }
            2 => {
                let _ = controller.send_suspend(addr("/dc"), Stamp::Default);
            }
            _ => {
                let _ = controller.send_resume(addr("/dc"), Stamp::Default);
            }
        }
        thread::sleep(Duration::from_micros(200));
    }
    let (runner, total) = audio.join().unwrap();
    controller.poll(&mut tally);

    assert_eq!(
        total,
        Counts::default(),
        "heap calls inside Runner::process"
    );
    let status = runner.status();
    assert_eq!(status.callbacks, CALLBACKS);
    assert!(tally.swaps >= 2, "swaps committed: {}", tally.swaps);
    assert!(tally.events > 0 && tally.edits > 0);
    assert!(status.log_overflows > 0, "{status:?}");
    assert!(status.meter_overflows > 0, "{status:?}");
}
