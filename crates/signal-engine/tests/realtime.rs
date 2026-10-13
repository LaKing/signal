//! The real-time runner, its controller and the null driver (M2-3): commands
//! apply at their frame and the output equals the offline render of what was
//! applied; queue overflow is counted and never changes the audio (AC4); the
//! f32 clamp, channel mapping, default stamp, late commands, returned plans
//! and callback sizes (AC6). The no-allocation check is in
//! `realtime_no_allocation.rs`; the kick and `signal replay` comparisons are
//! in `signal-cli/tests/realtime.rs`.

mod common;

use common::{addr, conn, inst, inst_with, plan, port_addr, registry, scaled};
use signal_core::{EventPayload, Frame};
use signal_engine::{
    Command, ControlError, ControlSink, Engine, EngineConfig, ExternalEvent, LogLine, Outcome,
    ParameterValue, RealtimeConfig, Report, SentCommand, Stamp,
};
use signal_graph::ExecutionPlan;
use signal_output::{
    CallbackSizes, Controller, Driver, MAX_CALLBACK_FRAMES, Meter, NullDriver, Runner, realtime,
    render_with,
};

/// 1 kHz: the fade length F is 5 frames.
const RATE: u32 = 1_000;
const SEED: u64 = 42;
const FRAMES: u64 = 3_000;

/// Version 0: a tone and a constant into a sink with `gain` 0.5.
fn v0() -> ExecutionPlan {
    let mut plan = plan(
        vec![
            inst_with("osc", "test.osc", &[("freq", 110.0)]),
            inst("dc", "test.dc"),
            inst_with("out", "test.gained_recorder", &[("gain", 0.5)]),
        ],
        vec![conn("osc.out", "out.l"), conn("dc.out", "out.r")],
    );
    plan.version = 0;
    plan
}

/// Version 1: a second tone added.
fn v1() -> ExecutionPlan {
    let mut plan = plan(
        vec![
            inst_with("osc", "test.osc", &[("freq", 110.0)]),
            inst_with("osc2", "test.osc", &[("freq", 230.0)]),
            inst("dc", "test.dc"),
            inst_with("out", "test.gained_recorder", &[("gain", 0.5)]),
        ],
        vec![
            conn("osc.out", "out.l"),
            scaled("osc2.out", "out.l", 0.5),
            conn("dc.out", "out.r"),
        ],
    );
    plan.version = 1;
    plan
}

fn config() -> RealtimeConfig {
    RealtimeConfig::new(RATE, SEED)
}

/// Everything `poll` hands out.
#[derive(Default)]
struct Seen {
    reports: Vec<(Report, SentCommand)>,
    lines: Vec<String>,
    meters: Vec<Meter>,
    parameters: Vec<(String, ParameterValue)>,
    frontier: Frame,
}

impl ControlSink for Seen {
    fn report(&mut self, report: &Report, command: &SentCommand) {
        self.reports.push((*report, command.clone()));
    }
    fn log(&mut self, line: LogLine<'_>) {
        self.lines.push(serde_json::to_string(&line).unwrap());
    }
    fn parameter(&mut self, port: &signal_core::PortAddress, value: ParameterValue) {
        self.parameters.push((port.to_string(), value));
    }
    fn frontier(&mut self, next_frame: Frame) {
        self.frontier = next_frame;
    }
}

/// `poll`, with the audio adapter's meters into `seen` too.
fn poll(controller: &mut Controller, seen: &mut Seen) {
    let mut meters = Vec::new();
    controller.poll_with(seen, |meter| meters.push(*meter));
    seen.meters.extend(meters);
}

/// The scripted commands, all sent before the first frame: sets, a ramp, a
/// swap that adds `/osc2`, a set to it, its removal (suspend, swap back at
/// the fade's end) and a suspend and resume of `/dc`.
fn script(controller: &mut Controller) {
    let at = |frame| Stamp::At(Frame(frame));
    let set = |value| EventPayload::Set(value);
    controller
        .send_event(port_addr("/osc.freq"), set(220.0), at(100))
        .unwrap();
    controller
        .send_event(port_addr("/out.gain"), common::ramp(0.25, 400), at(200))
        .unwrap();
    controller.swap(&v1(), at(1_000)).unwrap();
    // Sent after the removal swap below in time, before it in the queue.
    controller.swap(&removed(), at(1_505)).unwrap();
    controller
        .send_event(port_addr("/osc2.freq"), set(330.0), at(1_000))
        .unwrap();
    controller.send_suspend(addr("/osc2"), at(1_500)).unwrap();
    controller.send_suspend(addr("/dc"), at(2_000)).unwrap();
    controller.send_resume(addr("/dc"), at(2_300)).unwrap();
}

/// Version 2: `/osc2` removed again.
fn removed() -> ExecutionPlan {
    let mut plan = v0();
    plan.version = 2;
    plan
}

/// Runs `frames` frames through a synchronous null driver with callback
/// sizes from `seed`, calling `between` with the controller before each
/// callback; returns the output, the reports and the runner.
fn run(
    config: RealtimeConfig,
    channels: usize,
    seed: u64,
    frames: u64,
    mut between: impl FnMut(&mut Controller, &mut Seen),
) -> (Vec<f32>, Seen, Controller, Runner) {
    let (runner, mut controller) = realtime(&v0(), &registry(), config).unwrap();
    let mut driver = NullDriver::new(RATE, channels, seed);
    driver.start(runner).unwrap();
    let mut seen = Seen::default();
    driver
        .run(frames, || between(&mut controller, &mut seen))
        .unwrap();
    let runner = driver.stop().unwrap().unwrap();
    poll(&mut controller, &mut seen);
    (driver.take_output(), seen, controller, runner)
}

/// The offline render of what the runner reported it applied, at the frames
/// it reported, as f32: what the runner must have produced.
fn offline(reports: &[(Report, SentCommand)], frames: u64) -> Vec<f32> {
    let plans = [v0(), v1(), removed()];
    let mut applied: Vec<&(Report, SentCommand)> = reports
        .iter()
        .filter(|(r, _)| r.outcome != Outcome::Refused)
        .collect();
    applied.sort_by_key(|(r, command)| {
        let class = match command {
            SentCommand::Swap { .. } => 0,
            SentCommand::Suspend(_) | SentCommand::Resume(_) => 1,
            SentCommand::Event(_) => 2,
        };
        (r.frame, class, r.id)
    });
    let config = EngineConfig::new(RATE, SEED);
    let mut engine = Engine::new(&plans[0], &registry(), config, Vec::new()).unwrap();
    let mut next = 0;
    let rendered = render_with(&mut engine, frames, |engine| {
        engine.reclaim_externals().for_each(drop);
        engine.reclaim_edits().for_each(drop);
        let now = engine.next_frame();
        while let Some((report, command)) = applied.get(next).filter(|(r, _)| r.frame == now) {
            next += 1;
            match command {
                SentCommand::Swap { plan_version } => {
                    let mut swap =
                        engine.prepare_swap(&plans[*plan_version as usize], &registry())?;
                    engine.commit_swap(&mut swap, now)?;
                }
                SentCommand::Event(event) => engine
                    .push_external(ExternalEvent {
                        frame: report.frame,
                        ..event.clone()
                    })
                    .unwrap(),
                SentCommand::Suspend(a) => engine.push_suspend(a.clone(), report.frame).unwrap(),
                SentCommand::Resume(a) => engine.push_resume(a.clone(), report.frame).unwrap(),
            }
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(next, applied.len(), "every applied command was replayed");
    rendered.samples.iter().map(|&x| x as f32).collect()
}

/// M2.6-1 C6: at every block capacity the Runner splits each callback into
/// blocks at swap and command frames, and its output equals the per-frame
/// offline render of what it applied, bit for bit as f32.
#[test]
fn runner_blocks_at_every_capacity_equal_the_offline_render() {
    for capacity in [1, 7, 64, 256] {
        let config = RealtimeConfig {
            engine: EngineConfig {
                block_capacity: capacity,
                ..EngineConfig::new(RATE, SEED)
            },
            ..config()
        };
        let mut sent = false;
        let (output, seen, _, runner) = run(config, 2, 11, FRAMES, |controller, seen| {
            poll(controller, seen);
            if !sent {
                script(controller);
                sent = true;
            }
        });
        assert_eq!(seen.reports.len(), 8, "capacity {capacity}");
        assert_eq!(
            output,
            offline(&seen.reports, FRAMES),
            "capacity {capacity}"
        );
        assert_eq!(runner.status().next_frame, Frame(FRAMES));
    }
}

/// AC6: commands, swaps and the removal apply at their frames, and the output
/// equals the offline render of what was applied, bit for bit as f32.
#[test]
fn runner_output_equals_the_offline_render_of_what_it_applied() {
    let mut sent = false;
    let (output, seen, controller, runner) = run(config(), 2, 7, FRAMES, |controller, seen| {
        poll(controller, seen);
        if !sent {
            script(controller);
            sent = true;
        }
    });
    assert_eq!(output.len() as u64, FRAMES * 2);
    assert_eq!(seen.reports.len(), 8, "every command was reported");
    assert!(seen.reports.iter().all(|(r, _)| !r.is_late()));
    let frames: Vec<(u64, Outcome)> = seen
        .reports
        .iter()
        .map(|(r, _)| (r.frame.0, r.outcome))
        .collect();
    assert!(frames.contains(&(1_000, Outcome::Committed { version: 1 })));
    assert!(frames.contains(&(1_505, Outcome::Committed { version: 2 })));
    assert_eq!(output, offline(&seen.reports, FRAMES));

    let status = runner.status();
    assert_eq!(status.next_frame, Frame(FRAMES));
    assert_eq!(status.version, 2);
    assert_eq!(runner.clipped(), 0);
    assert_eq!(status.input_discards, Default::default());
    assert_eq!(controller.in_flight(), 0, "every returned item was dropped");
    // The log reaches the control thread with addresses, read with the plan
    // that wrote each record.
    assert!(seen.lines.contains(
        &r#"{"type":"event","frame":1000,"target":"/osc2.freq","payload":{"set":330.0},"source":{"external":{"sequence":5}}}"#
            .to_owned()
    ));
    assert!(
        seen.lines
            .contains(&r#"{"type":"suspend","frame":1500,"address":"/osc2"}"#.to_owned())
    );
    assert!(
        seen.lines
            .contains(&r#"{"type":"swap","frame":1505,"version":2,"plan_version":2}"#.to_owned())
    );
    // One meter per sink channel per callback, covering every frame.
    let metered: u64 = seen
        .meters
        .iter()
        .filter(|m| m.channel == 0)
        .map(|m| m.frames)
        .sum();
    assert_eq!(metered, FRAMES);
}

/// AC6: an event stamped for a frame that has run applies on the next frame,
/// and its report says so (requested 5, effective the next frame to run).
/// The run is long enough that callbacks follow the send.
#[test]
fn a_late_command_applies_on_the_next_frame_and_is_reported() {
    let mut sent = None;
    let (_, seen, controller, _) = run(config(), 2, 3, 40_000, |controller, seen| {
        poll(controller, seen);
        let now = controller.status().next_frame;
        if sent.is_none() && now >= Frame(200) {
            let id = controller
                .send_event(
                    port_addr("/osc.freq"),
                    EventPayload::Set(220.0),
                    Stamp::At(Frame(5)),
                )
                .unwrap();
            sent = Some((id, now));
        }
    });
    let (id, now) = sent.unwrap();
    let (report, _) = seen.reports.iter().find(|(r, _)| r.id == id).unwrap();
    assert_eq!(report.requested, Frame(5));
    assert_eq!(report.frame, now);
    assert!(report.is_late());
    assert_eq!(controller.status().late, 1);
    let line = format!(
        r#"{{"type":"event","frame":{},"target":"/osc.freq","payload":{{"set":220.0}},"source":{{"external":{{"sequence":{id}}}}}}}"#,
        now.0
    );
    assert!(seen.lines.contains(&line), "{line}");
}

/// AC6: the default stamp is the reported frame plus two buffers.
#[test]
fn the_default_stamp_is_the_current_frame_plus_the_latency() {
    assert_eq!(
        config().latency_frames,
        2 * RealtimeConfig::DEFAULT_BUFFER_FRAMES
    );
    let mut sent = None;
    let (_, seen, _, _) = run(config(), 2, 5, 40_000, |controller, seen| {
        poll(controller, seen);
        let now = controller.status().next_frame;
        if sent.is_none() && now >= Frame(300) {
            assert_eq!(controller.default_frame(), Frame(now.0 + 1_024));
            let id = controller
                .send_event(
                    port_addr("/osc.freq"),
                    EventPayload::Set(220.0),
                    Stamp::Default,
                )
                .unwrap();
            sent = Some((id, now));
        }
    });
    let (id, now) = sent.unwrap();
    let (report, _) = seen.reports.iter().find(|(r, _)| r.id == id).unwrap();
    assert_eq!(report.frame, Frame(now.0 + 1_024));
    assert!(!report.is_late());
}

/// AC4: with log, meter and status queues of one entry, never drained while
/// the run lasts, records are dropped and counted, nothing blocks, and the
/// audio is unchanged.
#[test]
fn lossy_queue_overflow_is_counted_and_does_not_change_the_audio() {
    let script_once = || {
        let mut sent = false;
        move |controller: &mut Controller, _: &mut Seen| {
            if !sent {
                script(controller);
                sent = true;
            }
        }
    };
    let (reference, ..) = run(config(), 2, 11, FRAMES, script_once());
    let tiny = RealtimeConfig {
        log: 1,
        meters: 1,
        parameters: 1,
        status: 1,
        ..config()
    };
    let (output, seen, _, runner) = run(tiny, 2, 11, FRAMES, script_once());
    assert_eq!(output, reference);
    let status = runner.status();
    assert!(status.log_overflows > 0, "{status:?}");
    assert!(runner.meter_overflows() > 0, "{status:?}");
    assert!(status.parameter_overflows > 0, "{status:?}");
    assert!(status.status_overflows > 0, "{status:?}");
    // Reports travel on the lossless return queue.
    assert_eq!(seen.reports.len(), 8);
}

/// M2.5-2: each callback reports parameters' base and effective values, read
/// on the control thread with the plan that wrote them: a set and a ramp
/// show in the base, and a plan's parameters come and go with its swaps.
#[test]
fn parameter_values_are_reported_with_their_plan() {
    let mut sent = false;
    let (_, seen, ..) = run(config(), 2, 11, FRAMES, |controller, seen| {
        if !sent {
            script(controller);
            sent = true;
        }
        poll(controller, seen);
    });
    let values = |port: &str| -> Vec<ParameterValue> {
        seen.parameters
            .iter()
            .filter(|(p, _)| p == port)
            .map(|(_, v)| *v)
            .collect()
    };
    let freq = values("/osc.freq");
    assert_eq!(freq.first().unwrap().base, 110.0);
    assert_eq!(
        freq.last(),
        Some(&ParameterValue {
            base: 220.0,
            effective: 220.0
        })
    );
    let gain = values("/out.gain");
    assert_eq!(gain.first().unwrap().base, 0.5);
    assert_eq!(gain.last().unwrap().base, 0.25, "the ramp's end");
    assert_eq!(
        values("/osc2.freq").last().unwrap().base,
        330.0,
        "while v1 ran"
    );
    // v2 (without /osc2) runs last: the last callback reports v0's ports.
    let (last, _) = seen.parameters.last().unwrap();
    let tail: Vec<&str> = seen.parameters[seen.parameters.len() - 3..]
        .iter()
        .map(|(p, _)| p.as_str())
        .collect();
    assert!(!tail.contains(&"/osc2.freq"), "{tail:?} ({last})");
}

/// AC4: a full command queue refuses the send without blocking, hands the
/// command back and counts it; the audio is that of the commands sent.
#[test]
fn a_full_command_queue_hands_the_command_back_and_counts_it() {
    let small = RealtimeConfig {
        commands: 2,
        ..config()
    };
    let set = |c: &mut Controller, value, frame| {
        c.send_event(
            port_addr("/osc.freq"),
            EventPayload::Set(value),
            Stamp::At(Frame(frame)),
        )
    };
    let mut refused = None;
    let mut first = true;
    let (output, seen, controller, _) = run(small, 2, 13, 1_000, |controller, seen| {
        poll(controller, seen);
        if first {
            first = false;
            set(controller, 220.0, 100).unwrap();
            set(controller, 330.0, 200).unwrap();
            refused = Some(set(controller, 440.0, 300));
        }
    });
    match refused.unwrap() {
        Err(ControlError::Full(Command::Event(event))) => {
            assert_eq!(event.payload, EventPayload::Set(440.0));
        }
        other => panic!("expected the event back, got {other:?}"),
    }
    assert_eq!(controller.command_overflows(), 1);
    assert_eq!(seen.reports.len(), 2);
    assert_eq!(output, offline(&seen.reports, 1_000));
}

/// AC6: f64 to f32 clamps to [−1, 1] and counts each clipped sample; sink
/// channels go to the same device channels, extra device channels are 0,
/// sink channels beyond the device's are not played.
#[test]
fn samples_are_clamped_counted_and_mapped_to_device_channels() {
    let loud = || {
        let mut plan = plan(
            vec![
                inst_with("hi", "test.dc", &[("value", 0.75)]),
                inst_with("lo", "test.dc", &[("value", -0.25)]),
                inst_with("out", "test.gained_recorder", &[("gain", 2.0)]),
            ],
            vec![conn("hi.out", "out.l"), conn("lo.out", "out.r")],
        );
        plan.version = 0;
        plan
    };
    for channels in [1, 2, 4] {
        let (runner, _controller) = realtime(&loud(), &registry(), config()).unwrap();
        let mut driver = NullDriver::new(RATE, channels, 17);
        driver.start(runner).unwrap();
        driver.run(100, || {}).unwrap();
        let runner = driver.stop().unwrap().unwrap();
        let expected: Vec<f32> = [1.0, -0.5, 0.0, 0.0][..channels].to_vec();
        for frame in driver.output().chunks(channels) {
            assert_eq!(frame, expected.as_slice(), "{channels} channels");
        }
        // Only the left channel (0.75 · 2) clips, whether or not it is played.
        assert_eq!(runner.clipped(), 100);
    }
}

/// AC6: a committed swap's box, now holding the old plan, comes back and is
/// dropped on the control thread.
#[test]
fn a_swap_returns_the_old_plan_to_the_control_thread() {
    let (runner, mut controller) = realtime(&v0(), &registry(), config()).unwrap();
    let mut driver = NullDriver::new(RATE, 2, 19);
    driver.start(runner).unwrap();
    controller.swap(&v1(), Stamp::At(Frame(10))).unwrap();
    assert_eq!(controller.in_flight(), 1);
    driver.run(5, || {}).unwrap();
    let mut seen = Seen::default();
    poll(&mut controller, &mut seen);
    assert!(seen.reports.is_empty(), "not committed yet");
    assert_eq!(controller.in_flight(), 1);
    driver.run(20, || {}).unwrap();
    poll(&mut controller, &mut seen);
    let (report, command) = &seen.reports[0];
    assert_eq!(report.outcome, Outcome::Committed { version: 1 });
    assert_eq!(report.frame, Frame(10));
    assert_eq!(command, &SentCommand::Swap { plan_version: 1 });
    assert_eq!(controller.in_flight(), 0, "the replaced plan came back");
    // A plan that changes the audio sink is refused before it is sent.
    let mut other = plan(
        vec![inst("dc", "test.dc"), inst("rec", "test.gained_recorder")],
        vec![conn("dc.out", "rec.l")],
    );
    other.version = 2;
    assert!(matches!(
        controller.swap(&other, Stamp::Default),
        Err(ControlError::Sink(_))
    ));
    driver.stop().unwrap();
}

/// A swap prepared by one Controller is refused by another before anything
/// is queued, so it cannot desynchronise it; its own Controller sends it, and
/// refuses it again once another swap has been sent since it was prepared.
#[test]
fn a_prepared_swap_is_sent_only_by_its_own_controller() {
    let (_runner_a, mut a) = realtime(&v0(), &registry(), config()).unwrap();
    let (_runner_b, mut b) = realtime(&v0(), &registry(), config()).unwrap();
    let ready = a.prepare_swap(&v1()).unwrap();
    assert!(matches!(
        b.send_swap(ready, Stamp::Default),
        Err(ControlError::Engine(signal_engine::EngineError::StaleSwap))
    ));
    assert_eq!(b.in_flight(), 0, "nothing was queued");
    assert!(!b.is_desynchronised());

    let first = a.prepare_swap(&v1()).unwrap();
    let second = a.prepare_swap(&v1()).unwrap();
    a.send_swap(first, Stamp::Default).unwrap();
    assert!(matches!(
        a.send_swap(second, Stamp::Default),
        Err(ControlError::Engine(signal_engine::EngineError::StaleSwap))
    ));
    assert_eq!(a.in_flight(), 1);
}

/// AC6: callback sizes are 1 to 4096, start with 1, a prime and a power of
/// two, and depend on the seed.
#[test]
fn callback_sizes_cover_one_primes_and_powers_of_two() {
    let primes = [3, 7, 31, 127, 509, 1021, 2039, 4093];
    for seed in [1, 2, 3] {
        let sizes: Vec<usize> = CallbackSizes::new(seed).take(100_000).collect();
        assert!(
            sizes
                .iter()
                .all(|&n| (1..=MAX_CALLBACK_FRAMES).contains(&n))
        );
        assert_eq!(sizes[0], 1);
        assert!(primes.contains(&sizes[1]), "{}", sizes[1]);
        assert!(sizes[2].is_power_of_two() && sizes[2] >= 2, "{}", sizes[2]);
        assert!(sizes.contains(&MAX_CALLBACK_FRAMES));
        assert!(
            sizes
                .iter()
                .any(|&n| !n.is_power_of_two() && !primes.contains(&n))
        );
    }
    let a: Vec<usize> = CallbackSizes::new(1).take(50).collect();
    let b: Vec<usize> = CallbackSizes::new(2).take(50).collect();
    assert_ne!(a, b);
}

/// Dropping a running threaded null driver stops and joins its thread: the
/// runner reports no further frame afterwards.
#[test]
fn dropping_a_threaded_null_driver_stops_its_thread() {
    let (runner, mut controller) = realtime(&v0(), &registry(), config()).unwrap();
    let mut driver = NullDriver::new(RATE, 2, 31).threaded(false);
    driver.start(runner).unwrap();
    while controller.status().next_frame < Frame(10_000) {
        controller.poll(&mut ());
        std::thread::yield_now();
    }
    drop(driver);
    controller.poll(&mut ());
    let stopped_at = controller.status().next_frame;
    std::thread::sleep(std::time::Duration::from_millis(100));
    controller.poll(&mut ());
    assert_eq!(controller.status().next_frame, stopped_at);
}

/// AC6: the threaded null driver runs the Runner on its own thread until
/// stopped, then hands it back with everything it rendered.
#[test]
fn the_threaded_null_driver_hands_the_runner_back() {
    let (runner, mut controller) = realtime(&v0(), &registry(), config()).unwrap();
    let mut driver = NullDriver::new(RATE, 2, 23).threaded(false);
    driver.start(runner).unwrap();
    while controller.status().next_frame < Frame(10_000) {
        controller.poll(&mut ());
        std::thread::yield_now();
    }
    let runner = driver.stop().unwrap().expect("the runner comes back");
    assert!(driver.stop().unwrap().is_none());
    assert_eq!(driver.output().len() as u64, runner.next_frame().0 * 2);
}
