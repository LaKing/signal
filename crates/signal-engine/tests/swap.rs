//! Plan swaps, incremental input and log draining (M2-1, ADR 0006,
//! `docs/semantics.md` §12). The no-allocation check is in `no_allocation.rs`;
//! the `std.sine_osc` phase check is in `signal-cli/tests/plan_swap.rs`.

mod common;

use common::{
    addr, base_trace, conn, engine, ext, inst, inst_with, plan, port_addr, ramp, registry, scaled,
};
use signal_core::{EventPayload, Frame, Registry};
use signal_engine::{
    EditFull, Engine, EngineConfig, EngineError, ExternalEvent, ExternalFull, InputDiscards,
    LogRecord,
};
use signal_graph::{ConnectionDef, ExecutionPlan, InstanceDef};
use signal_output::{RenderError, render, render_with};

/// 1 kHz: the fade length F is 5 frames.
const RATE: u32 = 1_000;
const F: u64 = 5;

/// The last frame's records as `log.jsonl` lines, in canonical order.
fn drain_lines(engine: &mut Engine) -> Vec<String> {
    let mut records = Vec::new();
    engine.drain_log(&mut |r| records.push(*r));
    records
        .iter()
        .map(|r| serde_json::to_string(&engine.log_line(r).unwrap()).unwrap())
        .collect()
}

/// The fade-in gain applied on the `j`-th frame of an added instance.
fn faded_in(live: f64, j: u64) -> f64 {
    if j + 1 >= F {
        live
    } else {
        live * ((j + 1) as f64 / F as f64)
    }
}

/// The plan of `no_allocation.rs`: module and external events, triggers to a
/// parameter, ramps, modulation, a feedback loop, randomness, a fault.
fn rich_plan() -> ExecutionPlan {
    plan(rich_instances(), rich_connections())
}

fn rich_instances() -> Vec<InstanceDef> {
    vec![
        inst_with("f1", "test.flood", &[("count", 16.0)]),
        inst_with("f2", "test.flood", &[("count", 16.0)]),
        inst("c", "test.counter"),
        inst_with("s", "test.setter", &[("value", 0.25)]),
        inst_with("k", "test.const", &[("value", 0.125)]),
        inst("p", "test.probe_add"),
        inst_with("t", "test.trigger", &[("period", 2.0)]),
        inst("e", "test.event_delay"),
        inst("impulse", "test.impulse"),
        inst("loop", "test.gain"),
        inst("delay", "test.unit_delay"),
        inst("d", "test.dc"),
        inst("x", "test.faulty"),
        inst("n", "test.noise"),
        inst("rec", "test.recorder"),
    ]
}

fn rich_connections() -> Vec<ConnectionDef> {
    vec![
        conn("f1.out", "c.in"),
        conn("f2.out", "c.in"),
        conn("s.out", "p.value"),
        conn("k.out", "p.value"),
        conn("t.out", "p.value"),
        conn("t.out", "e.in"),
        conn("impulse.out", "loop.in"),
        scaled("delay.out", "loop.in", 0.5),
        conn("loop.out", "delay.in"),
        conn("d.out", "rec.zeta"),
        conn("x.out", "rec.zeta"),
        conn("n.out", "rec.alpha"),
        conn("loop.out", "rec.alpha"),
        conn("p.out", "rec.alpha"),
    ]
}

/// The events of `no_allocation.rs`: unique sequence numbers that do not
/// increase with the frame.
fn rich_events() -> Vec<ExternalEvent> {
    vec![
        ext(20, 1, "/c.in", EventPayload::Trigger),
        ext(20, 2, "/c.in", EventPayload::Trigger),
        ext(20, 3, "/c.in", EventPayload::Trigger),
        ext(5, 4, "/p.value", ramp(1.0, 10)),
        ext(30, 5, "/p.value", EventPayload::Set(0.5)),
        ext(40, 6, "/p.value", ramp(0.0, 20)),
        ext(45, 7, "/p.value", ramp(0.75, 8)),
        ext(47, 8, "/c.in", EventPayload::Trigger),
    ]
}

fn rich_edits(engine: &mut Engine) {
    let edits = [
        ("/d", 10, 11),
        ("/d", 20, 23),
        ("/d", 30, 45),
        ("/p", 44, 50),
        ("/c", 46, 48),
        ("/e", 25, 30),
        ("/n", 3, 60),
    ];
    for (instance, suspend, resume) in edits {
        engine.suspend(&addr(instance), Frame(suspend)).unwrap();
        engine.resume(&addr(instance), Frame(resume)).unwrap();
    }
}

/// What a run produced, frame by frame.
#[derive(Debug, PartialEq)]
struct Run {
    /// The sink's inputs (`/rec.zeta`, `/rec.alpha`), as bits.
    audio: Vec<(u64, u64)>,
    lines: Vec<Vec<String>>,
}

/// Runs `frames` frames; before frame `f`, `before(engine, f)` may push input
/// or swap plans.
fn run(engine: &mut Engine, frames: u64, mut before: impl FnMut(&mut Engine, u64)) -> Run {
    let (zeta, alpha) = (port_addr("/rec.zeta"), port_addr("/rec.alpha"));
    let mut result = Run {
        audio: Vec::new(),
        lines: Vec::new(),
    };
    for frame in 0..frames {
        before(engine, frame);
        engine.run_frame();
        result.audio.push((
            engine.signal_input(&zeta).unwrap().to_bits(),
            engine.signal_input(&alpha).unwrap().to_bits(),
        ));
        result.lines.push(drain_lines(engine));
    }
    result
}

fn swap_at(engine: &mut Engine, plan: &ExecutionPlan, frame: u64) {
    let mut swap = engine.prepare_swap(plan, &registry()).unwrap();
    engine.commit_swap(&mut swap, Frame(frame)).unwrap();
}

fn is_swap(line: &str) -> bool {
    line.starts_with(r#"{"type":"swap""#)
}

/// S1, AC1: a swap to an identical plan, at every frame of the run, changes
/// nothing but the swap record, which comes first in its frame. Every survivor
/// carries its primitive, ramps (running and pending), suspension and fades,
/// fault state, outputs and held values across.
#[test]
fn identical_swap_at_any_frame_changes_nothing_but_the_swap_record() {
    const N: u64 = 80;
    let plan = rich_plan();
    let fresh = || {
        let mut engine = engine(&plan, RATE, rich_events());
        rich_edits(&mut engine);
        engine
    };
    let reference = run(&mut fresh(), N, |_, _| {});
    assert!(
        reference
            .lines
            .iter()
            .flatten()
            .any(|l| l.contains("fault"))
    );

    for s in 0..N {
        let mut engine = fresh();
        let swapped = run(&mut engine, N, |engine, frame| {
            if frame == s {
                swap_at(engine, &plan, frame);
            }
        });
        assert_eq!(swapped.audio, reference.audio, "swap at {s}");
        let first = &swapped.lines[s as usize][0];
        assert_eq!(
            first,
            &format!(r#"{{"type":"swap","frame":{s},"version":1,"plan_version":1}}"#)
        );
        let without: Vec<Vec<String>> = swapped
            .lines
            .iter()
            .map(|frame| frame.iter().filter(|l| !is_swap(l)).cloned().collect())
            .collect();
        assert_eq!(without, reference.lines, "swap at {s}");
        assert_eq!(engine.version(), 1);
        assert_eq!(engine.input_discards(), InputDiscards::default());
    }
}

/// S1: survivors are still checked against the registry, and a changed type
/// at an address is a removal plus an addition (a fresh instance).
#[test]
fn survivors_are_checked_and_a_changed_type_is_a_new_instance() {
    let counting = plan(vec![inst("x", "test.frames")], vec![]);
    let noise = plan(vec![inst("x", "test.noise")], vec![]);
    let mut engine = engine(&counting, RATE, vec![]);
    assert!(matches!(
        engine.prepare_swap(&counting, &Registry::new()),
        Err(EngineError::UnknownModuleType { .. })
    ));

    let out = port_addr("/x.out");
    for _ in 0..10 {
        engine.run_frame();
    }
    assert_eq!(engine.signal_output(&out), Some(9.0));
    swap_at(&mut engine, &noise, 10);
    engine.run_frame();
    let mut fresh_noise = common::engine(&noise, RATE, vec![]);
    fresh_noise.run_frame();
    let first = fresh_noise.signal_output(&out).unwrap();
    assert_eq!(
        engine.signal_output(&out).unwrap().to_bits(),
        faded_in(first, 0).to_bits()
    );
    // Back to the counter: it starts again from 0, nothing carried.
    engine.drain_log(&mut |_| {});
    swap_at(&mut engine, &counting, 11);
    engine.run_frame();
    assert_eq!(engine.signal_output(&out), Some(0.0));
}

/// S4, AC3: an added instance starts on the swap frame. Its `audio` outputs
/// equal its unswapped output (the same instance in a fresh engine of the new
/// plan, on its j-th frame) times the fade-in curve for exactly F frames;
/// control and event outputs start unfaded.
#[test]
fn added_instance_fades_in_from_the_swap_frame() {
    const S: u64 = 17;
    let before = plan(
        vec![inst_with("k", "test.const", &[("value", 0.5)])],
        vec![],
    );
    let after = plan(
        vec![
            inst_with("k", "test.const", &[("value", 0.5)]),
            inst("n", "test.noise"),
            inst_with("o", "test.osc", &[("freq", 130.0)]),
            inst("f", "test.frames"),
            inst("t", "test.trigger"),
            inst("c", "test.counter"),
        ],
        vec![conn("t.out", "c.in")],
    );
    let ports = ["/n.out", "/o.out", "/f.out", "/c.out"].map(port_addr);
    let read = |engine: &Engine| ports.clone().map(|p| engine.signal_output(&p).unwrap());

    let mut reference = engine(&after, RATE, vec![]);
    let unswapped: Vec<[f64; 4]> = (0..20)
        .map(|_| {
            reference.run_frame();
            read(&reference)
        })
        .collect();

    let mut engine = engine(&before, RATE, vec![]);
    for _ in 0..S {
        engine.run_frame();
    }
    swap_at(&mut engine, &after, S);
    for (j, live) in unswapped.iter().enumerate() {
        engine.run_frame();
        let [n, o, f, c] = read(&engine);
        let j = j as u64;
        assert_eq!(
            n.to_bits(),
            faded_in(live[0], j).to_bits(),
            "noise, j = {j}"
        );
        assert_eq!(o.to_bits(), faded_in(live[1], j).to_bits(), "osc, j = {j}");
        assert_eq!(f, live[2], "control output, j = {j}");
        assert_eq!(c, live[3], "events, j = {j}");
    }
    // The fade spans exactly F frames: frame F − 1 is already live.
    assert_ne!(unswapped[0][0], faded_in(unswapped[0][0], 0));
    assert_eq!(engine.removal_swap_frame(Frame(10)), Frame(10 + F));
}

/// The AC4 instances: `/o` (audio) feeds the sink and is the only modulation
/// source of a Multiply and a Replace parameter (base 0.5).
fn removal_plans() -> (ExecutionPlan, ExecutionPlan) {
    let receivers = || {
        vec![
            inst("pm", "test.probe_mul"),
            inst("pr", "test.probe_replace"),
            inst("rec", "test.recorder"),
        ]
    };
    let mut with = receivers();
    with.push(inst_with("o", "test.osc", &[("freq", 130.0)]));
    let with = plan(
        with,
        vec![
            conn("o.out", "rec.zeta"),
            conn("o.out", "pm.value"),
            scaled("o.out", "pr.value", -1.0),
            conn("pm.out", "rec.alpha"),
            conn("pr.out", "rec.alpha"),
        ],
    );
    let without = plan(
        receivers(),
        vec![conn("pm.out", "rec.alpha"), conn("pr.out", "rec.alpha")],
    );
    (with, without)
}

/// Each frame: sink inputs, the probes' effective values and outputs, as bits.
fn removal_trace(
    engine: &mut Engine,
    frames: u64,
    swap: Option<(u64, &ExecutionPlan)>,
) -> Vec<[u64; 6]> {
    let ports = [
        "/rec.zeta",
        "/rec.alpha",
        "/pm.value",
        "/pr.value",
        "/pm.out",
        "/pr.out",
    ]
    .map(port_addr);
    (0..frames)
        .map(|frame| {
            if let Some((at, plan)) = swap
                && at == frame
            {
                swap_at(engine, plan, frame);
            }
            engine.run_frame();
            engine.drain_log(&mut |_| {});
            [
                engine.signal_input(&ports[0]).unwrap(),
                engine.signal_input(&ports[1]).unwrap(),
                engine.parameter(&ports[2]).unwrap().effective,
                engine.parameter(&ports[3]).unwrap().effective,
                engine.signal_output(&ports[4]).unwrap(),
                engine.signal_output(&ports[5]).unwrap(),
            ]
            .map(f64::to_bits)
        })
        .collect()
}

/// AC4: removal as suspend at s, then a swap at s + F, fades the audio out
/// exactly like an ordinary suspend and introduces no discontinuity, also in
/// Multiply and Replace parameters whose only source was removed. Case A:
/// receivers running. Case B: the receivers are suspended across the source's
/// fade and the removal, then resume.
#[test]
fn removal_after_suspend_matches_an_ordinary_suspend() {
    let (with, without) = removal_plans();
    let frames = 10 + F + 3 * F + 10;

    // Case A.
    let s = 10;
    let mut suspended = engine(&with, RATE, vec![]);
    suspended.suspend(&addr("/o"), Frame(s)).unwrap();
    let mut removed = engine(&with, RATE, vec![]);
    removed.suspend(&addr("/o"), Frame(s)).unwrap();
    let at = removed.removal_swap_frame(Frame(s)).0;
    assert_eq!(at, s + F);
    let reference = removal_trace(&mut suspended, frames, None);
    assert_eq!(
        removal_trace(&mut removed, frames, Some((at, &without))),
        reference
    );
    // The parameters really were modulated: 0 now, not their base 0.5.
    assert_eq!(f64::from_bits(reference[(at + 1) as usize][2]), 0.0);
    assert_ne!(reference[(s - 1) as usize], reference[(at + 1) as usize]);

    // Case B: auditor-1's counterexample.
    let edits = |engine: &mut Engine| {
        for probe in ["/pm", "/pr"] {
            engine.suspend(&addr(probe), Frame(9)).unwrap();
            engine.resume(&addr(probe), Frame(16)).unwrap();
        }
        engine.suspend(&addr("/o"), Frame(10)).unwrap();
    };
    let mut suspended = engine(&with, RATE, vec![]);
    edits(&mut suspended);
    let mut removed = engine(&with, RATE, vec![]);
    edits(&mut removed);
    let reference = removal_trace(&mut suspended, frames, None);
    assert_eq!(
        removal_trace(&mut removed, frames, Some((15, &without))),
        reference
    );

    // A set releases the hold: the Multiply parameter returns to its base.
    removed
        .push_external(ext(frames, 1, "/pm.value", EventPayload::Set(0.25)))
        .unwrap();
    removed.run_frame();
    assert_eq!(
        removed
            .parameter(&port_addr("/pm.value"))
            .unwrap()
            .effective,
        0.25
    );
}

/// S3, cases C and D: a disconnect between survivors takes effect on the swap
/// frame, with no fade and no hold; when a parameter loses a removed source
/// and a surviving one, only the removed part is held. A rewired signal input
/// changes on the swap frame.
#[test]
fn connection_changes_between_survivors_take_effect_on_the_swap_frame() {
    // Case C: /k (0.25) → /pr.value (Replace, base 0.5), then disconnected.
    let connected = plan(
        vec![
            inst_with("k", "test.const", &[("value", 0.25)]),
            inst("pr", "test.probe_replace"),
            inst("rec", "test.recorder"),
        ],
        vec![conn("k.out", "pr.value"), conn("k.out", "rec.zeta")],
    );
    let rewired = plan(
        vec![
            inst_with("k", "test.const", &[("value", 0.25)]),
            inst("pr", "test.probe_replace"),
            inst("rec", "test.recorder"),
        ],
        vec![conn("k.out", "rec.alpha")],
    );
    let mut engine = engine(&connected, RATE, vec![]);
    let value = port_addr("/pr.value");
    let (zeta, alpha) = (port_addr("/rec.zeta"), port_addr("/rec.alpha"));
    for _ in 0..5 {
        engine.run_frame();
    }
    assert_eq!(engine.parameter(&value).unwrap().effective, 0.25);
    assert_eq!(engine.signal_input(&zeta), Some(0.25));
    swap_at(&mut engine, &rewired, 5);
    engine.run_frame();
    assert_eq!(engine.parameter(&value).unwrap().effective, 0.5);
    assert_eq!(engine.signal_input(&zeta), Some(0.0));
    assert_eq!(engine.signal_input(&alpha), Some(0.25));

    // Case D: /pm.value (Multiply, base 0.5) loses /o (removed after its
    // fade) and /k (0.25, survives, disconnected): the held modulation is the
    // removed part alone, +0.0, so the effective value is 0, not 0.5 (no hold)
    // or 0.125 (the surviving part kept).
    let both = plan(
        vec![
            inst_with("o", "test.osc", &[("freq", 130.0)]),
            inst_with("k", "test.const", &[("value", 0.25)]),
            inst("pm", "test.probe_mul"),
        ],
        vec![conn("o.out", "pm.value"), conn("k.out", "pm.value")],
    );
    let neither = plan(
        vec![
            inst_with("k", "test.const", &[("value", 0.25)]),
            inst("pm", "test.probe_mul"),
        ],
        vec![],
    );
    let mut engine = common::engine(&both, RATE, vec![]);
    engine.suspend(&addr("/o"), Frame(3)).unwrap();
    let at = engine.removal_swap_frame(Frame(3)).0;
    for _ in 0..at {
        engine.run_frame();
    }
    assert_eq!(
        engine.parameter(&port_addr("/pm.value")).unwrap().effective,
        0.125
    );
    swap_at(&mut engine, &neither, at);
    engine.run_frame();
    let effective = engine.parameter(&port_addr("/pm.value")).unwrap().effective;
    assert_eq!(effective.to_bits(), 0.0f64.to_bits());
}

/// S3: versions count from 0, and the swap record names both versions.
#[test]
fn versions_count_swaps_and_the_record_names_both() {
    let plan = plan(vec![inst("k", "test.const")], vec![]);
    let mut engine = engine(&plan, RATE, vec![]);
    assert_eq!(engine.version(), 0);
    let mut lines = Vec::new();
    for frame in 0..6 {
        if frame == 2 || frame == 4 {
            swap_at(&mut engine, &plan, frame);
        }
        engine.run_frame();
        lines.extend(drain_lines(&mut engine));
    }
    assert_eq!(engine.version(), 2);
    assert_eq!(
        lines,
        [
            r#"{"type":"swap","frame":2,"version":1,"plan_version":1}"#,
            r#"{"type":"swap","frame":4,"version":2,"plan_version":1}"#,
        ]
    );
}

/// S2: a commit is refused for another frame, for a stale or used swap, for a
/// second swap in one frame, and before the last frame's log is drained
/// (another engine's swap: `a_swap_from_another_engine_is_refused`).
#[test]
fn commit_refusals() {
    let plan = plan(vec![inst("p", "test.probe_add")], vec![]);
    let registry = registry();
    let events = vec![ext(0, 1, "/p.value", EventPayload::Set(0.25))];
    let mut engine = engine(&plan, RATE, events);
    let mut first = engine.prepare_swap(&plan, &registry).unwrap();
    let mut stale = engine.prepare_swap(&plan, &registry).unwrap();
    assert_eq!(
        engine.commit_swap(&mut first, Frame(1)),
        Err(EngineError::SwapFrame {
            frame: Frame(1),
            next: Frame(0)
        })
    );
    engine.commit_swap(&mut first, Frame(0)).unwrap();
    assert!(first.is_committed());
    assert_eq!(
        engine.commit_swap(&mut first, Frame(0)),
        Err(EngineError::StaleSwap)
    );
    assert_eq!(
        engine.commit_swap(&mut stale, Frame(0)),
        Err(EngineError::StaleSwap)
    );
    let mut second = engine.prepare_swap(&plan, &registry).unwrap();
    assert_eq!(
        engine.commit_swap(&mut second, Frame(0)),
        Err(EngineError::SwapPending)
    );
    engine.run_frame();
    assert_eq!(engine.frame_log().len(), 2);
    assert_eq!(
        engine.commit_swap(&mut second, Frame(1)),
        Err(EngineError::LogNotDrained)
    );
    engine.drain_log(&mut |_| {});
    engine.commit_swap(&mut second, Frame(1)).unwrap();
}

/// S2 (auditor-1's reproducer): a swap prepared on engine A, committed to an
/// engine B at the same version, is refused and moves nothing.
#[test]
fn a_swap_from_another_engine_is_refused() {
    let a = plan(vec![inst("a", "test.frames")], vec![]);
    let b = plan(
        vec![inst_with("b", "test.const", &[("value", 0.25)])],
        vec![],
    );
    let engine_a = engine(&a, RATE, vec![]);
    let mut engine_b = engine(&b, RATE, vec![]);
    let mut foreign = engine_a.prepare_swap(&a, &registry()).unwrap();
    engine_b.run_frame();
    engine_b.drain_log(&mut |_| {});
    assert_eq!(engine_a.version(), engine_b.version());
    assert_eq!(
        engine_b.commit_swap(&mut foreign, Frame(1)),
        Err(EngineError::ForeignSwap)
    );
    assert!(!foreign.is_committed());
    assert_eq!(engine_b.version(), 0);
    engine_b.run_frame();
    engine_b.drain_log(&mut |_| {});
    assert_eq!(engine_b.signal_output(&port_addr("/b.out")), Some(0.25));
    assert_eq!(engine_b.signal_output(&port_addr("/a.out")), None);

    // B's own swap to the same plan commits: /a is a fresh instance.
    let mut own = engine_b.prepare_swap(&a, &registry()).unwrap();
    engine_b.commit_swap(&mut own, Frame(2)).unwrap();
    engine_b.run_frame();
    engine_b.run_frame();
    assert_eq!(engine_b.signal_output(&port_addr("/a.out")), Some(1.0));
}

/// AC5: a ramp running across a swap, and one pending while suspended across
/// a swap, end exactly as without the swap.
#[test]
fn ramps_survive_a_swap() {
    let before = plan(vec![inst("p", "test.probe_add")], vec![]);
    let after = plan(
        vec![inst("p", "test.probe_add"), inst("k", "test.const")],
        vec![],
    );
    let events = vec![ext(10, 1, "/p.value", ramp(1.0, 100))];

    let mut unswapped = engine(&before, RATE, events.clone());
    let reference = base_trace(&mut unswapped, 130, "/p.value");
    let mut swapped = engine(&before, RATE, events);
    let mut trace = base_trace(&mut swapped, 50, "/p.value");
    swap_at(&mut swapped, &after, 50);
    trace.extend(base_trace(&mut swapped, 80, "/p.value"));
    assert_eq!(trace, reference);
    assert_ne!(reference[109], 1.0);
    assert_eq!(reference[110], 1.0);

    // Pending: suspended at 30 with the ramp half done, a new ramp held
    // pending, swapped at 40, resumed at 60.
    let pending = |engine: &mut Engine| {
        engine.suspend(&addr("/p"), Frame(30)).unwrap();
        engine.resume(&addr("/p"), Frame(60)).unwrap();
        engine
            .push_external(ext(35, 2, "/p.value", ramp(0.0, 20)))
            .unwrap();
    };
    let events = vec![ext(10, 1, "/p.value", ramp(1.0, 40))];
    let mut unswapped = engine(&before, RATE, events.clone());
    pending(&mut unswapped);
    let reference = base_trace(&mut unswapped, 100, "/p.value");
    let mut swapped = engine(&before, RATE, events);
    pending(&mut swapped);
    let mut trace = base_trace(&mut swapped, 40, "/p.value");
    swap_at(&mut swapped, &after, 40);
    trace.extend(base_trace(&mut swapped, 60, "/p.value"));
    assert_eq!(trace, reference);
    assert_eq!(reference[80], 0.0);
}

/// S6: `drain_log` gives the frame's records in the order `render` writes
/// them, and empties the log.
#[test]
fn drain_log_is_the_rendered_order() {
    let plan = rich_plan();
    let mut rendering = engine(&plan, RATE, rich_events());
    rich_edits(&mut rendering);
    let rendered = render(&mut rendering, 80).unwrap();

    let mut draining = engine(&plan, RATE, rich_events());
    rich_edits(&mut draining);
    let mut log = String::new();
    for _ in 0..80 {
        draining.run_frame();
        for line in drain_lines(&mut draining) {
            log.push_str(&line);
            log.push('\n');
        }
        assert!(draining.frame_log().is_empty());
    }
    assert_eq!(log.as_bytes(), rendered.log);
}

/// M2-2: `render_with` records exactly what a hand-driven run of the same
/// swaps and pushed input records, sink inputs and log alike.
#[test]
fn render_with_records_swaps_and_pushed_input() {
    let (with, without) = removal_plans();
    let script = |engine: &mut Engine, frame: u64| {
        if frame == 3 {
            engine
                .push_external(ext(3, 1, "/pm.value", EventPayload::Set(0.25)))
                .unwrap();
            engine.push_suspend(addr("/o"), Frame(3)).unwrap();
        }
        if frame == 3 + F {
            swap_at(engine, &without, frame);
        }
    };

    let mut driven = engine(&with, RATE, vec![]);
    let reference = run(&mut driven, 20, script);

    let mut rendering = engine(&with, RATE, vec![]);
    let rendered = render_with(&mut rendering, 20, |engine| {
        script(engine, engine.next_frame().0);
        Ok(())
    })
    .unwrap();
    let audio: Vec<(u64, u64)> = rendered
        .samples
        .chunks(2)
        .map(|c| (c[0].to_bits(), c[1].to_bits()))
        .collect();
    assert_eq!(audio, reference.audio);
    let lines: String = reference
        .lines
        .concat()
        .iter()
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(String::from_utf8(rendered.log).unwrap(), lines);
    assert!(lines.contains(r#""type":"swap""#));
    assert_eq!(rendering.input_discards(), InputDiscards::default());
}

/// M2-2: a swap that drops the audio sink stops `render_with`; an error from
/// the caller's step stops it too.
#[test]
fn render_with_refuses_a_lost_sink_and_reports_edit_errors() {
    let (with, _) = removal_plans();
    let no_sink = plan(vec![inst("o", "test.osc")], vec![]);
    let mut engine_a = engine(&with, RATE, vec![]);
    let error = render_with(&mut engine_a, 10, |engine| {
        if engine.next_frame() == Frame(2) {
            swap_at(engine, &no_sink, 2);
        }
        Ok(())
    })
    .unwrap_err();
    assert!(
        matches!(&error, RenderError::SinkChanged { before, after, channels: 2 }
            if *before == addr("/rec") && after.is_empty()),
        "{error}"
    );

    let mut engine_b = engine(&with, RATE, vec![]);
    let error = render_with(&mut engine_b, 10, |engine| {
        engine.commit_swap(&mut engine.prepare_swap(&with, &registry())?, Frame(99))
    })
    .unwrap_err();
    assert!(
        matches!(error, RenderError::Edit(EngineError::SwapFrame { .. })),
        "{error}"
    );
}

/// AC7: the same plans, swap frames and pushed events give the same bytes;
/// pushing events (in advance, or frame by frame) gives what the up-front list
/// gives, with sequence numbers that do not increase with the frame.
#[test]
fn runs_are_deterministic_and_pushing_equals_up_front() {
    let a = rich_plan();
    let b = plan(
        vec![
            inst("c", "test.counter"),
            inst("p", "test.probe_add"),
            inst_with("t", "test.trigger", &[("period", 3.0)]),
            inst("n", "test.noise"),
            inst("o", "test.osc"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("t.out", "c.in"),
            conn("n.out", "rec.zeta"),
            conn("o.out", "rec.alpha"),
            conn("p.out", "rec.alpha"),
        ],
    );
    let scenario = |events: Vec<ExternalEvent>, mode: u8| {
        let pushed = mode > 0;
        let mut engine = engine(&a, RATE, if pushed { Vec::new() } else { events.clone() });
        let mut by_frame = events;
        by_frame.sort_by_key(|e| e.frame);
        if mode == 1 {
            for event in by_frame.drain(..) {
                engine.push_external(event).unwrap();
            }
        }
        let run = run(&mut engine, 60, |engine, frame| {
            if mode == 2 {
                while by_frame.first().is_some_and(|e| e.frame.0 == frame) {
                    engine.push_external(by_frame.remove(0)).unwrap();
                }
            }
            if frame == 22 {
                swap_at(engine, &b, frame);
            }
            if frame == 41 {
                swap_at(engine, &a, frame);
            }
        });
        assert_eq!(engine.input_discards(), InputDiscards::default());
        run
    };
    let upfront = scenario(rich_events(), 0);
    assert_eq!(scenario(rich_events(), 0), upfront);
    assert_eq!(scenario(rich_events(), 1), upfront);
    assert_eq!(scenario(rich_events(), 2), upfront);
    assert!(
        upfront.lines[20]
            .iter()
            .filter(|l| l.contains("external"))
            .count()
            >= 3
    );

    // Auditor-1's pair: a larger sequence number before a smaller one.
    let pair = || {
        vec![
            ext(0, 10, "/p.value", EventPayload::Set(0.25)),
            ext(1, 1, "/p.value", EventPayload::Set(0.75)),
        ]
    };
    assert_eq!(scenario(pair(), 1), scenario(pair(), 0));
    assert_eq!(scenario(pair(), 2), scenario(pair(), 0));
}

/// AC8: an event or edit stamped for a frame that has already run applies on
/// the next frame, and its record carries that effective frame.
#[test]
fn late_input_applies_on_the_next_frame() {
    let plan = rich_plan();
    let reference = {
        let mut engine = engine(
            &plan,
            RATE,
            vec![ext(10, 1, "/p.value", EventPayload::Set(0.75))],
        );
        engine.suspend(&addr("/d"), Frame(10)).unwrap();
        run(&mut engine, 20, |_, _| {})
    };
    let mut engine = engine(&plan, RATE, vec![]);
    let late = run(&mut engine, 20, |engine, frame| {
        if frame == 10 {
            engine
                .push_external(ext(3, 1, "/p.value", EventPayload::Set(0.75)))
                .unwrap();
            engine.push_suspend(addr("/d"), Frame(3)).unwrap();
        }
    });
    assert_eq!(late, reference);
    assert!(
        late.lines[10]
            .iter()
            .any(|l| l.contains(r#""frame":10,"target":"/p.value""#))
    );
    assert!(late.lines[10].contains(&r#"{"type":"suspend","frame":10,"address":"/d"}"#.to_owned()));
}

/// S5: fixed-capacity queues: a full queue hands the input back; an event
/// stamped before the last queued one takes its frame; unknown targets and
/// non-finite values are counted, not logged; targets resolve against the
/// plan running at the effective frame; used input is reclaimed.
#[test]
fn push_queues() {
    let one = plan(
        vec![
            inst("p", "test.probe_add"),
            inst("d", "test.dc"),
            inst("g", "test.dc"),
        ],
        vec![],
    );
    let two = plan(
        vec![
            inst("p", "test.probe_add"),
            inst("d", "test.dc"),
            inst("k2", "test.const"),
        ],
        vec![],
    );
    let config = EngineConfig {
        queue_capacity: 3,
        ..EngineConfig::new(RATE, 1)
    };
    let mut engine = Engine::new(&one, &registry(), config, Vec::new()).unwrap();

    // Full: the event and the address come back.
    for (frame, sequence) in [(5, 1), (2, 2), (6, 3)] {
        engine
            .push_external(ext(frame, sequence, "/p.value", EventPayload::Set(0.25)))
            .unwrap();
    }
    let extra = ext(7, 4, "/p.value", EventPayload::Set(0.5));
    assert_eq!(
        engine.push_external(extra.clone()),
        Err(ExternalFull(extra))
    );
    for frame in [1, 2, 4] {
        engine.push_suspend(addr("/g"), Frame(frame)).unwrap();
    }
    assert_eq!(
        engine.push_resume(addr("/g"), Frame(9)),
        Err(EditFull(addr("/g")))
    );

    let mut lines = Vec::new();
    for _ in 0..7 {
        engine.run_frame();
        lines.extend(drain_lines(&mut engine));
    }
    // Sequence 2, stamped 2 after one stamped 5, takes effect at 5.
    let events: Vec<_> = lines.iter().filter(|l| l.contains("/p.value")).collect();
    assert_eq!(events.len(), 3);
    assert!(events[0].contains(r#""frame":5"#) && events[0].contains(r#""sequence":1"#));
    assert!(events[1].contains(r#""frame":5"#) && events[1].contains(r#""sequence":2"#));
    // Three suspends of /g: one applies, two do not fit its state.
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains(r#""address":"/g""#))
            .count(),
        1
    );
    assert_eq!(
        engine.input_discards(),
        InputDiscards {
            unrouted: 0,
            invalid: 2
        }
    );

    // Used input occupies the queue until reclaimed.
    assert!(
        engine
            .push_external(ext(9, 5, "/p.value", EventPayload::Set(0.5)))
            .is_err()
    );
    assert_eq!(engine.reclaim_externals().count(), 3);
    assert_eq!(engine.reclaim_edits().count(), 3);

    // Unknown target and non-finite value: counted, not logged. An event for
    // an instance a later swap adds resolves; an edit for a removed one does
    // not.
    let next = engine.next_frame().0;
    engine
        .push_external(ext(next, 6, "/nope.in", EventPayload::Trigger))
        .unwrap();
    engine
        .push_external(ext(next, 7, "/p.value", EventPayload::Set(f64::NAN)))
        .unwrap();
    engine
        .push_external(ext(next + 1, 8, "/k2.value", EventPayload::Set(2.0)))
        .unwrap();
    engine.run_frame();
    assert!(drain_lines(&mut engine).is_empty());
    assert_eq!(
        engine.input_discards(),
        InputDiscards {
            unrouted: 1,
            invalid: 3
        }
    );
    engine.push_suspend(addr("/g"), Frame(next + 1)).unwrap();
    swap_at(&mut engine, &two, next + 1);
    engine.run_frame();
    assert_eq!(
        drain_lines(&mut engine),
        [
            format!(
                r#"{{"type":"swap","frame":{},"version":1,"plan_version":1}}"#,
                next + 1
            ),
            format!(
                r#"{{"type":"event","frame":{},"target":"/k2.value","payload":{{"set":2.0}},"source":{{"external":{{"sequence":8}}}}}}"#,
                next + 1
            ),
        ]
    );
    assert_eq!(engine.parameter(&port_addr("/k2.value")).unwrap().base, 2.0);
    assert_eq!(
        engine.input_discards(),
        InputDiscards {
            unrouted: 2,
            invalid: 3
        }
    );
}

/// S5: pushed and scheduled edits mix: a pushed edit need not come after a
/// scheduled future one, and once applied it is what `suspend` and `resume`
/// check against.
#[test]
fn pushed_and_scheduled_edits_mix() {
    let plan = plan(vec![inst("d", "test.dc"), inst("e", "test.dc")], vec![]);
    let mut engine = engine(&plan, RATE, vec![]);
    engine.suspend(&addr("/d"), Frame(50)).unwrap();
    engine.push_suspend(addr("/e"), Frame(5)).unwrap();
    engine.push_suspend(addr("/d"), Frame(10)).unwrap();
    engine.push_resume(addr("/d"), Frame(20)).unwrap();
    let mut lines = Vec::new();
    for _ in 0..60 {
        engine.run_frame();
        lines.extend(drain_lines(&mut engine));
    }
    assert_eq!(
        lines,
        [
            r#"{"type":"suspend","frame":5,"address":"/e"}"#,
            r#"{"type":"suspend","frame":10,"address":"/d"}"#,
            r#"{"type":"resume","frame":20,"address":"/d","discarded":0}"#,
            r#"{"type":"suspend","frame":50,"address":"/d"}"#,
        ]
    );
    // /e was suspended by a push: the scheduling check knows it.
    assert!(matches!(
        engine.suspend(&addr("/e"), Frame(70)),
        Err(EngineError::InvalidEdit { .. })
    ));
    engine.resume(&addr("/e"), Frame(70)).unwrap();
    assert_eq!(engine.input_discards(), InputDiscards::default());
}

/// M2.6-2 AC5 (ADR 0019 §2): on 4 threads, every segment on the workers,
/// swaps between the rich plan and a larger one (each swap's task graph is
/// built by `prepare_swap`, on the calling thread here as on the control
/// thread live) give the one-thread render bit for bit, and the engine's
/// worker pool is the one it started with: no thread is spawned by a swap.
#[test]
fn swaps_on_workers_rebuild_tasks_and_keep_the_pool() {
    let a = rich_plan();
    // The same instances and connections as `rich_plan`, plus `o2`.
    let b = {
        let mut connections = vec![scaled("o2.out", "rec.zeta", 0.25)];
        connections.extend(rich_connections());
        let mut instances = rich_instances();
        instances.push(inst_with("o2", "test.osc", &[("freq", 330.0)]));
        plan(instances, connections)
    };
    let render = |threads: usize| {
        let config = common::config_threads(RATE, 42, threads);
        let mut engine = Engine::new(&a, &registry(), config, rich_events()).unwrap();
        let started = signal_engine::spawned_workers();
        let workers = engine.schedule_stats().workers;
        let mut swaps = 0;
        let rendered = signal_output::render_blocks(&mut engine, 2_000, 64, |engine| {
            let now = engine.next_frame().0;
            if now > 0 && now % 100 == 0 {
                let next = if swaps % 2 == 0 { &b } else { &a };
                let mut swap = engine.prepare_swap(next, &registry())?;
                engine.commit_swap(&mut swap, Frame(now))?;
                swaps += 1;
            }
            Ok(Some(Frame(now - now % 100 + 100)))
        })
        .unwrap();
        assert_eq!(engine.version(), 19);
        let stats = engine.schedule_stats();
        assert_eq!(stats.workers, workers);
        assert_eq!(signal_engine::spawned_workers(), started, "no swap spawns");
        (rendered.samples, rendered.log, stats)
    };
    let (samples, log, one) = render(1);
    let (parallel_samples, parallel_log, four) = render(4);
    assert_eq!(one.workers, 0);
    assert_eq!(four.workers, 3);
    assert!(
        four.parallel_segments > 0 && four.worker_tasks > 0,
        "{four:?}"
    );
    assert!(
        parallel_samples == samples,
        "the audio differs on 4 threads"
    );
    assert!(parallel_log == log, "the log differs on 4 threads");
}

/// `LogRecord::Swap` is public for real-time callers that keep records.
#[test]
fn swap_records_come_first_in_drain_order() {
    let plan = plan(vec![inst("p", "test.probe_add")], vec![]);
    let mut engine = engine(
        &plan,
        RATE,
        vec![ext(0, 1, "/p.value", EventPayload::Set(0.5))],
    );
    swap_at(&mut engine, &plan, 0);
    engine.run_frame();
    let mut records = Vec::new();
    engine.drain_log(&mut |r| records.push(*r));
    assert!(matches!(
        records.as_slice(),
        [
            LogRecord::Swap {
                frame: Frame(0),
                version: 1,
                plan_version: 1
            },
            LogRecord::Event { .. }
        ]
    ));
}
