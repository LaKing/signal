//! Sleep groups (ADR 0044 §11, `docs/semantics.md` §17), test 13: a group
//! of a sine through a unit delay into a VCA under an envelope, driven by
//! a constant the script sets. It falls asleep at multiples of *R* after
//! *R* quiet frames, wakes on its driver and on a trigger to the
//! envelope's event input but not on a ramp of a member's parameter (which
//! keeps advancing), reads exactly +0.0 at its boundary while asleep, and
//! composes with suspension and swaps (one that keeps the group, one that
//! changes *R*). The engine equals the oracle bit for bit over blocks of
//! 1 to 256 frames on 1 to 4 threads.

mod common;

use common::{Setup, compare, graph_of, registry, run_engine};
use signal_core::{Curve, EventPayload, Frame, InstanceAddress, PortAddress};
use signal_engine::{Engine, EngineConfig};
use signal_graph::{
    CompositeDef, ConnectionDef, ExecutionPlan, Exports, InputExport, InstanceDef, OutputExport,
    ParameterExport, SleepDef, build_plan,
};
use signal_oracle::{Edit, ExternalEvent, Script, Swap};

const RATE: u32 = 48_000;
const FRAMES: u64 = 9_000;

fn instance(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect(),
        scope: None,
    }
}

fn wire(from: &str, to: &str, scale: f64) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: Some(scale),
    }
}

/// The member composite: `osc` through the state-output `dly` into `vca`,
/// times `env` (τ of its release: 24 frames, exactly 0 about 500 frames
/// after the gate falls).
fn member() -> CompositeDef {
    CompositeDef {
        seed: None,
        name: "member".to_owned(),
        instances: vec![
            instance("osc", "std.sine_osc", &[("freq", 220.0)]),
            instance("dly", "std.unit_delay", &[]),
            instance(
                "env",
                "std.adsr",
                &[
                    ("attack_ms", 1.0),
                    ("decay_ms", 5.0),
                    ("sustain", 0.5),
                    ("release_ms", 0.5),
                ],
            ),
            instance("vca", "std.mul", &[]),
        ],
        connections: vec![
            wire("osc.out", "dly.in", 1.0),
            wire("dly.out", "vca.a", 1.0),
            wire("env.out", "vca.b", 1.0),
        ],
        exports: Exports {
            inputs: vec![InputExport {
                name: "gate".to_owned(),
                to: vec!["env.gate".to_owned()],
            }],
            outputs: vec![OutputExport {
                name: "out".to_owned(),
                from: "vca.out".to_owned(),
            }],
            parameters: vec![ParameterExport {
                name: "freq".to_owned(),
                to: vec!["osc.freq".to_owned()],
                range: None,
                values: None,
            }],
        },
        sleep: Vec::new(),
    }
}

/// The root: `drv` drives the group `grp` and gates its envelope.
fn plan(rest: u64, version: u64) -> ExecutionPlan {
    let root = CompositeDef {
        seed: None,
        name: "project".to_owned(),
        instances: vec![
            instance("drv", "std.const", &[("value", 0.0)]),
            instance("grp", "member", &[]),
            instance("out", "std.audio_out", &[]),
        ],
        connections: vec![
            wire("drv.out", "grp.gate", 1.0),
            wire("grp.out", "out.l", 1.0),
            wire("grp.out", "out.r", 0.5),
        ],
        exports: Exports::default(),
        sleep: vec![SleepDef {
            members: "grp".to_owned(),
            driver: "drv.out".to_owned(),
            rest,
        }],
    };
    let mut plan = build_plan(&root, &[member()], &registry(), 0).unwrap();
    plan.version = version;
    plan
}

fn port(text: &str) -> PortAddress {
    text.parse().unwrap()
}

fn event(frame: u64, sequence: u64, target: &str, payload: EventPayload) -> ExternalEvent {
    ExternalEvent {
        frame,
        sequence,
        target: port(target),
        payload,
    }
}

fn edit(frame: u64, instance: &str, resume: bool) -> Edit {
    Edit {
        frame,
        instance: instance.parse().unwrap(),
        resume,
    }
}

/// The script: the gate on at 0 and off at 400 (asleep before 1100); a
/// ramp of `osc.freq` from 1500 over 600 frames (no wake); a trigger to
/// the envelope at 2600 (wakes); the driver at 0.7 from 4000 (wakes) and
/// 0.3 from 4300; `vca` suspended over 4100–4200 awake, `osc` over
/// 6000–6600 asleep; a swap at 5000 that keeps the group and one at 7000
/// with *R* = 128 (it starts awake).
fn script() -> (Script, Vec<ExecutionPlan>) {
    let swaps = vec![(5000, plan(64, 1)), (7000, plan(128, 2))];
    let script = Script {
        sample_rate: RATE,
        seed: 5,
        frames: FRAMES,
        events: vec![
            event(0, 1, "/drv.value", EventPayload::Set(1.0)),
            event(400, 2, "/drv.value", EventPayload::Set(0.0)),
            event(
                1500,
                3,
                "/grp/osc.freq",
                EventPayload::Ramp {
                    to: 880.0,
                    frames: 600,
                    curve: Curve::Linear,
                },
            ),
            event(2600, 4, "/grp/env.trigger", EventPayload::Trigger),
            event(4000, 5, "/drv.value", EventPayload::Set(0.7)),
            event(4300, 6, "/drv.value", EventPayload::Set(0.3)),
        ],
        edits: vec![
            edit(4100, "/grp/vca", false),
            edit(4200, "/grp/vca", true),
            edit(6000, "/grp/osc", false),
            edit(6600, "/grp/osc", true),
        ],
        swaps: swaps
            .iter()
            .map(|(frame, plan)| Swap {
                frame: *frame,
                graph: graph_of(plan, *frame),
            })
            .collect(),
        inputs: Vec::new(),
    };
    (script, swaps.into_iter().map(|(_, p)| p).collect())
}

#[test]
fn the_engine_sleeps_as_the_oracle_does_in_any_partition() {
    let registry = registry();
    let first = plan(64, 0);
    let (script, plans) = script();
    let oracle = signal_oracle::run(&registry, &graph_of(&first, 1), &script).unwrap();
    for block in [1, 3, 64, 100, 256] {
        for threads in [1, 2, 4] {
            let setup = Setup {
                block,
                threads,
                splits: vec![63, 1000, 2600, 4001, 6999],
            };
            let engine = run_engine(&registry, &first, &plans, &script, &setup, &oracle);
            if let Err(problem) = compare(&oracle, &engine) {
                panic!("{setup:?}: {problem}");
            }
        }
    }
}

/// Frame by frame in the engine: whether the group is asleep after each
/// frame, and the bits of its boundary output.
fn timeline() -> Vec<(bool, u64)> {
    let registry = registry();
    let (script, _) = script();
    let upfront = script
        .events
        .iter()
        .filter(|e| e.frame < 5000)
        .map(|e| signal_engine::ExternalEvent {
            frame: Frame(e.frame),
            sequence: e.sequence,
            target: e.target.clone(),
            payload: e.payload,
        })
        .collect();
    let mut engine =
        Engine::new(&plan(64, 0), &registry, EngineConfig::new(RATE, 5), upfront).unwrap();
    let group: InstanceAddress = "/grp".parse().unwrap();
    let out = port("/grp/vca.out");
    (0..5000)
        .map(|_| {
            engine.run_frame();
            let asleep = engine.is_asleep(&group).unwrap();
            (asleep, engine.signal_output(&out).unwrap().to_bits())
        })
        .collect()
}

#[test]
fn a_group_sleeps_at_multiples_of_r_and_wakes_on_its_driver_and_events() {
    let frames = timeline();
    // Asleep from frame g (decided after frame g − 1) only at multiples of 64.
    let falls: Vec<usize> = (1..frames.len())
        .filter(|&n| frames[n].0 && !frames[n - 1].0)
        .map(|n| n + 1)
        .collect();
    assert!(falls.len() >= 2, "{falls:?}");
    assert!(falls.iter().all(|g| g % 64 == 0), "{falls:?}");
    // Exactly +0.0 at the boundary on every frame it sleeps.
    let asleep_frames = |n: usize| n > 0 && frames[n - 1].0;
    for n in (1..frames.len()).filter(|&n| asleep_frames(n) && frames[n].0) {
        assert_eq!(frames[n].1, 0, "frame {n}");
    }
    // Asleep before the ramp and through it: a parameter event wakes nothing.
    assert!((1200..2600).all(|n| frames[n].0), "asleep over the ramp");
    // The trigger at 2600 and the driver at 4000 wake it on their frames.
    assert!(!frames[2600].0 && frames[2599].0);
    assert!(
        (2600..2610).any(|n| frames[n].1 != 0),
        "the trigger's attack sounds"
    );
    assert!(frames[3999].0 && !frames[4000].0);
}
