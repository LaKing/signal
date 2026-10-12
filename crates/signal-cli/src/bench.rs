//! `signal bench` (V13): what the per-frame engine costs at 96 kHz, as input to
//! D-EXEC-3 (`docs/benchmarks.md`). It drives the offline [`Engine`] one frame
//! at a time, exactly as it runs; the only instrumentation is a clock read
//! around each repeat of [`REPEAT_FRAMES`] frames, here and nowhere else.
//!
//! - Every registered `std.*` primitive: [`INSTANCES`] instances fed by a
//!   shared stimulus (a saw, a 2 Hz gate, a 132 bpm transport), minus the
//!   stimulus alone, per instance.
//! - The engine with an empty plan: its fixed per-frame overhead.
//! - The kick project (`projects/kick`, compiled in).
//! - Kick voices: the kick project with its `drums` instance duplicated *N*
//!   times into a tree of `std.mix`; the largest *N* whose median stays within
//!   25 % and 50 % of the 96 kHz budget.
//!
//! The numbers are measurements of the machine that ran them, never pass or
//! fail; the command checks nothing.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::Instant;

use clap::Args;
use serde_json::{Value, json};
use signal_core::{Direction, PortKind, PortShape, Registry};
use signal_engine::{Engine, EngineConfig};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, InstanceDef, build_plan};
use signal_project::{Project, SourceFile};

use crate::{Failure, print_stdout};

/// The rate every case runs at.
const SAMPLE_RATE: u32 = 96_000;
/// The seed every case runs with (the kick project's).
const SEED: u64 = 42;
/// Frames per timed repeat.
const REPEAT_FRAMES: u64 = 1024;
/// Repeats per case, at least, however short `--seconds` is.
const MIN_REPEATS: usize = 5;
/// Instances of a primitive in its graph.
const INSTANCES: usize = 16;
/// The largest voice count tried.
const MAX_VOICES: usize = 4096;

/// The kick project, as committed.
const KICK: [(&str, &str); 3] = [
    (
        "project.toml",
        include_str!("../../../projects/kick/project.toml"),
    ),
    (
        "composites/drums.toml",
        include_str!("../../../projects/kick/composites/drums.toml"),
    ),
    (
        "composites/kick.toml",
        include_str!("../../../projects/kick/composites/kick.toml"),
    ),
];

/// Arguments of `signal bench`.
#[derive(Args)]
pub(crate) struct BenchArgs {
    /// Time spent measuring each case, in seconds (at least 5 repeats of
    /// 1024 frames each).
    #[arg(long, default_value_t = 1.0)]
    seconds: f64,
}

/// One case's nanoseconds per frame over its repeats.
#[derive(Clone, Copy)]
struct Timing {
    median: f64,
    p99: f64,
    repeats: usize,
}

impl Timing {
    fn json(&self) -> Value {
        json!({ "median_ns": self.median, "p99_ns": self.p99, "repeats": self.repeats })
    }
}

/// The 96 kHz real-time budget per frame, in ns.
fn budget_ns() -> f64 {
    1e9 / f64::from(SAMPLE_RATE)
}

pub(crate) fn run(args: &BenchArgs, registry: &Registry, json: bool) -> Result<(), Failure> {
    if !(args.seconds.is_finite() && args.seconds > 0.0) {
        return Err(Failure::Usage(format!(
            "--seconds must be a positive number, not {}",
            args.seconds
        )));
    }
    let seconds = args.seconds;
    let budget = budget_ns();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    if !json {
        eprintln!("signal: bench: {seconds} s per case, {profile} build");
    }

    let empty = measure(
        &plan_of(&CompositeDef::default(), &[], registry)?,
        registry,
        seconds,
    )?;

    let stimulus = measure(
        &plan_of(&stimulus_graph(), &[], registry)?,
        registry,
        seconds,
    )?;
    let mut primitives = Vec::new();
    for module_type in registry.iter() {
        let name = module_type.descriptor.type_name;
        if !name.starts_with("std.") {
            continue;
        }
        let graph = primitive_graph(registry, name)?;
        let timing = measure(&plan_of(&graph, &[], registry)?, registry, seconds)?;
        let per_instance = Timing {
            median: (timing.median - stimulus.median) / INSTANCES as f64,
            p99: (timing.p99 - stimulus.median) / INSTANCES as f64,
            repeats: timing.repeats,
        };
        if !json {
            eprintln!("signal: bench: {name} {:.1} ns", per_instance.median);
        }
        primitives.push((name, per_instance, timing));
    }

    let kick = Project::from_sources(
        KICK.iter()
            .map(|(path, text)| SourceFile {
                path: (*path).to_owned(),
                text: (*text).to_owned(),
            })
            .collect(),
    )
    .map_err(|d| Failure::Internal(format!("the kick project does not load: {d:?}")))?;
    let kick_timing = measure(
        &plan_of(&kick.root, &kick.composites, registry)?,
        registry,
        seconds,
    )?;

    let mut voices = BTreeMap::new();
    let mut voice_timing = |n: usize| -> Result<Timing, Failure> {
        if let Some(timing) = voices.get(&n) {
            return Ok(*timing);
        }
        let root = voices_graph(&kick.root, n)?;
        let timing = measure(
            &plan_of(&root, &kick.composites, registry)?,
            registry,
            seconds,
        )?;
        if !json {
            eprintln!("signal: bench: {n} voices {:.0} ns", timing.median);
        }
        voices.insert(n, timing);
        Ok(timing)
    };
    let within = [0.25, 0.5].map(|fraction| fraction * budget);
    // Double until over the larger limit, then bisect below each limit.
    let mut over = 1;
    while over < MAX_VOICES && voice_timing(over)?.median <= within[1] {
        over *= 2;
    }
    let mut most = Vec::new();
    for limit in within {
        let (mut fits, mut fails) = (0, over);
        while fails - fits > 1 {
            let mid = (fits + fails) / 2;
            if voice_timing(mid)?.median <= limit {
                fits = mid;
            } else {
                fails = mid;
            }
        }
        if fits == MAX_VOICES - 1 && voice_timing(MAX_VOICES)?.median <= limit {
            fits = MAX_VOICES;
        }
        most.push((
            fits,
            if fits > 0 {
                Some(voice_timing(fits)?)
            } else {
                None
            },
        ));
    }

    let voices_json = |(n, timing): &(usize, Option<Timing>), limit: f64| {
        json!({
            "limit_ns": limit,
            "voices": n,
            "timing": timing.as_ref().map(Timing::json),
            "budget_fraction": timing.map(|t| t.median / budget),
        })
    };
    if json {
        let output = json!({
            "label": "measurement",
            "profile": profile,
            "machine": machine(),
            "sample_rate": SAMPLE_RATE,
            "budget_ns": budget,
            "seconds_per_case": seconds,
            "frames_per_repeat": REPEAT_FRAMES,
            "engine_empty": empty.json(),
            "stimulus": stimulus.json(),
            "primitives": primitives.iter().map(|(name, per_instance, graph)| json!({
                "type": name,
                "instances": INSTANCES,
                "median_ns": per_instance.median,
                "p99_ns": per_instance.p99,
                "repeats": per_instance.repeats,
                "graph": graph.json(),
            })).collect::<Vec<_>>(),
            "kick": {
                "median_ns": kick_timing.median,
                "p99_ns": kick_timing.p99,
                "repeats": kick_timing.repeats,
                "budget_fraction": kick_timing.median / budget,
            },
            "kick_voices": {
                "criterion": "median",
                "within_25_percent": voices_json(&most[0], within[0]),
                "within_50_percent": voices_json(&most[1], within[1]),
                "measured": voices.iter().map(|(n, t)| json!({
                    "voices": n, "median_ns": t.median, "p99_ns": t.p99, "repeats": t.repeats,
                })).collect::<Vec<_>>(),
            },
        });
        print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
    } else {
        let mut text = format!(
            "measurement, {profile} build, {SAMPLE_RATE} Hz (budget {budget:.0} ns per frame)\n\
             engine, empty plan        {:>9.1} ns/frame (p99 {:.1})\n\
             stimulus alone            {:>9.1} ns/frame (p99 {:.1})\n\
             per instance (median, p99 ns/frame; {INSTANCES} instances minus the stimulus):\n",
            empty.median, empty.p99, stimulus.median, stimulus.p99
        );
        for (name, timing, _) in &primitives {
            text.push_str(&format!(
                "  {name:<22}  {:>8.1}  {:>8.1}\n",
                timing.median, timing.p99
            ));
        }
        text.push_str(&format!(
            "kick project              {:>9.1} ns/frame (p99 {:.1}), {:.1} % of the budget\n\
             kick voices within 25 %   {:>9}\n\
             kick voices within 50 %   {:>9}",
            kick_timing.median,
            kick_timing.p99,
            100.0 * kick_timing.median / budget,
            most[0].0,
            most[1].0
        ));
        print_stdout(&text);
    }
    Ok(())
}

/// Times `plan` for about `seconds`: repeats of [`REPEAT_FRAMES`] frames after
/// one untimed repeat.
fn measure(plan: &ExecutionPlan, registry: &Registry, seconds: f64) -> Result<Timing, Failure> {
    let config = EngineConfig::new(SAMPLE_RATE, SEED);
    let mut engine = Engine::new(plan, registry, config, Vec::new())
        .map_err(|e| Failure::Internal(e.to_string()))?;
    for _ in 0..REPEAT_FRAMES {
        engine.run_frame();
    }
    let mut repeats = Vec::new();
    let start = Instant::now();
    while repeats.len() < MIN_REPEATS || start.elapsed().as_secs_f64() < seconds {
        let began = Instant::now();
        for _ in 0..REPEAT_FRAMES {
            engine.run_frame();
        }
        repeats.push(began.elapsed().as_nanos() as f64 / REPEAT_FRAMES as f64);
    }
    repeats.sort_by(f64::total_cmp);
    // Nearest rank.
    let rank = |p: f64| {
        let k = ((p * repeats.len() as f64).ceil() as usize).clamp(1, repeats.len());
        repeats.get(k - 1).copied().unwrap_or_default()
    };
    Ok(Timing {
        median: rank(0.5),
        p99: rank(0.99),
        repeats: repeats.len(),
    })
}

fn plan_of(
    root: &CompositeDef,
    composites: &[CompositeDef],
    registry: &Registry,
) -> Result<ExecutionPlan, Failure> {
    build_plan(root, composites, registry, 0).map_err(|errors| {
        let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
        Failure::Internal(format!("a bench graph is invalid: {}", messages.join("; ")))
    })
}

fn instance(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect(),
    }
}

fn connection(from: &str, to: &str, scale: Option<f64>) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale,
    }
}

/// The inputs every primitive is fed: a 440 Hz band-limited saw, a 2 Hz LFO as
/// a gate, and a 132 bpm transport with ghost notes, as the kick project's.
fn stimulus_graph() -> CompositeDef {
    CompositeDef {
        instances: vec![
            instance("saw", "std.osc", &[]),
            instance("gate", "std.lfo", &[("freq_hz", 2.0)]),
            instance(
                "transport",
                "std.transport",
                &[("bpm", 132.0), ("ghost_probability", 0.15)],
            ),
        ],
        ..CompositeDef::default()
    }
}

/// The stimulus and [`INSTANCES`] instances of `type_name`, each input fed by
/// it: signals by the saw (phase inputs scaled to 0.05 cycles), gates by the
/// LFO, the first event input by the beats and ghost notes, any other by the
/// accents. Outputs are left open; every instance runs regardless.
fn primitive_graph(registry: &Registry, type_name: &str) -> Result<CompositeDef, Failure> {
    let descriptor = registry
        .iter()
        .find(|t| t.descriptor.type_name == type_name)
        .map(|t| t.descriptor)
        .ok_or_else(|| Failure::Internal(format!("{type_name} is not registered")))?;
    let mut graph = stimulus_graph();
    for i in 0..INSTANCES {
        let name = format!("p{i}");
        graph.instances.push(instance(&name, type_name, &[]));
        let mut events = 0;
        for port in descriptor
            .ports
            .iter()
            .filter(|p| p.direction == Direction::In)
        {
            let to = format!("{name}.{}", port.name);
            let sources: &[(&str, Option<f64>)] = match (port.shape, port.kind) {
                (PortShape::Signal, PortKind::Phase) => &[("saw.out", Some(0.05))],
                (PortShape::Signal, PortKind::Gate) => &[("gate.out", None)],
                (PortShape::Signal, _) => &[("saw.out", None)],
                (PortShape::Events, _) => {
                    events += 1;
                    if events == 1 {
                        &[("transport.beat", None), ("transport.ghost", None)]
                    } else {
                        &[("transport.accent", None)]
                    }
                }
                (shape, _) => {
                    return Err(Failure::Internal(format!(
                        "{to}: the bench cannot feed a {shape:?} input"
                    )));
                }
            };
            for (from, scale) in sources {
                graph.connections.push(connection(from, &to, *scale));
            }
        }
    }
    Ok(graph)
}

/// The kick project's root with its `drums` instance duplicated `n` times
/// (every connection to and from it repeated), the copies' outputs summed by
/// a tree of 4-input `std.mix` into the sink.
fn voices_graph(kick: &CompositeDef, n: usize) -> Result<CompositeDef, Failure> {
    let drums = kick
        .instances
        .iter()
        .find(|i| i.name == "drums")
        .ok_or_else(|| Failure::Internal("the kick project has no `drums`".to_owned()))?;
    let mut root = CompositeDef {
        name: kick.name.clone(),
        instances: kick
            .instances
            .iter()
            .filter(|i| i.name != "drums")
            .cloned()
            .collect(),
        connections: Vec::new(),
        exports: kick.exports.clone(),
    };
    let mut outputs = Vec::new();
    for i in 0..n {
        let name = format!("drums_{i}");
        root.instances.push(InstanceDef {
            name: name.clone(),
            ..drums.clone()
        });
        outputs.push(format!("{name}.out"));
    }
    let mut sink_inputs = Vec::new();
    for c in &kick.connections {
        match (c.from.split_once('.'), c.to.split_once('.')) {
            (Some(("drums", _)), Some((_, _))) => sink_inputs.push(c.to.clone()),
            (_, Some(("drums", port))) => {
                for i in 0..n {
                    root.connections.push(connection(
                        &c.from,
                        &format!("drums_{i}.{port}"),
                        c.scale,
                    ));
                }
            }
            _ => root.connections.push(c.clone()),
        }
    }
    // At least one mixer, then one level per factor of 4.
    let mut level = 0;
    loop {
        let mut next = Vec::new();
        for (j, group) in outputs.chunks(4).enumerate() {
            let name = format!("mix_{level}_{j}");
            root.instances.push(instance(&name, "std.mix", &[]));
            for (input, from) in ["a", "b", "c", "d"].iter().zip(group) {
                root.connections
                    .push(connection(from, &format!("{name}.{input}"), None));
            }
            next.push(format!("{name}.out"));
        }
        outputs = next;
        level += 1;
        if outputs.len() <= 1 {
            break;
        }
    }
    for to in sink_inputs {
        for from in &outputs {
            root.connections.push(connection(from, &to, None));
        }
    }
    Ok(root)
}

/// The machine, from `sysctl` (null where it does not answer).
fn machine() -> Value {
    let sysctl = |key: &str| {
        Command::new("sysctl")
            .args(["-n", key])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    json!({
        "model": sysctl("hw.model"),
        "cpu": sysctl("machdep.cpu.brand_string"),
        "cores": sysctl("hw.ncpu"),
        "performance_cores": sysctl("hw.perflevel0.physicalcpu"),
        "memory_bytes": sysctl("hw.memsize"),
        "os": sysctl("kern.osproductversion"),
    })
}
