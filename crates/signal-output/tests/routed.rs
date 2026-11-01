//! Several play sides (ADR 0038, M4.1): a run of a plan with two audio sinks,
//! `main` (`/out`) and `cue` (`/cue`), each with its own `gain`. The Runner's
//! audio frame is their channels in the routing's canonical order; each
//! sink's channels are exactly what a run of that sink alone plays; the
//! offline render records the main sink in `audio.wav` exactly as a
//! one-sink render does and the other beside it.

use std::collections::BTreeMap;

use signal_core::{InstanceAddress, Registry};
use signal_engine::{Engine, EngineConfig, RealtimeConfig};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::{
    Driver, MAIN, NullDriver, PlaySide, RoleBinding, SoundSystem, realtime, realtime_routed,
    render, render_sides, route,
};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

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

fn connection(from: &str, to: &str) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    }
}

/// `a` (220 Hz) into `out` at gain 0.5, `b` (330 Hz) into `cue` at gain
/// 0.25; with `cue` false, `out` alone.
fn plan(registry: &Registry, cue: bool) -> ExecutionPlan {
    let mut instances = vec![
        instance("a", "std.sine_osc", &[("freq", 220.0)]),
        instance("out", "std.audio_out", &[("gain", 0.5)]),
    ];
    let mut connections = vec![connection("a.out", "out.l"), connection("a.out", "out.r")];
    if cue {
        instances.push(instance("b", "std.sine_osc", &[("freq", 330.0)]));
        instances.push(instance("cue", "std.audio_out", &[("gain", 0.25)]));
        connections.push(connection("b.out", "cue.l"));
        connections.push(connection("b.out", "cue.r"));
    }
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, 0).unwrap()
}

fn address(text: &str) -> InstanceAddress {
    text.parse().unwrap()
}

const FRAMES: u64 = 4_800;

fn play(runner: signal_output::Runner, channels: usize) -> Vec<f32> {
    let mut driver = NullDriver::new(48_000, channels, 11);
    driver.start(runner).unwrap();
    driver.run(FRAMES, || {}).unwrap();
    driver.take_output()
}

#[test]
fn two_sinks_play_concatenated_in_the_routings_order() {
    let registry = registry();
    let both = plan(&registry, true);
    let sides = [
        PlaySide {
            address: address("/out"),
            role: MAIN.to_owned(),
            width: 2,
        },
        PlaySide {
            address: address("/cue"),
            role: "cue".to_owned(),
            width: 2,
        },
    ];
    let system = SoundSystem {
        roles: [("cue".to_owned(), RoleBinding::clock(vec![3, 4]))].into(),
    };
    let routing = route(&sides, &system, Some(4));
    assert_eq!(routing.addresses(), [address("/cue"), address("/out")]);
    let (runner, _controller) = realtime_routed(
        &both,
        &registry,
        RealtimeConfig::new(48_000, 1),
        None,
        &routing.addresses(),
    )
    .unwrap();
    assert_eq!(runner.width(), 4);
    let frame = play(runner, 4);

    // `/out` alone, as every run before ADR 0038 played it.
    let alone = plan(&registry, false);
    let (runner, _controller) =
        realtime(&alone, &registry, RealtimeConfig::new(48_000, 1)).unwrap();
    assert_eq!(runner.width(), 2);
    let main = play(runner, 2);

    for f in 0..FRAMES as usize {
        // Frame order: cue l, cue r, main l, main r.
        assert_eq!(frame[f * 4 + 2], main[f * 2], "main l at {f}");
        assert_eq!(frame[f * 4 + 3], main[f * 2 + 1], "main r at {f}");
        assert_eq!(frame[f * 4], frame[f * 4 + 1]);
    }
    let cue_peak = (0..FRAMES as usize).fold(0.0f32, |p, f| p.max(frame[f * 4].abs()));
    assert!(
        (cue_peak - 0.25).abs() < 1e-3,
        "cue at its gain: {cue_peak}"
    );
}

#[test]
fn the_main_sink_renders_as_before_and_the_cue_beside_it() {
    let registry = registry();
    let config = EngineConfig::new(48_000, 1);
    let mut engine = Engine::new(&plan(&registry, true), &registry, config, Vec::new()).unwrap();
    let (main, sides) =
        render_sides(&mut engine, FRAMES, &address("/out"), &[address("/cue")]).unwrap();
    let mut alone = Engine::new(&plan(&registry, false), &registry, config, Vec::new()).unwrap();
    let before = render(&mut alone, FRAMES).unwrap();
    assert_eq!(main.samples, before.samples);
    assert_eq!(main.audio_fnv1a64(), before.audio_fnv1a64());
    assert_eq!(sides.len(), 1);
    assert_eq!(sides[0].address, address("/cue"));
    assert_eq!(sides[0].channels, 2);
    assert_eq!(sides[0].samples.len() as u64, FRAMES * 2);
    // Several sinks are still refused by the one-sink render.
    let mut engine = Engine::new(&plan(&registry, true), &registry, config, Vec::new()).unwrap();
    assert!(render(&mut engine, FRAMES).is_err());
}
