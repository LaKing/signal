//! M2.10-1 review: the core types stay within the engine's magnitude bound
//! (10⁶, `docs/semantics.md` §7), so none of them faults on its own, through
//! the engine: `std.frames` past 10⁶ frames, and `std.div`, `std.exp`,
//! `std.pow`, `std.tan` and `std.neg` where their results would exceed it. It
//! lives here because this crate already depends on both `signal-std` and
//! `signal-engine`.

use signal_core::{InstanceAddress, PortAddress, Registry};
use signal_engine::{Engine, EngineConfig, LogRecord};
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
    let mut config = EngineConfig::new(1_000, 7);
    config.block_capacity = 4096;
    Engine::new(plan, &registry(), config, Vec::new()).unwrap()
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
