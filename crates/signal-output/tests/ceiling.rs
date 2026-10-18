//! The output ceiling (`Runner::set_ceiling`, `signal mcp`'s `--max-gain`,
//! M2.12-1): every device sample is held within ±ceiling whatever the sink's
//! inputs and gain are (a connection scaled ×20 included), the held samples
//! are counted and metered as held, and without a ceiling the device gets
//! exactly what it got before. A full-scale sine scaled ×20 into a sink of gain
//! 0.2 is the input.

use std::collections::BTreeMap;

use signal_core::Registry;
use signal_engine::{Engine, EngineConfig, RealtimeConfig};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::{Driver, NullDriver, device_sample, hold, realtime, render};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// `osc` (440 Hz, full scale) into `out.l` and `out.r` scaled by
/// `scale`; `out.gain` = `gain`.
fn plan(registry: &Registry, scale: f64, gain: f64) -> ExecutionPlan {
    let connection = |to: &str| ConnectionDef {
        from: "osc.out".to_owned(),
        to: to.to_owned(),
        scale: Some(scale),
    };
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            InstanceDef {
                name: "osc".to_owned(),
                module: "std.sine_osc".to_owned(),
                parameters: BTreeMap::from([("freq".to_owned(), 440.0)]),
            },
            InstanceDef {
                name: "out".to_owned(),
                module: "std.audio_out".to_owned(),
                parameters: BTreeMap::from([("gain".to_owned(), gain)]),
            },
        ],
        connections: vec![connection("out.l"), connection("out.r")],
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, 0).unwrap()
}

const FRAMES: u64 = 4_800;

/// What the null driver (the device) receives with `ceiling`, the held
/// count and the meters' highest peak.
fn played(plan: &ExecutionPlan, registry: &Registry, ceiling: Option<f64>) -> (Vec<f32>, u64, f32) {
    let (mut runner, mut controller) =
        realtime(plan, registry, RealtimeConfig::new(48_000, 1)).unwrap();
    runner.set_ceiling(ceiling);
    let mut driver = NullDriver::new(48_000, 2, 3);
    driver.start(runner).unwrap();
    let mut peak = 0.0f32;
    driver
        .run(FRAMES, || {
            controller.poll_with(&mut (), |m| peak = peak.max(m.peak));
        })
        .unwrap();
    controller.poll_with(&mut (), |m| peak = peak.max(m.peak));
    let held = controller.ceilinged();
    (driver.take_output(), held, peak)
}

#[test]
fn a_scaled_connection_never_passes_the_ceiling() {
    let registry = registry();
    let plan = plan(&registry, 20.0, 0.2);
    let (out, held, peak) = played(&plan, &registry, Some(0.25));
    assert_eq!(out.len() as u64, FRAMES * 2);
    let highest = out.iter().fold(0.0f32, |p, x| p.max(x.abs()));
    assert_eq!(highest, 0.25, "held at the ceiling, and reaching it");
    assert!(held > FRAMES, "most samples are held: {held}");
    assert_eq!(peak, 0.25, "the meters measure what the device gets");
    // Without the ceiling the same plan plays full scale (the device's own
    // clamp), as before.
    let (open, none, _) = played(&plan, &registry, None);
    assert_eq!(none, 0);
    assert_eq!(open.iter().fold(0.0f32, |p, x| p.max(x.abs())), 1.0);
    // The held output is the open one held, sample for sample.
    let mut count = 0;
    let rebuilt: Vec<f32> = open
        .iter()
        .map(|&x| hold(x, Some(0.25), &mut count))
        .collect();
    assert_eq!(rebuilt, out);
    assert_eq!(count, held);
}

#[test]
fn below_the_ceiling_nothing_changes_and_renders_never_hold() {
    let registry = registry();
    let plan = plan(&registry, 1.0, 0.2);
    let (held_out, held, _) = played(&plan, &registry, Some(0.25));
    let (open, _, _) = played(&plan, &registry, None);
    assert_eq!(held, 0);
    assert_eq!(held_out, open, "under the ceiling the output is untouched");
    // The offline render is the project's audio: no ceiling there.
    let loud = self::plan(&registry, 20.0, 0.2);
    let mut engine =
        Engine::new(&loud, &registry, EngineConfig::new(48_000, 1), Vec::new()).unwrap();
    let rendered = render(&mut engine, FRAMES).unwrap();
    assert!(
        rendered
            .samples
            .iter()
            .any(|&x| f64::from(device_sample(x)).abs() > 0.25)
    );
}

#[test]
fn a_ceiling_that_is_no_number_is_silence() {
    let registry = registry();
    let plan = plan(&registry, 1.0, 0.2);
    for ceiling in [f64::NAN, -1.0, 0.0] {
        let (out, _, _) = played(&plan, &registry, Some(ceiling));
        assert!(out.iter().all(|&x| x == 0.0), "{ceiling}");
    }
}
