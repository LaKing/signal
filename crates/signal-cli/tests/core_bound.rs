//! M2.10-1 review: the core types stay within the engine's magnitude bound
//! (10⁶, `docs/semantics.md` §7), so none of them faults on its own, through
//! the engine: `std.frames` past 10⁶ frames, and `std.div`, `std.exp`,
//! `std.pow`, `std.tan` and `std.neg` where their results would exceed it.
//! M2.10-2 (AC10, AC11): the same for core set II, rendered past the bound,
//! and, through the engine, `std.random`'s held value and the canonical order
//! of same-frame `Set` events. It lives here because this crate already
//! depends on both `signal-std` and `signal-engine`.

use signal_core::{EventPayload, Frame, InstanceAddress, PortAddress, Registry};
use signal_engine::{Engine, EngineConfig, ExternalEvent, LogRecord};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn inst(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect(),
        scope: None,
    }
}

fn conn(from: &str, to: &str) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    }
}

fn plan(instances: Vec<InstanceDef>, connections: Vec<ConnectionDef>) -> ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    build_plan(&root, &[], &registry(), 1).unwrap()
}

/// An engine at a low rate with the largest blocks, so a long render is fast.
fn engine(plan: &ExecutionPlan) -> Engine {
    engine_with(plan, Vec::new())
}

/// [`engine`] with external events.
fn engine_with(plan: &ExecutionPlan, events: Vec<ExternalEvent>) -> Engine {
    let mut config = EngineConfig::new(1_000, 7);
    config.block_capacity = 4096;
    Engine::new(plan, &registry(), config, events).unwrap()
}

/// External event number `sequence` at `frame` to `target`.
fn external(sequence: u64, frame: u64, target: &str, payload: EventPayload) -> ExternalEvent {
    ExternalEvent {
        frame: Frame(frame),
        sequence,
        target: target.parse().unwrap(),
        payload,
    }
}

/// Runs `engine` up to frame `until` (exclusive) in blocks, and returns the
/// fault records it logged.
fn run_until(engine: &mut Engine, until: u64) -> Vec<LogRecord> {
    let mut faults = Vec::new();
    while engine.next_frame().0 < until {
        let left = usize::try_from(until - engine.next_frame().0).unwrap();
        assert!(
            engine.run_block(left.min(4096)) > 0,
            "{:?}",
            engine.failure()
        );
        engine.drain_log(&mut |r| {
            if matches!(r, LogRecord::Fault { .. }) {
                faults.push(*r);
            }
        });
    }
    faults
}

fn output(engine: &Engine, port: &str) -> f64 {
    engine
        .signal_output(&port.parse::<PortAddress>().unwrap())
        .unwrap()
}

/// H1: `std.frames` runs past 10⁶ frames without a fault; `lo` wraps and
/// `hi` carries exactly.
#[test]
fn frames_runs_past_a_million_frames_without_a_fault() {
    let plan = plan(
        vec![
            inst("n", "std.frames", &[]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![conn("n.lo", "out.l"), conn("n.hi", "out.r")],
    );
    let mut engine = engine(&plan);
    let lo_hi = |engine: &Engine| (output(engine, "/n.lo"), output(engine, "/n.hi"));
    // The last frame run is 999 999: count 999 999.
    assert_eq!(run_until(&mut engine, 1_000_000), []);
    assert_eq!(lo_hi(&engine), (999_999.0, 0.0));
    // Frame 1 000 000: lo wraps, hi carries.
    assert_eq!(run_until(&mut engine, 1_000_001), []);
    assert_eq!(lo_hi(&engine), (0.0, 1.0));
    assert_eq!(run_until(&mut engine, 1_100_000), []);
    assert_eq!(lo_hi(&engine), (99_999.0, 1.0));
    let n: InstanceAddress = "/n".parse().unwrap();
    assert_eq!(engine.is_muted(&n), Some(false));
}

/// M1: results beyond the bound are clamped to ±10⁶ through the engine, so
/// nothing faults: div(±1, 1e−7), exp(14), pow(10, 7), pow(−10, 7), tan at
/// the double nearest π/2, and neg of a fan-in sum of 2·10⁶.
#[test]
fn core_results_beyond_the_bound_are_clamped_without_a_fault() {
    let pi_2 = std::f64::consts::FRAC_PI_2;
    let plan = plan(
        vec![
            inst("one", "std.const", &[("value", 1.0)]),
            inst("minus_one", "std.const", &[("value", -1.0)]),
            inst("tiny", "std.const", &[("value", 1e-7)]),
            inst("fourteen", "std.const", &[("value", 14.0)]),
            inst("ten", "std.const", &[("value", 10.0)]),
            inst("minus_ten", "std.const", &[("value", -10.0)]),
            inst("seven", "std.const", &[("value", 7.0)]),
            inst("pole", "std.const", &[("value", pi_2)]),
            inst("top", "std.const", &[("value", 1e6)]),
            inst("top2", "std.const", &[("value", 1e6)]),
            inst("div", "std.div", &[]),
            inst("div_neg", "std.div", &[]),
            inst("exp", "std.exp", &[]),
            inst("pow", "std.pow", &[]),
            inst("pow_neg", "std.pow", &[]),
            inst("tan", "std.tan", &[]),
            inst("neg", "std.neg", &[]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("one.out", "div.a"),
            conn("tiny.out", "div.b"),
            conn("minus_one.out", "div_neg.a"),
            conn("tiny.out", "div_neg.b"),
            conn("fourteen.out", "exp.in"),
            conn("ten.out", "pow.a"),
            conn("seven.out", "pow.b"),
            conn("minus_ten.out", "pow_neg.a"),
            conn("seven.out", "pow_neg.b"),
            conn("pole.out", "tan.in"),
            // Two sources sum to 2e6 at the input.
            conn("top.out", "neg.in"),
            conn("top2.out", "neg.in"),
            conn("div.out", "out.l"),
            conn("neg.out", "out.r"),
        ],
    );
    let mut engine = engine(&plan);
    assert_eq!(run_until(&mut engine, 512), []);
    let neg_in: PortAddress = "/neg.in".parse().unwrap();
    assert_eq!(engine.signal_input(&neg_in), Some(2e6));
    for (port, expected) in [
        ("/div.out", 1e6),
        ("/div_neg.out", -1e6),
        ("/exp.out", 1e6),
        ("/pow.out", 1e6),
        ("/pow_neg.out", -1e6),
        ("/tan.out", 1e6),
        ("/neg.out", -1e6),
    ] {
        assert_eq!(output(&engine, port), expected, "{port}");
    }
    for name in ["div", "div_neg", "exp", "pow", "pow_neg", "tan", "neg"] {
        let address: InstanceAddress = format!("/{name}").parse().unwrap();
        assert_eq!(engine.is_muted(&address), Some(false), "{name}");
    }
}

/// M2.10-2 AC10, AC11: core set II rendered past the bound through the
/// engine, with no fault and nothing muted. A counter stepping 1e5 per
/// trigger (1.25e7 in all), an integrator of 1e6 at gain 1e6, a buffer write
/// of a fan-in sum of 2e6, a table's Hermite overshoot of 1.125e6, a random
/// value at ±1e6, the vector sum of 1024 elements of 1e6, vadd and vmul of
/// them, a phasor at 1e9 Hz, a latch and a route of an external Set(±5e6),
/// and to_events of 2e6.
#[test]
fn core_set_two_stays_within_the_bound() {
    let instances = vec![
        inst("top", "std.const", &[("value", 1e6)]),
        inst("top2", "std.const", &[("value", 1e6)]),
        inst("one", "std.const", &[("value", 1.0)]),
        inst("half", "std.const", &[("value", 1.5)]),
        inst("clk", "std.phasor", &[("freq", 250.0)]),
        inst("fast", "std.phasor", &[("freq", 1e9)]),
        inst("cnt", "std.counter", &[("step", 1e5)]),
        inst("int", "std.integrator", &[("mode", 1.0), ("gain", 1e6)]),
        inst("buf", "std.buffer1k", &[("size", 16.0)]),
        inst(
            "tab",
            "std.table4",
            &[("v1", 1e6), ("v2", 1e6), ("interp", 2.0)],
        ),
        inst("rnd", "std.random", &[("lo", -1e6), ("hi", 1e6)]),
        inst("pk", "std.pack1024", &[]),
        inst("red", "std.reduce1024", &[]),
        inst("vs", "std.vadd1024", &[]),
        inst("vm", "std.vmul1024", &[]),
        inst("ixs", "std.index1024", &[]),
        inst("ixm", "std.index1024", &[]),
        inst("lat", "std.latch", &[]),
        inst("rt", "std.route", &[]),
        inst("lrt", "std.latch", &[]),
        inst("te", "std.to_events", &[]),
        inst("lte", "std.latch", &[]),
        inst("out", "std.audio_out", &[]),
    ];
    let mut connections = vec![
        conn("clk.wrap", "cnt.trigger"),
        conn("clk.wrap", "rnd.trigger"),
        conn("top.out", "int.in"),
        conn("top.out", "buf.value"),
        conn("top2.out", "buf.value"),
        conn("one.out", "buf.gate"),
        conn("half.out", "tab.pos"),
        conn("pk.out", "red.in"),
        conn("pk.out", "vs.a"),
        conn("pk.out", "vs.b"),
        conn("pk.out", "vm.a"),
        conn("pk.out", "vm.b"),
        conn("vs.out", "ixs.in"),
        conn("vm.out", "ixm.in"),
        conn("rt.b", "lrt.in"),
        conn("top.out", "te.in"),
        conn("top2.out", "te.in"),
        conn("te.out", "lte.in"),
        conn("cnt.count", "out.l"),
        conn("fast.phase", "out.r"),
    ];
    for e in 0..1024 {
        connections.push(conn("top.out", &format!("pk.e{e}")));
    }
    let plan = plan(instances, connections);
    let events = vec![
        external(0, 100, "/lat.in", EventPayload::Set(5e6)),
        external(1, 100, "/rt.in", EventPayload::Set(-5e6)),
    ];
    let mut engine = engine_with(&plan, events);
    let (mut counts, mut phases, mut randoms) = (Vec::new(), Vec::new(), Vec::new());
    for frame in (16..=512).step_by(16) {
        assert_eq!(run_until(&mut engine, frame), [], "frame {frame}");
        counts.push(output(&engine, "/cnt.count"));
        phases.push(output(&engine, "/fast.phase"));
        randoms.push(output(&engine, "/rnd.out"));
    }
    // 128 triggers of 1e5: the count went past the bound and wrapped.
    assert!(counts.iter().all(|c| (0.0..1e6).contains(c)), "{counts:?}");
    assert!(counts.windows(2).any(|w| w[1] < w[0]), "the count wrapped");
    assert!(phases.iter().all(|p| (0.0..1.0).contains(p)), "{phases:?}");
    assert!(
        randoms.iter().all(|v| (-1e6..1e6).contains(v)),
        "{randoms:?}"
    );
    for (port, expected) in [
        ("/int.out", 1e6),
        ("/buf.out0", 1e6),
        ("/tab.out", 1e6),
        ("/red.sum", 1e6),
        ("/red.mean", 1e6),
        ("/ixs.out", 1e6),
        ("/ixm.out", 1e6),
        ("/lat.out", 1e6),
        ("/lrt.out", -1e6),
        ("/lte.out", 1e6),
    ] {
        assert_eq!(output(&engine, port), expected, "{port}");
    }
    let te_in: PortAddress = "/te.in".parse().unwrap();
    assert_eq!(engine.signal_input(&te_in), Some(2e6));
    for name in [
        "cnt", "fast", "int", "buf", "tab", "rnd", "pk", "red", "vs", "vm", "ixs", "ixm", "lat",
        "rt", "te",
    ] {
        let address: InstanceAddress = format!("/{name}").parse().unwrap();
        assert_eq!(engine.is_muted(&address), Some(false), "{name}");
    }
}

/// Review item 3, through the engine: `std.pack2` of fan-in sums of ±2e6
/// clamps its elements to ±1e6; `std.unpack2` of two vectors meeting at its
/// input ([2e6, −2e6]) clamps its outputs; a `std.buffer1k` holding
/// [0, 1e6, 1e6, 0] (written from a wrapping table, index modulo 4) read at
/// 1.5 with Hermite overshoots to 1.125e6 and is clamped to 1e6. No fault,
/// nothing muted.
#[test]
fn pack_unpack_and_a_buffer_overshoot_stay_within_the_bound() {
    let plan = plan(
        vec![
            inst("top", "std.const", &[("value", 1e6)]),
            inst("top2", "std.const", &[("value", 1e6)]),
            inst("neg", "std.const", &[("value", -1e6)]),
            inst("neg2", "std.const", &[("value", -1e6)]),
            inst("one", "std.const", &[("value", 1.0)]),
            inst("half", "std.const", &[("value", 1.5)]),
            inst("n", "std.frames", &[]),
            inst("pa", "std.pack2", &[]),
            inst("pb", "std.pack2", &[]),
            inst("ua", "std.unpack2", &[]),
            inst("ub", "std.unpack2", &[]),
            inst(
                "tab",
                "std.table4",
                &[("v1", 1e6), ("v2", 1e6), ("interp", 0.0), ("wrap", 1.0)],
            ),
            inst("buf", "std.buffer1k", &[("size", 4.0), ("interp", 2.0)]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("top.out", "pa.e0"),
            conn("top2.out", "pa.e0"),
            conn("neg.out", "pa.e1"),
            conn("neg2.out", "pa.e1"),
            conn("top.out", "pb.e0"),
            conn("neg.out", "pb.e1"),
            // pa clamps to [1e6, −1e6]; with pb, ub receives [2e6, −2e6].
            conn("pa.out", "ua.in"),
            conn("pa.out", "ub.in"),
            conn("pb.out", "ub.in"),
            conn("n.lo", "tab.pos"),
            conn("n.lo", "buf.index"),
            conn("tab.out", "buf.value"),
            conn("one.out", "buf.gate"),
            conn("half.out", "buf.read0"),
            conn("ub.e0", "out.l"),
            conn("buf.out0", "out.r"),
        ],
    );
    let mut engine = engine(&plan);
    assert_eq!(run_until(&mut engine, 64), []);
    let pa_e0: PortAddress = "/pa.e0".parse().unwrap();
    assert_eq!(engine.signal_input(&pa_e0), Some(2e6));
    for (port, expected) in [
        ("/ua.e0", 1e6),
        ("/ua.e1", -1e6),
        ("/ub.e0", 1e6),
        ("/ub.e1", -1e6),
        ("/buf.out0", 1e6),
    ] {
        assert_eq!(output(&engine, port), expected, "{port}");
    }
    for name in ["pa", "pb", "ua", "ub", "tab", "buf"] {
        let address: InstanceAddress = format!("/{name}").parse().unwrap();
        assert_eq!(engine.is_muted(&address), Some(false), "{name}");
    }
}

/// M2.10-2 AC5 (auditor-1-random-held-value), through the engine: a `set` of
/// `hi` or `lo` after the trigger leaves the held value as it was; the next
/// trigger draws with the new bounds.
#[test]
fn random_holds_its_value_when_the_bounds_change() {
    let plan = plan(
        vec![
            inst("rnd", "std.random", &[("lo", 0.0), ("hi", 1.0)]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![conn("rnd.out", "out.l")],
    );
    let events = vec![
        external(0, 10, "/rnd.trigger", EventPayload::Trigger),
        external(1, 20, "/rnd.hi", EventPayload::Set(1e6)),
        external(2, 30, "/rnd.lo", EventPayload::Set(5e5)),
        external(3, 40, "/rnd.trigger", EventPayload::Trigger),
    ];
    let mut engine = engine_with(&plan, events);
    assert_eq!(run_until(&mut engine, 5), []);
    assert_eq!(
        output(&engine, "/rnd.out"),
        0.0,
        "lo before the first trigger"
    );
    assert_eq!(run_until(&mut engine, 11), []);
    let held = output(&engine, "/rnd.out");
    assert!((0.0..1.0).contains(&held), "{held}");
    for until in [21, 31, 40] {
        assert_eq!(run_until(&mut engine, until), []);
        let now = output(&engine, "/rnd.out");
        assert_eq!(now.to_bits(), held.to_bits(), "{until}");
    }
    assert_eq!(run_until(&mut engine, 41), []);
    let drawn = output(&engine, "/rnd.out");
    assert!((5e5..1e6).contains(&drawn), "{drawn}");
}

/// M2.10-2 AC4, AC8: `Set` events that meet in one event input on one frame
/// apply in canonical order, by source address (ADR 0016 §3), so the later
/// address wins in a latch, whichever is connected first; route keeps that
/// order.
#[test]
fn same_frame_sets_meet_in_canonical_order() {
    let plan = plan(
        vec![
            inst("one", "std.const", &[("value", 1.0)]),
            inst("two", "std.const", &[("value", 2.0)]),
            inst("ta", "std.to_events", &[]),
            inst("tb", "std.to_events", &[]),
            inst("tc", "std.to_events", &[]),
            inst("td", "std.to_events", &[]),
            inst("l1", "std.latch", &[]),
            inst("l2", "std.latch", &[]),
            inst("gate", "std.const", &[("value", 1.0)]),
            inst("rt", "std.route", &[]),
            inst("lr", "std.latch", &[]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("one.out", "ta.in"),
            conn("two.out", "tb.in"),
            conn("two.out", "tc.in"),
            conn("one.out", "td.in"),
            // Connected in the other order: the addresses decide.
            conn("tb.out", "l1.in"),
            conn("ta.out", "l1.in"),
            conn("td.out", "l2.in"),
            conn("tc.out", "l2.in"),
            conn("tb.out", "rt.in"),
            conn("ta.out", "rt.in"),
            conn("gate.out", "rt.gate"),
            conn("rt.a", "lr.in"),
            conn("l1.out", "out.l"),
            conn("l2.out", "out.r"),
        ],
    );
    let mut engine = engine(&plan);
    assert_eq!(run_until(&mut engine, 4), []);
    assert_eq!(
        output(&engine, "/l1.out"),
        2.0,
        "/tb.out sorts after /ta.out"
    );
    assert_eq!(
        output(&engine, "/l2.out"),
        1.0,
        "/td.out sorts after /tc.out"
    );
    assert_eq!(output(&engine, "/lr.out"), 2.0);
}

/// M2.10-2 AC6, AC10 (finding auditor-1-vector-premature-clamping): index and
/// reduce compute from the elements as received, a fan-in sum beyond the
/// bound included, and clamp only their results. Two `std.pack2` of
/// [1e6, −5e5] sum to [2e6, −1e6] at the inputs: sum 1e6, mean 5e5, min
/// −1e6, max 2e6 clamped to 1e6, index(0.5) 5e5; no fault, nothing muted.
#[test]
fn vector_results_come_from_the_elements_as_received() {
    let plan = plan(
        vec![
            inst("top", "std.const", &[("value", 1e6)]),
            inst("neg", "std.const", &[("value", -5e5)]),
            inst("half", "std.const", &[("value", 0.5)]),
            inst("pa", "std.pack2", &[]),
            inst("pb", "std.pack2", &[]),
            inst("red", "std.reduce2", &[]),
            inst("ix", "std.index2", &[]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("top.out", "pa.e0"),
            conn("neg.out", "pa.e1"),
            conn("top.out", "pb.e0"),
            conn("neg.out", "pb.e1"),
            conn("pa.out", "red.in"),
            conn("pb.out", "red.in"),
            conn("pa.out", "ix.in"),
            conn("pb.out", "ix.in"),
            conn("half.out", "ix.pos"),
            conn("red.sum", "out.l"),
            conn("ix.out", "out.r"),
        ],
    );
    let mut engine = engine(&plan);
    assert_eq!(run_until(&mut engine, 64), []);
    for (port, expected) in [
        ("/red.sum", 1e6),
        ("/red.mean", 5e5),
        ("/red.min", -1e6),
        ("/red.max", 1e6),
        ("/ix.out", 5e5),
    ] {
        assert_eq!(output(&engine, port), expected, "{port}");
    }
    for name in ["red", "ix"] {
        let address: InstanceAddress = format!("/{name}").parse().unwrap();
        assert_eq!(engine.is_muted(&address), Some(false), "{name}");
    }
}

/// M2.10-2 AC6 (finding auditor-1-index-intermediate-overflow): index reads
/// extreme finite elements without an intermediate overflow. A `std.pack2` of
/// [1, −1] through a connection scaled by `f64::MAX` reaches `std.index2` as
/// [MAX, −MAX], whose difference overflows; at 0.5 the read is their
/// midpoint, 0, and at 0.75 −MAX/2, clamped to −1e6; no fault, nothing muted.
#[test]
fn index_reads_extreme_finite_elements() {
    let scaled = |to: &str| ConnectionDef {
        from: "pk.out".to_owned(),
        to: to.to_owned(),
        scale: Some(f64::MAX),
    };
    let plan = plan(
        vec![
            inst("one", "std.const", &[("value", 1.0)]),
            inst("minus", "std.const", &[("value", -1.0)]),
            inst("half", "std.const", &[("value", 0.5)]),
            inst("most", "std.const", &[("value", 0.75)]),
            inst("pk", "std.pack2", &[]),
            inst("mid", "std.index2", &[]),
            inst("late", "std.index2", &[]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("one.out", "pk.e0"),
            conn("minus.out", "pk.e1"),
            scaled("mid.in"),
            scaled("late.in"),
            conn("half.out", "mid.pos"),
            conn("most.out", "late.pos"),
            conn("mid.out", "out.l"),
            conn("late.out", "out.r"),
        ],
    );
    let mut engine = engine(&plan);
    assert_eq!(run_until(&mut engine, 64), []);
    assert_eq!(output(&engine, "/mid.out"), 0.0);
    assert_eq!(output(&engine, "/late.out"), -1e6);
    for name in ["mid", "late"] {
        let address: InstanceAddress = format!("/{name}").parse().unwrap();
        assert_eq!(engine.is_muted(&address), Some(false), "{name}");
    }
}
