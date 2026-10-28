//! ADR 0037 §6 against the oracle: `Inputs::sources_changed`, a playhead
//! following another (`std.playhead.follow`, its switch `jump` and
//! `release`) and `std.prefer.switched`, across swaps that connect, rewire
//! to another source and disconnect, run by the oracle and by the block
//! engine, which must match bit for bit (audio, log, end state) for every
//! block size, partition and thread count.

mod common;

use std::collections::BTreeMap;

use common::{Setup, compare, graph_of, registry, run_engine};
use signal_core::EventPayload;
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_oracle::{ExternalEvent, Script, Swap};

fn instance(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect::<BTreeMap<_, _>>(),
        scope: None,
    }
}

fn wire(from: &str, to: &str, scale: Option<f64>) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale,
    }
}

/// Two host playheads `h1` (120 bpm) and `h2` (97 bpm); a deck playhead
/// `deck` (133 bpm on its own) following `follow` (`None`: nothing) with
/// that host's jumps; a `std.prefer` `pf` of `prefer` (else the deck's
/// position); counters of the deck's jumps and of `pf.switched`; all heard
/// on the sink, scaled down.
fn plan(follow: Option<&str>, prefer: Option<&str>, version: u64) -> ExecutionPlan {
    let mut connections = vec![
        wire("deck.position", "out.l", Some(1e-3)),
        wire("deck.jump", "jumps.trigger", None),
        wire("pf.switched", "switches.trigger", None),
        wire("deck.position", "pf.b", None),
        wire("jumps.count", "out.r", Some(1e-2)),
        wire("switches.count", "out.r", Some(1e-1)),
        wire("pf.out", "out.r", Some(1e-3)),
        wire("deck.tempo", "out.r", Some(1e-5)),
        wire("deck.running", "out.l", Some(1e-2)),
    ];
    if let Some(host) = follow {
        connections.push(wire(&format!("{host}.position"), "deck.follow", None));
        connections.push(wire(&format!("{host}.jump"), "deck.follow_jump", None));
    }
    if let Some(host) = prefer {
        connections.push(wire(&format!("{host}.position"), "pf.a", None));
    }
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            instance("h1", "std.playhead", &[("bpm", 120.0)]),
            instance("h2", "std.playhead", &[("bpm", 97.0)]),
            instance("deck", "std.playhead", &[("bpm", 133.0)]),
            instance("pf", "std.prefer", &[]),
            instance("jumps", "std.counter", &[]),
            instance("switches", "std.counter", &[]),
            instance("out", "std.audio_out", &[]),
        ],
        connections,
        exports: Exports::default(),
    };
    let mut plan = build_plan(&root, &[], &registry(), 0).unwrap();
    plan.version = version;
    plan
}

/// Follow h1 from the start; rewire to h2 at 300 (`sources_changed`), and
/// `pf.a` connected; `release` set to 1 at 650 and let go at 700 (return);
/// follow h1 again at 1100, `pf.a` rewired to h1; a seek on h1 at 1200
/// while followed; `release` set to 0 at 1400 and let go at 1600
/// (continue).
fn script() -> (ExecutionPlan, Vec<ExecutionPlan>, Script) {
    let first = plan(Some("h1"), None, 0);
    let swaps = vec![
        (300, plan(Some("h2"), Some("h2"), 1)),
        (700, plan(None, Some("h2"), 2)),
        (1100, plan(Some("h1"), Some("h1"), 3)),
        (1600, plan(None, None, 4)),
    ];
    let script = Script {
        sample_rate: 1000,
        seed: 5,
        frames: 2000,
        events: vec![
            ExternalEvent {
                frame: 1200,
                sequence: 3,
                target: "/h1.seek".parse().unwrap(),
                payload: EventPayload::Set(40.0),
            },
            ExternalEvent {
                frame: 650,
                sequence: 1,
                target: "/deck.release".parse().unwrap(),
                payload: EventPayload::Set(1.0),
            },
            ExternalEvent {
                frame: 1400,
                sequence: 2,
                target: "/deck.release".parse().unwrap(),
                payload: EventPayload::Set(0.0),
            },
        ],
        edits: Vec::new(),
        swaps: swaps
            .iter()
            .map(|(frame, plan)| Swap {
                frame: *frame,
                graph: graph_of(plan, *frame),
            })
            .collect(),
    };
    (first, swaps.into_iter().map(|(_, p)| p).collect(), script)
}

#[test]
fn following_and_switching_match_the_oracle_in_every_configuration() {
    let registry = registry();
    let (first, swaps, script) = script();
    let oracle = signal_oracle::run(&registry, &graph_of(&first, 1), &script).unwrap();
    let mut setups = Vec::new();
    for block in [1, 3, 64, 256] {
        for threads in [1, 2, 4] {
            setups.push(Setup {
                block,
                threads,
                splits: vec![150, 301, 699, 1101, 1599],
            });
        }
    }
    for setup in &setups {
        let engine = run_engine(&registry, &first, &swaps, &script, setup, &oracle);
        if let Err(problem) = compare(&oracle, &engine) {
            panic!("{setup:?}: {problem}");
        }
    }
    // The run is not trivially equal: the sink moves on every frame.
    assert!(oracle.samples.windows(4).any(|w| w[0] != w[2]));
}

/// The deck's position, jumps and switches per frame, read from the engine
/// through its outputs: the playhead copies the followed host exactly, a
/// rewire is one jump, a release with 0 continues without a step and with 1
/// returns to the own clock.
#[test]
fn the_deck_follows_switches_and_releases() {
    let registry = registry();
    let (first, swaps, script) = script();
    let mut engine = signal_engine::Engine::new(
        &first,
        &registry,
        signal_engine::EngineConfig::new(script.sample_rate, script.seed),
        script
            .events
            .iter()
            .map(|e| signal_engine::ExternalEvent {
                frame: signal_core::Frame(e.frame),
                sequence: e.sequence,
                target: e.target.clone(),
                payload: e.payload,
            })
            .collect(),
    )
    .unwrap();
    let names = [
        "/deck.position",
        "/h1.position",
        "/h2.position",
        "/jumps.count",
        "/switches.count",
        "/pf.out",
        "/deck.running",
        "/deck.tempo",
    ];
    let mut trace: Vec<[f64; 8]> = Vec::new();
    while engine.next_frame().0 < script.frames {
        let now = engine.next_frame().0;
        if let Some(k) = script.swaps.iter().position(|s| s.frame == now) {
            engine.drain_log(&mut |_| {});
            let mut prepared = engine.prepare_swap(&swaps[k], &registry).unwrap();
            engine
                .commit_swap(&mut prepared, signal_core::Frame(now))
                .unwrap();
        }
        assert_eq!(engine.run_block(1), 1);
        let mut row = [0.0; 8];
        for (value, name) in row.iter_mut().zip(names) {
            let index = engine.signal_output_index(&name.parse().unwrap()).unwrap();
            *value = engine.signal_output_at(index, 0).unwrap();
        }
        trace.push(row);
        engine.drain_log(&mut |_| {});
    }
    let (deck, h1, h2, jumps, switches) = (0, 1, 2, 3, 4);
    let own = |n: usize| (n as f64 * 133.0) / 60_000.0;
    // Following h1 from frame 0: an exact copy; one jump on frame 0 (starts
    // following), none after.
    for (n, row) in trace.iter().enumerate().take(300) {
        assert_eq!(row[deck].to_bits(), row[h1].to_bits(), "frame {n}");
    }
    assert_eq!(trace[0][jumps], 1.0);
    assert_eq!(trace[299][jumps], 1.0);
    // Rewired to h2 at 300: a copy of h2 from that frame, one jump there.
    for (n, row) in trace.iter().enumerate().take(700).skip(300) {
        assert_eq!(row[deck].to_bits(), row[h2].to_bits(), "frame {n}");
    }
    assert_eq!((trace[299][jumps], trace[300][jumps]), (1.0, 2.0));
    // `pf.a` connected at 300: one switch; `out` is h2's position.
    assert_eq!((trace[299][switches], trace[300][switches]), (0.0, 1.0));
    assert_eq!(trace[300][5].to_bits(), trace[300][h2].to_bits());
    // Let go at 700 with release 1: the position returns to the own clock,
    // which kept running underneath from frame 0 (133 bpm): an unfollowed
    // playhead's position on that frame; one jump.
    assert_eq!(trace[700][deck], own(700));
    assert_eq!(trace[1099][deck], own(1099));
    assert_eq!(trace[700][jumps], 3.0);
    // Following h1 again at 1100 (and pf.a rewired to h1: one switch).
    assert_eq!(trace[1100][deck].to_bits(), trace[1100][h1].to_bits());
    assert_eq!(trace[1100][jumps], 4.0);
    assert_eq!(trace[1100][switches], 2.0);
    // h1's seek at 1200 reaches the deck through follow_jump: one jump, as
    // forwarded (no second one on that frame).
    assert_eq!(trace[1200][deck], 40.0);
    assert_eq!((trace[1199][jumps], trace[1200][jumps]), (4.0, 5.0));
    // Let go at 1600 with release 0: the position goes on from h1's at 1599
    // (no step on 1600), at the deck's own 133 bpm; one jump.
    assert_eq!(trace[1600][deck], trace[1599][h1]);
    assert_eq!(
        trace[1601][deck],
        trace[1599][h1] + (1.0 * 133.0) / 60_000.0
    );
    assert_eq!(trace[1600][jumps], 6.0);
    // pf.a disconnected at 1600: the deck's own position, one switch.
    assert_eq!(trace[1600][switches], 3.0);
    assert_eq!(trace[1600][5].to_bits(), trace[1600][deck].to_bits());
    // Following without follow_running and follow_tempo: running is the
    // motion, tempo the motion per minute (0 on the first followed frame).
    assert_eq!(trace[0][6], 0.0);
    assert_eq!(trace[10][6], 1.0);
    assert!((trace[10][7] - 120.0).abs() < 1e-6, "{}", trace[10][7]);
}
