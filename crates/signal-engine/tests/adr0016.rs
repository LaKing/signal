//! The items of `docs/adr/0016-execution-contract.md`, "Verification", one test
//! (or a few) per item, in the ADR's order.

mod common;

use common::{
    addr, base_trace, conn, engine, ext, inst, inst_with, output_trace, plan, port_addr, ramp,
    random_order, registry, scaled,
};
use signal_core::{EventPayload, Frame, SplitMix64};
use signal_engine::{Engine, EngineError, LineSource, LogLine, Render, render};
use signal_graph::{CompositeDef, ExecutionPlan, Exports, GraphError, GraphWarning, build_plan};

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// The sources, in delivery order, of the events `target` got in the last frame.
fn delivered(engine: &Engine, target: &str) -> Vec<String> {
    let target = port_addr(target);
    engine
        .frame_log()
        .iter()
        .filter_map(|record| match engine.log_line(record)? {
            LogLine::Event {
                target: t, source, ..
            } if *t == target => Some(match source {
                LineSource::External { sequence } => format!("external {sequence}"),
                LineSource::Port(port) => port.to_string(),
            }),
            _ => None,
        })
        .collect()
}

// --- 1. Schedule independence ------------------------------------------------

/// Signals, events into a parameter, modulation, a feedback loop through a
/// state-output instance, a state-output event receiver, randomness, a
/// disconnected instance, and an order-sensitive fan-in: `rec.alpha` sums ±10¹⁶
/// with small values, so any other summation order would change its bits.
fn rich_plan() -> ExecutionPlan {
    plan(
        vec![
            inst("noise", "test.noise"),
            inst_with("osc", "test.osc", &[("freq", 187.5)]),
            inst_with("gain", "test.gain", &[("gain", 0.5)]),
            inst_with("big", "test.const", &[("value", 1.0)]),
            inst_with("neg", "test.const", &[("value", -1.0)]),
            inst_with("one", "test.const", &[("value", 1.0)]),
            inst_with("s1", "test.setter", &[("value", 0.25)]),
            inst_with("s2", "test.setter", &[("value", 0.75)]),
            inst("probe", "test.probe_add"),
            inst_with("clock", "test.trigger", &[("period", 3.0)]),
            inst("count", "test.counter"),
            inst("later", "test.event_delay"),
            inst("impulse", "test.impulse"),
            inst("loop", "test.gain"),
            inst("delay", "test.unit_delay"),
            inst("lonely", "test.osc"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("noise.out", "rec.zeta"),
            conn("osc.out", "gain.in"),
            conn("gain.out", "rec.zeta"),
            scaled("big.out", "rec.alpha", 1e16),
            scaled("neg.out", "rec.alpha", 1e16),
            conn("one.out", "rec.alpha"),
            conn("s1.out", "probe.value"),
            conn("s2.out", "probe.value"),
            conn("osc.out", "probe.value"),
            conn("probe.out", "rec.alpha"),
            conn("clock.out", "count.in"),
            conn("clock.out", "later.in"),
            conn("count.out", "rec.alpha"),
            conn("later.out", "rec.alpha"),
            conn("impulse.out", "loop.in"),
            scaled("delay.out", "loop.in", 0.5),
            conn("loop.out", "delay.in"),
            conn("loop.out", "rec.zeta"),
        ],
    )
}

fn render_rich(plan: &ExecutionPlan) -> Render {
    let external = vec![
        ext(5, 1, "/probe.value", EventPayload::Set(0.125)),
        ext(7, 2, "/count.in", EventPayload::Trigger),
    ];
    let mut engine = engine(plan, 48_000, external);
    engine.suspend(&addr("/osc"), Frame(50)).unwrap();
    engine.resume(&addr("/osc"), Frame(60)).unwrap();
    render(&mut engine, 300).unwrap()
}

/// Several valid topological orders (random tie-breaks) give bit-identical
/// audio and identical logs. The test-only way to supply an order is to replace
/// the plan's `schedule`, which the engine checks rather than trusts.
#[test]
fn schedule_independence() {
    let plan = rich_plan();
    let reference = render_rich(&plan);
    let mut other_orders = 0;
    for seed in 0..32 {
        let mut shuffled = plan.clone();
        shuffled.schedule = random_order(&plan, seed);
        if shuffled.schedule != plan.schedule {
            other_orders += 1;
        }
        let rendered = render_rich(&shuffled);
        assert_eq!(
            bits(&rendered.samples),
            bits(&reference.samples),
            "seed {seed}"
        );
        assert_eq!(rendered.log, reference.log, "seed {seed}");
    }
    assert!(other_orders >= 16, "only {other_orders} orders differ");
}

#[test]
fn invalid_schedule_is_refused() {
    let plan = rich_plan();
    let config = signal_engine::EngineConfig {
        sample_rate: 48_000,
        seed: 1,
    };
    let refused = |schedule: Vec<_>| {
        let mut broken = plan.clone();
        broken.schedule = schedule;
        Engine::new(&broken, &registry(), config, vec![]).err()
    };
    // `/gain` reads `/osc` in the same frame, so it may not run first.
    let mut swapped = plan.schedule.clone();
    let osc = swapped.iter().position(|a| *a == addr("/osc")).unwrap();
    let gain = swapped.iter().position(|a| *a == addr("/gain")).unwrap();
    assert!(osc < gain);
    swapped.swap(osc, gain);
    assert_eq!(refused(swapped), Some(EngineError::InvalidSchedule));
    let mut missing = plan.schedule.clone();
    missing.pop();
    assert_eq!(refused(missing), Some(EngineError::InvalidSchedule));
}

// --- 2. Cycles and feedback --------------------------------------------------

#[test]
fn cycle_without_state_output_is_rejected() {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![inst("a", "test.gain"), inst("b", "test.gain")],
        connections: vec![conn("a.out", "b.in"), conn("b.out", "a.in")],
        exports: Exports::default(),
    };
    let errors = build_plan(&root, &[], &registry(), 1).unwrap_err();
    assert_eq!(
        errors,
        [GraphError::Cycle {
            instances: vec![addr("/a"), addr("/b")]
        }]
    );
}

/// y[n] = x[n] + 0.5 · y[n − 1] with an impulse x: y[n] = 0.5ⁿ exactly.
#[test]
fn feedback_through_unit_delay_matches_hand_computed_reference() {
    let plan = plan(
        vec![
            inst("impulse", "test.impulse"),
            inst("loop", "test.gain"),
            inst("delay", "test.unit_delay"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("impulse.out", "loop.in"),
            scaled("delay.out", "loop.in", 0.5),
            conn("loop.out", "delay.in"),
            conn("loop.out", "rec.zeta"),
        ],
    );
    let rendered = render(&mut engine(&plan, 48_000, vec![]), 64).unwrap();
    let y: Vec<f64> = rendered.samples.chunks(2).map(|frame| frame[0]).collect();
    let reference: Vec<f64> = (0..64).map(|n| 0.5f64.powi(n)).collect();
    assert_eq!(bits(&y), bits(&reference));
}

// --- 3. A disconnected oscillator advances -----------------------------------

/// Phase increment 187.5 / 48000 = 2⁻⁸, so the analytic phase after n frames,
/// frac(n · 2⁻⁸), is exact.
#[test]
fn disconnected_oscillator_advances_analytically() {
    for connected in [true, false] {
        let connections = if connected {
            vec![conn("osc.out", "rec.zeta")]
        } else {
            vec![]
        };
        let plan = plan(
            vec![
                inst_with("osc", "test.osc", &[("freq", 187.5)]),
                inst("rec", "test.recorder"),
            ],
            connections,
        );
        let unreached = GraphWarning::NoSinkReachable {
            instance: addr("/osc"),
        };
        assert_eq!(plan.warnings.contains(&unreached), !connected);
        let phases = output_trace(&mut engine(&plan, 48_000, vec![]), 1000, "/osc.out");
        let analytic: Vec<f64> = (0..1000).map(|n| (n % 256) as f64 / 256.0).collect();
        assert_eq!(bits(&phases), bits(&analytic), "connected: {connected}");
    }
}

// --- 4. Event ordering -------------------------------------------------------

/// Two sources send conflicting `set`s in the same frame: the later in
/// canonical order (by source port address) wins, so renaming a source flips it.
#[test]
fn conflicting_sets_follow_canonical_order_and_renaming_flips_them() {
    let base_after_one_frame = |first: &str| {
        let plan = plan(
            vec![
                inst_with(first, "test.setter", &[("value", 0.25)]),
                inst_with("b", "test.setter", &[("value", 0.75)]),
                inst("p", "test.probe_add"),
            ],
            vec![
                conn(&format!("{first}.out"), "p.value"),
                conn("b.out", "p.value"),
            ],
        );
        let mut engine = engine(&plan, 48_000, vec![]);
        engine.run_frame();
        let base = engine.parameter(&port_addr("/p.value")).unwrap().base;
        (base, delivered(&engine, "/p.value"))
    };
    // /a.out < /b.out: b's set applies last.
    assert_eq!(
        base_after_one_frame("a"),
        (0.75, vec!["/a.out".into(), "/b.out".into()])
    );
    // Renamed to /c: /b.out < /c.out, so c's set applies last.
    assert_eq!(
        base_after_one_frame("c"),
        (0.25, vec!["/b.out".into(), "/c.out".into()])
    );
}

/// External events come first, by sequence number, then module events.
#[test]
fn external_events_come_first_in_sequence_order() {
    let plan = plan(
        vec![
            inst_with("a", "test.setter", &[("value", 0.25)]),
            inst("p", "test.probe_add"),
            inst("q", "test.probe_add"),
        ],
        vec![conn("a.out", "p.value")],
    );
    let external = vec![
        ext(2, 7, "/p.value", EventPayload::Set(0.125)),
        ext(2, 3, "/p.value", EventPayload::Set(0.375)),
        ext(2, 9, "/q.value", EventPayload::Set(0.875)),
        ext(2, 4, "/q.value", EventPayload::Set(0.0625)),
    ];
    let mut engine = engine(&plan, 48_000, external);
    for _ in 0..3 {
        engine.run_frame();
    }
    assert_eq!(
        delivered(&engine, "/p.value"),
        ["external 3", "external 7", "/a.out"]
    );
    assert_eq!(engine.parameter(&port_addr("/p.value")).unwrap().base, 0.25);
    assert_eq!(delivered(&engine, "/q.value"), ["external 4", "external 9"]);
    assert_eq!(
        engine.parameter(&port_addr("/q.value")).unwrap().base,
        0.875
    );
}

/// A source connected twice to one input delivers every event twice, still in
/// its emission order: `set(1)`, `set(1)`, `set(2)`, `set(2)`, not its list
/// repeated. An order-sensitive receiver reads them as 1122 (the wrong order
/// gives 1212), and `log.jsonl` records the same order, for an event input and
/// for a parameter.
#[test]
fn duplicate_event_connections_keep_emission_order() {
    let plan = plan(
        vec![
            inst("src", "test.pair"),
            inst("seq", "test.sequence"),
            inst("p", "test.probe_add"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("src.out", "seq.in"),
            scaled("src.out", "seq.in", 2.0),
            conn("src.out", "p.value"),
            conn("src.out", "p.value"),
            conn("seq.out", "rec.zeta"),
        ],
    );
    assert_eq!(plan.sources[&port_addr("/seq.in")].len(), 2);
    assert_eq!(plan.sources[&port_addr("/p.value")].len(), 2);
    let rendered = render(&mut engine(&plan, 48_000, vec![]), 1).unwrap();
    assert_eq!(rendered.samples[0], 1122.0);

    let line = |target: &str, value: &str| {
        format!(
            r#"{{"type":"event","frame":0,"target":"{target}","payload":{{"set":{value}}},"source":{{"port":"/src.out"}}}}"#
        )
    };
    let mut expected = String::new();
    for target in ["/p.value", "/seq.in"] {
        for value in ["1.0", "1.0", "2.0", "2.0"] {
            expected.push_str(&line(target, value));
            expected.push('\n');
        }
    }
    assert_eq!(String::from_utf8(rendered.log).unwrap(), expected);
}

// --- 5. Base and modulation --------------------------------------------------

/// A `set` while a signal modulates the parameter changes only the base; the
/// module sees base + modulation (Add).
#[test]
fn set_during_modulation_changes_only_the_base() {
    let plan = plan(
        vec![
            inst_with("p", "test.probe_add", &[("value", 0.25)]),
            inst_with("m", "test.const", &[("value", 0.125)]),
        ],
        vec![scaled("m.out", "p.value", 2.0)],
    );
    let mut engine = engine(
        &plan,
        48_000,
        vec![ext(3, 1, "/p.value", EventPayload::Set(0.0625))],
    );
    for n in 0..6 {
        engine.run_frame();
        let value = engine.parameter(&port_addr("/p.value")).unwrap();
        let base = if n < 3 { 0.25 } else { 0.0625 };
        assert_eq!(value.base, base, "frame {n}");
        assert_eq!(value.effective, base + 0.25, "frame {n}");
        assert_eq!(
            engine.signal_output(&port_addr("/p.out")),
            Some(value.effective)
        );
        assert_eq!(engine.signal_output(&port_addr("/m.out")), Some(0.125));
    }
}

/// Add, Multiply and Replace with modulation, clamped to [0, 1]; with no signal
/// sources the effective value is clamp(base) under every `combine`, and the base
/// itself stays unclamped.
#[test]
fn effective_value_follows_declared_combine() {
    let plan = plan(
        vec![
            inst_with("m", "test.const", &[("value", 0.5)]),
            inst_with("add", "test.probe_add", &[("value", 0.75)]),
            inst_with("mul", "test.probe_mul", &[("value", 0.75)]),
            inst_with("rep", "test.probe_replace", &[("value", 0.75)]),
            inst("add0", "test.probe_add"),
            inst("mul0", "test.probe_mul"),
            inst("rep0", "test.probe_replace"),
        ],
        vec![
            conn("m.out", "add.value"),
            conn("m.out", "mul.value"),
            conn("m.out", "rep.value"),
        ],
    );
    let alone = ["/add0.value", "/mul0.value", "/rep0.value"];
    let mut external = Vec::new();
    for (k, target) in alone.iter().enumerate() {
        external.push(ext(2, k as u64, target, EventPayload::Set(7.0)));
        external.push(ext(4, 10 + k as u64, target, EventPayload::Set(-3.0)));
    }
    let mut engine = engine(&plan, 48_000, external);
    let effective = |engine: &Engine, port: &str| engine.parameter(&port_addr(port)).unwrap();

    engine.run_frame();
    assert_eq!(effective(&engine, "/add.value").effective, 1.0); // 0.75 + 0.5, clamped
    assert_eq!(effective(&engine, "/mul.value").effective, 0.375);
    assert_eq!(effective(&engine, "/rep.value").effective, 0.5);
    for target in alone {
        assert_eq!(effective(&engine, target).effective, 0.5);
    }

    for (frame, base, clamped) in [(2, 7.0, 1.0), (4, -3.0, 0.0)] {
        while engine.next_frame() <= Frame(frame) {
            engine.run_frame();
        }
        for target in alone {
            let value = effective(&engine, target);
            assert_eq!(
                (value.base, value.effective),
                (base, clamped),
                "{target} at {frame}"
            );
            let out = target.replace(".value", ".out");
            assert_eq!(engine.signal_output(&port_addr(&out)), Some(clamped));
        }
    }
}

// --- 6. Delivery to a state-output instance ----------------------------------

/// Events reaching a state-output instance in frame n (from a direct module or
/// from outside) change its output first at n + 1.
#[test]
fn event_to_state_output_instance_changes_output_at_next_frame() {
    let plan = plan(
        vec![
            inst_with("t", "test.trigger", &[("period", 4.0)]),
            inst("d", "test.event_delay"),
        ],
        vec![conn("t.out", "d.in")],
    );
    // Triggers at frames 0, 4 and 8 from /t, and one external at frame 6.
    let mut engine = engine(
        &plan,
        48_000,
        vec![ext(6, 1, "/d.in", EventPayload::Trigger)],
    );
    let out = output_trace(&mut engine, 10, "/d.out");
    assert_eq!(out, [0.0, 1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0]);
}

// --- 7. Suspension -----------------------------------------------------------

#[test]
fn suspended_trigger_source_emits_nothing() {
    let plan = plan(
        vec![inst("t", "test.trigger"), inst("c", "test.counter")],
        vec![conn("t.out", "c.in")],
    );
    let mut engine = engine(&plan, 48_000, vec![]);
    engine.suspend(&addr("/t"), Frame(4)).unwrap();
    engine.resume(&addr("/t"), Frame(10)).unwrap();
    for n in 0..14 {
        engine.run_frame();
        let emitted = !delivered(&engine, "/c.in").is_empty();
        assert_eq!(emitted, !(4..10).contains(&n), "frame {n}");
    }
    assert_eq!(engine.signal_output(&port_addr("/c.out")), Some(8.0));
}

/// At 1000 Hz, F = 5. Suspended at 10: a control output holds its frame-9 value;
/// an audio output fades from it and is exactly 0 on the F-th suspended frame.
#[test]
fn suspended_signal_outputs_hold_and_audio_fades_to_zero_within_f() {
    let plan = plan(vec![inst("f", "test.frames"), inst("d", "test.dc")], vec![]);
    let mut engine = engine(&plan, 1000, vec![]);
    assert_eq!(engine.fade_frames(), 5);
    for instance in ["/f", "/d"] {
        engine.suspend(&addr(instance), Frame(10)).unwrap();
        engine.resume(&addr(instance), Frame(30)).unwrap();
    }
    let (f, d) = (port_addr("/f.out"), port_addr("/d.out"));
    let mut frames = Vec::new();
    let mut audio = Vec::new();
    for _ in 0..31 {
        engine.run_frame();
        frames.push(engine.signal_output(&f).unwrap());
        audio.push(engine.signal_output(&d).unwrap());
    }
    assert_eq!(frames[10..30], [9.0; 20], "held");
    assert_eq!(frames[30], 10.0, "time stopped while suspended");
    let fading: Vec<f64> = (1..=4).rev().map(|k| 0.75 * (k as f64 / 5.0)).collect();
    assert_eq!(bits(&audio[10..14]), bits(&fading));
    assert_eq!(bits(&audio[14..30]), [0.0f64.to_bits(); 16]);

    // At 96 kHz, F = 480: exactly 0 on frame s + 479, not before.
    let mut engine = common::engine(&plan, 96_000, vec![]);
    assert_eq!(engine.fade_frames(), 480);
    engine.suspend(&addr("/d"), Frame(1)).unwrap();
    let audio = output_trace(&mut engine, 482, "/d.out");
    assert_ne!(audio[479], 0.0);
    assert_eq!(audio[480].to_bits(), 0.0f64.to_bits());
}

#[test]
fn set_while_suspended_is_visible_after_resume() {
    let plan = plan(
        vec![inst_with("p", "test.probe_add", &[("value", 0.25)])],
        vec![],
    );
    let mut engine = engine(
        &plan,
        48_000,
        vec![ext(5, 1, "/p.value", EventPayload::Set(0.125))],
    );
    engine.suspend(&addr("/p"), Frame(3)).unwrap();
    engine.resume(&addr("/p"), Frame(8)).unwrap();
    let (value, out) = (port_addr("/p.value"), port_addr("/p.out"));
    for n in 0..9 {
        engine.run_frame();
        let base = engine.parameter(&value).unwrap().base;
        let seen = engine.signal_output(&out).unwrap();
        assert_eq!(base, if n < 5 { 0.25 } else { 0.125 }, "base at {n}");
        assert_eq!(seen, if n < 8 { 0.25 } else { 0.125 }, "output at {n}");
    }
}

/// A ramp of 8 frames from frame 2, suspended at 5 (i = 3 frames run) for 10
/// frames, continues on resume with its remaining 5 frames: `to` at 20, not 10.
#[test]
fn running_ramp_resumes_with_remaining_frames() {
    let plan = plan(
        vec![inst_with("p", "test.probe_add", &[("value", 0.0)])],
        vec![],
    );
    let mut engine = engine(&plan, 48_000, vec![ext(2, 1, "/p.value", ramp(1.0, 8))]);
    engine.suspend(&addr("/p"), Frame(5)).unwrap();
    engine.resume(&addr("/p"), Frame(15)).unwrap();
    let bases = base_trace(&mut engine, 22, "/p.value");
    let i_at = |n: usize| match n {
        0..=2 => 0,
        3..=4 => n - 2,
        5..=14 => 2,
        _ => n - 12,
    };
    for (n, base) in bases.iter().enumerate() {
        let i = i_at(n);
        let expected = if i >= 8 { 1.0 } else { i as f64 / 8.0 };
        assert_eq!(base.to_bits(), expected.to_bits(), "frame {n}");
    }
    assert_eq!(bases[20], 1.0);
    assert_ne!(bases[19], 1.0);
}

#[test]
fn discarded_triggers_are_counted() {
    let plan = plan(
        vec![
            inst("t", "test.trigger"),
            inst("c", "test.counter"),
            inst("rec", "test.recorder"),
        ],
        vec![conn("t.out", "c.in"), conn("c.out", "rec.zeta")],
    );
    let mut engine = engine(&plan, 48_000, vec![]);
    engine.suspend(&addr("/c"), Frame(4)).unwrap();
    engine.resume(&addr("/c"), Frame(10)).unwrap();
    let rendered = render(&mut engine, 12).unwrap();
    assert_eq!(engine.discards(&addr("/c")), Some(6));
    let log = String::from_utf8(rendered.log).unwrap();
    let discards = log
        .lines()
        .filter(|l| l.contains(r#""type":"discard""#) && l.contains(r#""reason":"suspended""#))
        .count();
    assert_eq!(discards, 6);
    assert!(log.contains(r#"{"type":"resume","frame":10,"address":"/c","discarded":6}"#));
    // The counter's output held while it was suspended.
    let counts: Vec<f64> = rendered.samples.chunks(2).map(|f| f[0]).collect();
    assert_eq!(
        counts,
        [1.0, 2.0, 3.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 5.0, 6.0]
    );
}

/// A ramp received while suspended waits and starts at resume from the base
/// there; a later `set` while suspended cancels it.
#[test]
fn ramp_issued_while_suspended_starts_at_resume() {
    let plan = plan(
        vec![inst_with("p", "test.probe_add", &[("value", 0.5)])],
        vec![],
    );
    let mut engine = engine(&plan, 48_000, vec![ext(4, 1, "/p.value", ramp(1.0, 4))]);
    engine.suspend(&addr("/p"), Frame(2)).unwrap();
    engine.resume(&addr("/p"), Frame(10)).unwrap();
    let bases = base_trace(&mut engine, 16, "/p.value");
    assert_eq!(&bases[..11], [0.5; 11]);
    assert_eq!(&bases[11..], [0.625, 0.75, 0.875, 1.0, 1.0]);

    let external = vec![
        ext(4, 1, "/p.value", ramp(1.0, 4)),
        ext(6, 2, "/p.value", EventPayload::Set(0.25)),
    ];
    let mut engine = common::engine(&plan, 48_000, external);
    engine.suspend(&addr("/p"), Frame(2)).unwrap();
    engine.resume(&addr("/p"), Frame(10)).unwrap();
    let bases = base_trace(&mut engine, 16, "/p.value");
    assert_eq!(&bases[6..], [0.25; 10]);
}

/// At 1000 Hz (F = 5), after a suspension longer than F, the audio output fades
/// in from 0 and is exactly live on the F-th running frame.
#[test]
fn audio_fades_in_over_f_on_resume() {
    let plan = plan(vec![inst("d", "test.dc")], vec![]);
    let mut engine = engine(&plan, 1000, vec![]);
    engine.suspend(&addr("/d"), Frame(10)).unwrap();
    engine.resume(&addr("/d"), Frame(30)).unwrap();
    let audio = output_trace(&mut engine, 36, "/d.out");
    assert_eq!(audio[29].to_bits(), 0.0f64.to_bits());
    let fading: Vec<f64> = (1..=4).map(|k| 0.75 * (k as f64 / 5.0)).collect();
    assert_eq!(bits(&audio[30..34]), bits(&fading));
    assert_eq!(audio[34].to_bits(), 0.75f64.to_bits());
    assert_eq!(audio[35], 0.75);
}

/// The worked trace of `docs/semantics.md` at F = 240 (48 kHz): a one-frame
/// suspension, then a full fade-in from 0 over 240 frames.
#[test]
fn one_frame_suspension_fades_in_from_zero_over_f() {
    let plan = plan(vec![inst("d", "test.dc")], vec![]);
    let mut engine = engine(&plan, 48_000, vec![]);
    assert_eq!(engine.fade_frames(), 240);
    engine.suspend(&addr("/d"), Frame(10)).unwrap();
    engine.resume(&addr("/d"), Frame(11)).unwrap();
    let audio = output_trace(&mut engine, 260, "/d.out");
    assert_eq!(audio[9], 0.75);
    assert_eq!(audio[10], 0.746875);
    assert_eq!(audio[11], 0.003125);
    assert_eq!(audio[12], 0.00625);
    assert_eq!(audio[13], 0.009375000000000001);
    for (n, &value) in audio.iter().enumerate().take(250).skip(11) {
        let expected = 0.75 * ((n - 10) as f64 / 240.0);
        assert_eq!(value.to_bits(), expected.to_bits(), "frame {n}");
        assert!(value < 0.75);
    }
    assert_eq!(audio[249], 0.746875);
    assert_eq!(audio[250].to_bits(), 0.75f64.to_bits());
    assert_eq!(audio[251], 0.75);
}

/// A suspend during a fade-in fades out from what readers saw at s − 1 and is
/// exactly 0 on the F-th suspended frame.
#[test]
fn suspend_during_fade_in_fades_out_from_current_output() {
    let plan = plan(vec![inst("d", "test.dc")], vec![]);
    let mut engine = engine(&plan, 48_000, vec![]);
    for (suspend, resume) in [(10, 11), (111, 500)] {
        engine.suspend(&addr("/d"), Frame(suspend)).unwrap();
        engine.resume(&addr("/d"), Frame(resume)).unwrap();
    }
    let audio = output_trace(&mut engine, 760, "/d.out");
    let held = audio[110];
    assert_eq!(held, 0.75 * (100.0 / 240.0));
    assert_eq!(held, 0.3125);
    assert_eq!(audio[111], held * (239.0 / 240.0));
    assert_ne!(audio[349], 0.0);
    assert_eq!(bits(&audio[350..500]), [0.0f64.to_bits(); 150]);
    assert_eq!(audio[500], 0.75 * (1.0 / 240.0));
    assert_eq!(audio[739].to_bits(), 0.75f64.to_bits());
}

/// A resume during a fade-out starts the fade-in from 0 again, whatever the
/// output was, and is exactly live on the F-th running frame.
#[test]
fn resume_during_fade_out_restarts_fade_in_from_zero() {
    let plan = plan(vec![inst("d", "test.dc")], vec![]);
    let mut engine = engine(&plan, 48_000, vec![]);
    engine.suspend(&addr("/d"), Frame(10)).unwrap();
    engine.resume(&addr("/d"), Frame(110)).unwrap();
    let audio = output_trace(&mut engine, 360, "/d.out");
    assert_eq!(audio[109], 0.75 * (140.0 / 240.0));
    assert_eq!(audio[109], 0.4375);
    assert_eq!(audio[110], 0.003125);
    assert!(audio[348] < 0.75);
    assert_eq!(audio[349].to_bits(), 0.75f64.to_bits());
}

// --- 8. Ramps ----------------------------------------------------------------

#[test]
fn ramp_with_zero_frames_is_to_at_start() {
    let plan = plan(
        vec![inst_with("p", "test.probe_add", &[("value", 0.25)])],
        vec![],
    );
    let mut engine = engine(&plan, 48_000, vec![ext(3, 1, "/p.value", ramp(0.75, 0))]);
    let bases = base_trace(&mut engine, 5, "/p.value");
    assert_eq!(bases, [0.25, 0.25, 0.25, 0.75, 0.75]);
}

/// For N ∈ {1, 7, 96000}: the base is b at n₀, the closed form in between, and
/// exactly `to` at n₀ + N.
#[test]
fn ramp_reaches_to_exactly_after_n_running_frames() {
    let (b, to, n0) = (0.25f64, 0.875f64, 2u64);
    for frames in [1u64, 7, 96_000] {
        let plan = plan(
            vec![inst_with("p", "test.probe_add", &[("value", b)])],
            vec![],
        );
        let mut engine = engine(
            &plan,
            96_000,
            vec![ext(n0, 1, "/p.value", ramp(to, frames))],
        );
        let bases = base_trace(&mut engine, (n0 + frames + 2) as usize, "/p.value");
        let at = |n: u64| bases[n as usize];
        assert_eq!(at(n0).to_bits(), b.to_bits(), "N = {frames}");
        for i in 1..frames {
            let expected = b + (to - b) * (i as f64 / frames as f64);
            assert_eq!(
                at(n0 + i).to_bits(),
                expected.to_bits(),
                "N = {frames}, i = {i}"
            );
        }
        assert_eq!(at(n0 + frames).to_bits(), to.to_bits(), "N = {frames}");
        assert_eq!(at(n0 + frames + 1).to_bits(), to.to_bits(), "N = {frames}");
    }
}

/// With 5 suspended frames in between, `to` arrives 5 frames later.
#[test]
fn ramp_completion_is_postponed_by_suspended_frames() {
    let (b, to, n0) = (0.25f64, 0.875f64, 2u64);
    for frames in [1u64, 7, 96_000] {
        let plan = plan(
            vec![inst_with("p", "test.probe_add", &[("value", b)])],
            vec![],
        );
        let mut engine = engine(
            &plan,
            96_000,
            vec![ext(n0, 1, "/p.value", ramp(to, frames))],
        );
        engine.suspend(&addr("/p"), Frame(n0 + 1)).unwrap();
        engine.resume(&addr("/p"), Frame(n0 + 6)).unwrap();
        let bases = base_trace(&mut engine, (n0 + frames + 7) as usize, "/p.value");
        let done = (n0 + frames + 5) as usize;
        assert_eq!(bases[done].to_bits(), to.to_bits(), "N = {frames}");
        assert_ne!(bases[done - 1], to, "N = {frames}");
    }
}

#[test]
fn interrupting_set_or_ramp_starts_from_interrupted_value() {
    let plan = plan(
        vec![inst_with("p", "test.probe_add", &[("value", 0.0)])],
        vec![],
    );
    // A ramp 0 → 1 over 8 frames, interrupted at frame 4 (value 0.5) by a ramp
    // to 0 over 4 frames, which starts from 0.5.
    let external = vec![
        ext(0, 1, "/p.value", ramp(1.0, 8)),
        ext(4, 2, "/p.value", ramp(0.0, 4)),
    ];
    let bases = base_trace(&mut engine(&plan, 48_000, external), 10, "/p.value");
    assert_eq!(
        bases,
        [0.0, 0.125, 0.25, 0.375, 0.5, 0.375, 0.25, 0.125, 0.0, 0.0]
    );
    // A `set` at frame 4 replaces the interrupted ramp's value and ends it.
    let external = vec![
        ext(0, 1, "/p.value", ramp(1.0, 8)),
        ext(4, 2, "/p.value", EventPayload::Set(0.3)),
    ];
    let bases = base_trace(&mut engine(&plan, 48_000, external), 10, "/p.value");
    assert_eq!(
        bases,
        [0.0, 0.125, 0.25, 0.375, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3]
    );
}

/// ADR 0016 Verification asks for ramps "and random k-partitions". Met by
/// option B (coordinator decision, M1-4): the executor runs k = 1 only, and
/// block execution (D-EXEC-3) is out of scope, so the block-partition clause
/// itself is not exercised until D-EXEC-3. This test checks the property ADR
/// 0016 §4a gives for k-independence: every frame's base is the closed form of
/// (i, N), never accumulated. The reference is evaluated block by block over
/// random partitions and compared bit for bit with the executor's per-frame
/// values.
#[test]
fn ramp_values_match_closed_form_over_random_partitions() {
    let (b, to, n0, frames) = (0.125f64, 0.875f64, 3u64, 1000u64);
    let plan = plan(
        vec![inst_with("p", "test.probe_add", &[("value", b)])],
        vec![],
    );
    let total = n0 + frames + 10;
    let mut engine = engine(
        &plan,
        48_000,
        vec![ext(n0, 1, "/p.value", ramp(to, frames))],
    );
    let bases = base_trace(&mut engine, total as usize, "/p.value");
    let closed = |n: u64| match n.checked_sub(n0) {
        None => b,
        Some(i) if i >= frames => to,
        Some(i) => b + (to - b) * (i as f64 / frames as f64),
    };
    for seed in 0..16 {
        let mut rng = SplitMix64::new(seed);
        let mut start = 0;
        while start < total {
            let k = 1 + rng.next_u64() % 64;
            for n in start..(start + k).min(total) {
                assert_eq!(
                    bases[n as usize].to_bits(),
                    closed(n).to_bits(),
                    "seed {seed}, frame {n}"
                );
            }
            start += k;
        }
    }
}
