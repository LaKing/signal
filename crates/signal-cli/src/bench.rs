//! `signal bench` (V13): what the per-frame engine costs at 96 kHz, as input to
//! D-EXEC-3 (`docs/benchmarks.md`). It drives the offline [`Engine`] one frame
//! at a time (and, for the block rows, *k* frames at a time), exactly as it
//! runs; the only instrumentation is a clock read around each repeat of
//! [`REPEAT_FRAMES`] frames, here and nowhere else.
//!
//! - Every registered `std.*` primitive: [`INSTANCES`] instances fed by a
//!   shared stimulus (a saw, a 2 Hz gate, a 132 bpm transport), minus the
//!   stimulus alone, per instance.
//! - The engine with an empty plan: its fixed per-frame overhead.
//! - The kick project (`projects/kick`, compiled in).
//! - Kick voices: the kick project with its `drums` instance duplicated *N*
//!   times into a tree of `std.mix`; the largest *N* whose median stays within
//!   25 % and 50 % of the 96 kHz budget.
//! - Block sizes (ADR 0019 §1): the kick and the kick voices within 25 % of
//!   the budget, run in blocks of *k* = 1, 64 and 256 frames, on one thread
//!   and on the performance cores.
//! - Scaling (ADR 0019 §2): the most kick voices within 25 % and 50 % of the
//!   budget at *k* = 128 on 1, 2, 4, 8 and all performance cores.
//! - The small-plan threshold: kick voices in segments of 16 to 256 frames on
//!   two threads (every segment on the workers) against one thread, and the
//!   smallest per-segment estimate at which two threads were faster.
//!
//! - `--realtime` (ADR 0019 §4, M2.6-3), instead of all the above: kick
//!   voices in blocks of 128 frames paced at the 96 kHz period, timed block
//!   by block on 1, 8 and all performance cores (and one fewer): ordinary;
//!   with the workers in the time-constraint policy the CoreAudio driver gives
//!   them (the calling thread ordinary, so only the workers differ); and with
//!   the calling thread in it too (as a device's I/O thread is). Per block it
//!   also reads the wake-up tail (M2.6-5) off the engine's task hook: the
//!   time from the first thread into the block's job until every joined
//!   worker has started, and the longest task body. `--rt-schedule` repeats
//!   the table for other worker schedules.
//!
//! The numbers are measurements of the machine that ran them, never pass or
//! fail; the command checks nothing.
//!
//! `--write-voices <N> <DIR>` measures nothing: it writes the kick voices as
//! a project (the soak's many-voice load, `scripts/soak.sh --voices`).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use clap::Args;
use serde_json::{Value, json};
use signal_audio::{TimeConstraint, WorkerPath, rt_thread};
use signal_core::{Direction, PortKind, PortShape, Registry};
use signal_engine::{Engine, EngineConfig, HookPoint, Schedule, Workers, performance_cores};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
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
/// The block sizes the kick and kick voices are timed at (ADR 0019 §1); each
/// divides [`REPEAT_FRAMES`].
const BLOCK_SIZES: [usize; 3] = [1, 64, 256];
/// The block size of the scaling table (ADR 0019 §2).
const SCALING_BLOCK: usize = 128;
/// The kick voices and segment lengths the small-plan threshold is measured
/// at; each segment length divides [`REPEAT_FRAMES`].
const THRESHOLD_VOICES: [usize; 5] = [1, 2, 4, 8, 16];
const THRESHOLD_BLOCKS: [usize; 4] = [16, 64, 128, 256];
/// The block (and device period) of the `--realtime` table, in frames.
const RT_BLOCK: usize = 128;
/// Timed blocks per row of the `--realtime` table, at least.
const RT_MIN_BLOCKS: usize = 100;
/// How long the GPU partial bank waits for one result before it moves on.
#[cfg(feature = "lab")]
const GPU_GIVE_UP: Duration = Duration::from_secs(5);
/// How long the `--realtime` table waits for the workers to change policy.
const RT_WAIT: Duration = Duration::from_secs(1);
/// The `--realtime` variants: ordinary threads; the workers in the
/// time-constraint policy (the calling thread ordinary, as in `ordinary`:
/// only the workers differ); and the calling thread too, as a device's I/O
/// thread is.
const RT_VARIANTS: [&str; 3] = ["ordinary", "rt-workers", "rt-all"];

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
    /// Write the kick with its `drums` composite duplicated N times into a
    /// tree of `std.mix` as a project in DIR (new or empty), and measure
    /// nothing: the soak's many-voice load.
    #[arg(long, num_args = 2, value_names = ["N", "DIR"])]
    write_voices: Option<Vec<String>>,
    /// Measure only real-time worker scheduling: kick voices in paced
    /// 128-frame blocks on 1, 8 and all performance cores (and one fewer),
    /// with ordinary and with time-constraint workers.
    #[arg(long, conflicts_with = "write_voices")]
    realtime: bool,
    /// The kick voices of the --realtime table.
    #[arg(long, default_value_t = 24, requires = "realtime")]
    rt_voices: usize,
    /// A worker schedule of the --realtime table (repeatable; default:
    /// `default`): `base` (the M2.6-4 scheduling), `default` (the real-time
    /// one `signal play` uses), or `base` changed by a comma list of
    /// `spin=<ns|period|quarter>` (idle spin) and `task=<ns>` (chain size).
    #[arg(long, requires = "realtime")]
    rt_schedule: Vec<String>,
    /// Measure only the GPU partial bank (`lab.gpu_partials`): its cost per
    /// block on the GPU, next to its CPU reference.
    #[cfg(feature = "lab")]
    #[arg(long, conflicts_with_all = ["write_voices", "realtime"])]
    gpu: bool,
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
    if let Some(voices) = &args.write_voices {
        return write_voices(voices, registry, json);
    }
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
    if args.realtime {
        return realtime_table(
            args.rt_voices,
            &args.rt_schedule,
            seconds,
            profile,
            registry,
            json,
        );
    }
    #[cfg(feature = "lab")]
    if args.gpu {
        let gpu = gpu_partials(seconds, json);
        if json {
            let output = json!({
                "label": "measurement",
                "profile": profile,
                "machine": machine(),
                "sample_rate": SAMPLE_RATE,
                "seconds_per_case": seconds,
                "gpu_partials": gpu.json(),
            });
            print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
        } else {
            print_stdout(&format!(
                "measurement, {profile} build, {SAMPLE_RATE} Hz\n{}",
                gpu.text()
            ));
        }
        return Ok(());
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

    let kick = kick_project()?;
    let kick_timing = measure(
        &plan_of(&kick.root, &kick.composites, registry)?,
        registry,
        seconds,
    )?;

    let within = [0.25, 0.5].map(|fraction| fraction * budget);
    let voices_plan = |n: usize| plan_of(&voices_graph(&kick.root, n)?, &kick.composites, registry);
    let (most, voices) = most_voices(within, |n| {
        let timing = measure(&voices_plan(n)?, registry, seconds)?;
        if !json {
            eprintln!("signal: bench: {n} voices {:.0} ns", timing.median);
        }
        Ok(timing)
    })?;

    // Block sizes (ADR 0019 §1): the kick and the kick voices within 25 % of
    // the budget (at k = 1), run k frames at a time, on one thread and on
    // the performance cores (ADR 0019 §2, the default schedule).
    let (cores, cores_source) = performance_cores();
    let all = Threads {
        threads: cores,
        schedule: Schedule::DEFAULT,
    };
    let block_voices = most[0].0.max(1);
    let kick_plan = plan_of(&kick.root, &kick.composites, registry)?;
    let block_voices_plan = voices_plan(block_voices)?;
    let mut blocks = Vec::new();
    for k in BLOCK_SIZES {
        let mut row = Vec::new();
        for on in [ONE_THREAD, all] {
            let kick = measure_blocks(&kick_plan, registry, seconds, k, on)?;
            let voices = measure_blocks(&block_voices_plan, registry, seconds, k, on)?;
            if !json {
                eprintln!(
                    "signal: bench: k = {k}, {} threads: kick {:.1} ns, {block_voices} voices {:.0} ns",
                    on.threads, kick.median, voices.median
                );
            }
            row.push((kick, voices));
        }
        blocks.push((k, row));
    }

    // Scaling (ADR 0019 §2): the most kick voices within 25 % and 50 % of
    // the budget at k = SCALING_BLOCK on 1, 2, 4, 8 and all performance
    // cores, with the default schedule.
    let mut thread_counts = vec![1, 2, 4, 8, cores];
    thread_counts.sort_unstable();
    thread_counts.dedup();
    let mut scaling = Vec::new();
    for threads in thread_counts {
        let on = Threads {
            threads,
            schedule: Schedule::DEFAULT,
        };
        let (most, measured) = most_voices(within, |n| {
            let timing = measure_blocks(&voices_plan(n)?, registry, seconds, SCALING_BLOCK, on)?;
            if !json {
                eprintln!(
                    "signal: bench: k = {SCALING_BLOCK}, {threads} threads: {n} voices {:.0} ns",
                    timing.median
                );
            }
            Ok(timing)
        })?;
        scaling.push((threads, most, measured));
    }

    // The small-plan threshold (ADR 0019 §2): kick voices in segments of k
    // frames, on 2 threads with every segment on the workers against one
    // thread; the plan's estimate per segment where the workers start to win.
    let forced = Threads {
        threads: 2,
        schedule: Schedule::FORCED,
    };
    let mut threshold = Vec::new();
    for n in THRESHOLD_VOICES {
        let plan = voices_plan(n)?;
        let estimate = estimate_of(&plan, registry)?;
        for k in THRESHOLD_BLOCKS {
            let one = measure_blocks(&plan, registry, seconds, k, ONE_THREAD)?;
            let two = measure_blocks(&plan, registry, seconds, k, forced)?;
            if !json {
                eprintln!(
                    "signal: bench: threshold: {n} voices, k = {k}: estimate {} ns, 1 thread {:.0} ns, 2 threads {:.0} ns",
                    estimate.0 * k as u64,
                    one.median,
                    two.median
                );
            }
            threshold.push((n, k, estimate, one, two));
        }
    }
    let crossover = threshold
        .iter()
        .filter(|(_, _, _, one, two)| two.median < one.median)
        .map(|(_, k, estimate, _, _)| estimate.0 * *k as u64)
        .min();

    // The GPU partial bank (ADR 0019 §3), in a `--features lab` build.
    #[cfg(feature = "lab")]
    let gpu = gpu_partials(seconds, json);

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
            "blocks": {
                "voices": block_voices,
                "threads": cores,
                "measured": blocks.iter().map(|(k, row)| json!({
                    "k": k,
                    "kick": row[0].0.json(),
                    "kick_voices": row[0].1.json(),
                    "on_performance_cores": {
                        "threads": cores,
                        "kick": row[1].0.json(),
                        "kick_voices": row[1].1.json(),
                    },
                })).collect::<Vec<_>>(),
            },
            "scaling": {
                "block": SCALING_BLOCK,
                "performance_cores": cores,
                "cores_source": cores_source.name(),
                "rows": scaling.iter().map(|(threads, most, measured)| json!({
                    "threads": threads,
                    "within_25_percent": voices_json(&most[0], within[0]),
                    "within_50_percent": voices_json(&most[1], within[1]),
                    "measured": measured.iter().map(|(n, t)| json!({
                        "voices": n, "median_ns": t.median, "p99_ns": t.p99, "repeats": t.repeats,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            },
            "parallel_threshold": {
                "threads": forced.threads,
                "crossover_ns": crossover,
                "rows": threshold.iter().map(|(n, k, (estimate, units, tasks), one, two)| json!({
                    "voices": n,
                    "k": k,
                    "units": units,
                    "tasks": tasks,
                    "estimate_ns": estimate * *k as u64,
                    "one_thread": one.json(),
                    "two_threads": two.json(),
                })).collect::<Vec<_>>(),
            },
        });
        #[cfg(feature = "lab")]
        let output = {
            let mut output = output;
            output["gpu_partials"] = gpu.json();
            output
        };
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
        text.push_str(&format!(
            "\nblocks of k frames (median, p99 ns/frame; kick, then {block_voices} kick voices; \
             1 thread, then {cores}):"
        ));
        for (k, row) in &blocks {
            for (on, (kick, voices)) in [1, cores].iter().zip(row) {
                text.push_str(&format!(
                    "\n  k = {k:<4} {on:>2} threads  {:>9.1}  {:>9.1}  {:>9.1}  {:>9.1}",
                    kick.median, kick.p99, voices.median, voices.p99
                ));
            }
        }
        text.push_str(&format!(
            "\nscaling at k = {SCALING_BLOCK} (kick voices within 25 %, 50 %; {cores} performance \
             cores, {}):",
            cores_source.name()
        ));
        for (threads, most, _) in &scaling {
            text.push_str(&format!(
                "\n  {threads:>2} threads  {:>6}  {:>6}",
                most[0].0, most[1].0
            ));
        }
        text.push_str(&format!(
            "\nparallel threshold (2 threads, every segment on the workers): crossover {}",
            crossover.map_or("none measured".to_owned(), |ns| format!(
                "{ns} ns per segment"
            ))
        ));
        #[cfg(feature = "lab")]
        text.push_str(&format!("\n{}", gpu.text()));
        print_stdout(&text);
    }
    Ok(())
}

/// The kick project, as committed.
fn kick_project() -> Result<Project, Failure> {
    Project::from_sources(
        KICK.iter()
            .map(|(path, text)| SourceFile {
                path: (*path).to_owned(),
                text: (*text).to_owned(),
            })
            .collect(),
    )
    .map_err(|d| Failure::Internal(format!("the kick project does not load: {d:?}")))
}

/// `--write-voices <N> <DIR>`: the kick voices ([`voices_graph`]) as a
/// project in `DIR`, its composites as committed, checked by loading and
/// planning it back.
fn write_voices(args: &[String], registry: &Registry, json: bool) -> Result<(), Failure> {
    let [n, dir] = args else {
        return Err(Failure::Usage("--write-voices needs N and DIR".to_owned()));
    };
    let n: usize = match n.parse() {
        Ok(n) if (1..=MAX_VOICES).contains(&n) => n,
        _ => {
            return Err(Failure::Usage(format!(
                "--write-voices: N must be a whole number from 1 to {MAX_VOICES}, not {n}"
            )));
        }
    };
    let dir = Path::new(dir);
    if fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(Failure::Usage(format!(
            "{} is not empty; the project is written into a new directory",
            dir.display()
        )));
    }
    let kick = kick_project()?;
    let mut root = voices_graph(&kick.root, n)?;
    // N voices sum to N times the kick: the sink's gain is divided by N so
    // the soak does not clip. Every instance still runs; the cost is the same.
    if let Some(out) = root.instances.iter_mut().find(|i| i.name == "out")
        && let Some(gain) = out.parameters.get_mut("gain")
    {
        *gain /= n as f64;
    }
    let name = format!("kick-voices-{n}");
    let text = project_text(&name, kick.sample_rate, kick.seed, n, &root)?;
    let io = |path: &Path, e: std::io::Error| Failure::Io(format!("{}: {e}", path.display()));
    fs::create_dir_all(dir.join("composites")).map_err(|e| io(dir, e))?;
    let project = dir.join("project.toml");
    fs::write(&project, text).map_err(|e| io(&project, e))?;
    for (path, text) in KICK.iter().filter(|(path, _)| *path != "project.toml") {
        let path = dir.join(path);
        fs::write(&path, text).map_err(|e| io(&path, e))?;
    }
    let written = Project::load(dir)
        .map_err(|e| Failure::Internal(format!("the written project does not load: {e}")))?;
    let planned = written
        .plan(registry)
        .map_err(|d| Failure::Internal(format!("the written project does not plan: {d:?}")))?;
    let instances = planned.plan.instances.len();
    if json {
        let output = json!({
            "project": dir.display().to_string(),
            "voices": n,
            "instances": instances,
        });
        print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
    } else {
        print_stdout(&format!(
            "wrote {} ({n} kick voices, {instances} primitive instances)",
            dir.display()
        ));
    }
    Ok(())
}

/// `root` as a `project.toml`: an inline table per instance, its module
/// first, and a line per connection.
fn project_text(
    name: &str,
    sample_rate: u32,
    seed: u64,
    voices: usize,
    root: &CompositeDef,
) -> Result<String, Failure> {
    if root.exports != Exports::default() {
        return Err(Failure::Internal("the kick's root has exports".to_owned()));
    }
    let mut text = format!(
        "# The kick project with its `drums` composite duplicated {voices} times, the\n\
         # copies summed by a tree of 4-input `std.mix` (`signal bench --write-voices`).\n\
         # A generated soak load, not a product.\n\
         schema_version = 1\n\n\
         [project]\nname = {name:?}\nsample_rate = {sample_rate}\nseed = {seed}\n\n\
         [instances]\n"
    );
    for i in &root.instances {
        let parameters: String = i
            .parameters
            .iter()
            .map(|(key, value)| format!(", {key} = {value}"))
            .collect();
        text.push_str(&format!(
            "{} = {{ module = {:?}{parameters} }}\n",
            i.name, i.module
        ));
    }
    text.push_str("\n[wiring]\nconnections = [\n");
    for c in &root.connections {
        let scale = c
            .scale
            .map(|s| format!(", scale = {s}"))
            .unwrap_or_default();
        text.push_str(&format!(
            "  {{ from = {:?}, to = {:?}{scale} }},\n",
            c.from, c.to
        ));
    }
    text.push_str("]\n");
    Ok(text)
}

/// One row of the `--realtime` table, in ns per block.
struct RtRow {
    threads: usize,
    workers: usize,
    schedule: String,
    variant: &'static str,
    paths: Vec<WorkerPath>,
    caller: Option<WorkerPath>,
    block: Spread,
    over_quarter: usize,
    over_period: usize,
    tail: Tail,
}

/// The median, p99 (nearest rank) and maximum of some figures.
#[derive(Clone, Copy, Default)]
struct Spread {
    median: f64,
    p99: f64,
    max: f64,
}

impl Spread {
    fn of(mut values: Vec<f64>) -> Spread {
        values.sort_by(f64::total_cmp);
        let rank = |p: f64| {
            let k = ((p * values.len() as f64).ceil() as usize).clamp(1, values.len().max(1));
            values.get(k - 1).copied().unwrap_or_default()
        };
        Spread {
            median: rank(0.5),
            p99: rank(0.99),
            max: values.last().copied().unwrap_or_default(),
        }
    }

    fn json(&self) -> Value {
        json!({ "median_ns": self.median, "p99_ns": self.p99, "max_ns": self.max })
    }
}

/// A row's wake-up tail, over its blocks that ran one job on the workers.
#[derive(Default)]
struct Tail {
    /// From the first thread into the job until the last joined worker
    /// entered it (0 when no worker joined).
    worker_start: Spread,
    /// The longest task body of the job.
    slowest_task: Spread,
    /// From the block's start until the first thread entered the job: the
    /// staging, the open and whatever delayed the first participant.
    open_after: Spread,
    /// The workers that joined the job.
    joined: Spread,
    /// Blocks that ran on the workers.
    parallel_blocks: usize,
    /// Blocks with more than one job (none without edits); left out above.
    multi_job_blocks: usize,
}

impl Tail {
    fn json(&self) -> Value {
        json!({
            "worker_start": self.worker_start.json(),
            "slowest_task": self.slowest_task.json(),
            "open_after": self.open_after.json(),
            "joined": { "median": self.joined.median, "max": self.joined.max },
            "parallel_blocks": self.parallel_blocks,
            "multi_job_blocks": self.multi_job_blocks,
        })
    }
}

/// What the tail hook writes, in ns since [`TAIL_EPOCH`]; atomics only, so
/// the hook neither allocates nor locks on the workers. Reset before each
/// block and read after it: `run_block` returns only once every worker has
/// left the job (Release on leave, Acquire in the caller), so the workers'
/// writes are visible then.
static TAIL_EPOCH: OnceLock<Instant> = OnceLock::new();
static TASK_START: [AtomicU64; 1024] = [const { AtomicU64::new(0) }; 1024];
static FIRST_ENTER: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_WORKER_ENTER: AtomicU64 = AtomicU64::new(0);
static SLOWEST_TASK: AtomicU64 = AtomicU64::new(0);
static JOINED: AtomicU64 = AtomicU64::new(0);
static JOBS: AtomicU64 = AtomicU64::new(0);

fn tail_now() -> u64 {
    TAIL_EPOCH
        .get()
        .map_or(0, |epoch| epoch.elapsed().as_nanos() as u64)
}

/// The engine's task hook for the `--realtime` table.
fn tail_hook(point: HookPoint) {
    match point {
        HookPoint::Enter { lane } => {
            let now = tail_now();
            FIRST_ENTER.fetch_min(now, Ordering::Relaxed);
            if lane == 0 {
                JOBS.fetch_add(1, Ordering::Relaxed);
            } else {
                LAST_WORKER_ENTER.fetch_max(now, Ordering::Relaxed);
                JOINED.fetch_add(1, Ordering::Relaxed);
            }
        }
        HookPoint::Task { lane, .. } => {
            if let Some(start) = TASK_START.get(lane) {
                start.store(tail_now(), Ordering::Relaxed);
            }
        }
        HookPoint::Done { lane, .. } => {
            if let Some(start) = TASK_START.get(lane) {
                let body = tail_now().saturating_sub(start.load(Ordering::Relaxed));
                SLOWEST_TASK.fetch_max(body, Ordering::Relaxed);
            }
        }
        HookPoint::Leave { .. } => {}
    }
}

fn reset_tail() {
    FIRST_ENTER.store(u64::MAX, Ordering::Relaxed);
    LAST_WORKER_ENTER.store(0, Ordering::Relaxed);
    SLOWEST_TASK.store(0, Ordering::Relaxed);
    JOINED.store(0, Ordering::Relaxed);
    JOBS.store(0, Ordering::Relaxed);
}

/// The worker scheduling of the M2.6-4 checkpoint (dafa32b), pinned so that
/// `base` rows stay comparable whatever the defaults become.
const BASE_SCHEDULE: Schedule = Schedule {
    task_min_ns: 2_000,
    parallel_min_ns: 50_000,
    min_frames: 16,
    spin_ns: 50_000,
};

/// The schedule named by an `--rt-schedule` value (see [`BenchArgs`]) for
/// `threads` threads.
fn rt_schedule(spec: &str, period_ns: u64, threads: usize) -> Result<Schedule, Failure> {
    match spec {
        "base" => return Ok(BASE_SCHEDULE),
        "default" => return Ok(Schedule::realtime(period_ns, threads)),
        _ => {}
    }
    let usage = || {
        Failure::Usage(format!(
            "--rt-schedule: `{spec}` is not base, default or a comma list of \
             spin=<ns|period|quarter> and task=<ns>"
        ))
    };
    let mut schedule = BASE_SCHEDULE;
    for part in spec.split(',') {
        let (key, value) = part.split_once('=').ok_or_else(usage)?;
        let number = |v: &str| v.parse::<u64>().map_err(|_| usage());
        match (key, value) {
            ("spin", "period") => schedule.spin_ns = period_ns,
            ("spin", "quarter") => schedule.spin_ns = period_ns / 4,
            ("spin", v) => schedule.spin_ns = number(v)?,
            ("task", v) => schedule.task_min_ns = number(v)?,
            _ => return Err(usage()),
        }
    }
    Ok(schedule)
}

/// Puts every worker into `policy` (offline, without a workgroup) and says
/// which path each took.
fn enter_workers(workers: &Workers, policy: TimeConstraint) -> Vec<WorkerPath> {
    let paths = Arc::new(Mutex::new(vec![None; workers.count()]));
    let slots = Arc::clone(&paths);
    let generation = workers.post(move |w| {
        let path = rt_thread::enter(&policy, Err("offline: no device workgroup"));
        let mut slots = slots.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(slot) = slots.get_mut(w) {
            *slot = Some(path);
        }
    });
    let _ = workers.wait(generation, RT_WAIT);
    let paths = paths.lock().unwrap_or_else(PoisonError::into_inner);
    paths
        .iter()
        .map(|p| {
            p.clone()
                .unwrap_or_else(|| WorkerPath::None("pending".to_owned()))
        })
        .collect()
}

/// `--realtime`: `voices` kick voices in blocks of [`RT_BLOCK`] frames, one
/// block per period of 128 frames at 96 kHz (the thread sleeps between
/// blocks, so idle workers park as they do with a device), each block timed,
/// on 1, 8, the real-time auto count (two thirds of the performance cores)
/// and all of them, in each of
/// [`RT_VARIANTS`]: ordinary; the workers in the time-constraint policy the
/// CoreAudio driver sets (`TimeConstraint::from_period`), the calling thread
/// ordinary; and the calling thread in it too; once per schedule of
/// `schedules` (`default` when empty).
fn realtime_table(
    voices: usize,
    schedules: &[String],
    seconds: f64,
    profile: &str,
    registry: &Registry,
    json: bool,
) -> Result<(), Failure> {
    if !(1..=MAX_VOICES).contains(&voices) {
        return Err(Failure::Usage(format!(
            "--rt-voices must be from 1 to {MAX_VOICES}, not {voices}"
        )));
    }
    let kick = kick_project()?;
    let plan = plan_of(
        &voices_graph(&kick.root, voices)?,
        &kick.composites,
        registry,
    )?;
    let (cores, cores_source) = performance_cores();
    let mut thread_counts = vec![1, 8, signal_engine::realtime_threads(), cores];
    thread_counts.sort_unstable();
    thread_counts.dedup();
    let period = Duration::from_secs_f64(RT_BLOCK as f64 / f64::from(SAMPLE_RATE));
    let period_ns = period.as_nanos() as f64;
    let blocks = ((seconds / period.as_secs_f64()).ceil() as usize).max(RT_MIN_BLOCKS);
    let timebase = rt_thread::timebase();
    let policy = TimeConstraint::from_period(RT_BLOCK as u32, SAMPLE_RATE, timebase);
    let default = ["default".to_owned()];
    let schedules = if schedules.is_empty() {
        &default[..]
    } else {
        schedules
    };
    let period_ns_whole = period.as_nanos() as u64;
    for spec in schedules {
        rt_schedule(spec, period_ns_whole, 1)?;
    }
    TAIL_EPOCH.get_or_init(Instant::now);

    let mut rows = Vec::new();
    for &threads in &thread_counts {
        for name in schedules {
            let schedule = rt_schedule(name, period_ns_whole, threads)?;
            for variant in RT_VARIANTS {
                let config = EngineConfig {
                    block_capacity: RT_BLOCK,
                    threads,
                    schedule,
                    ..EngineConfig::new(SAMPLE_RATE, SEED)
                };
                let mut engine = Engine::new(&plan, registry, config, Vec::new())
                    .map_err(|e| Failure::Internal(e.to_string()))?;
                engine.set_task_hook(Some(tail_hook));
                let workers = engine.workers();
                let paths = if variant == "ordinary" {
                    Vec::new()
                } else {
                    enter_workers(&workers, policy)
                };
                let caller = (variant == "rt-all")
                    .then(|| rt_thread::enter(&policy, Err("offline: no device workgroup")));
                // Untimed: one repeat's worth of blocks.
                for _ in 0..REPEAT_FRAMES as usize / RT_BLOCK {
                    engine.run_block(RT_BLOCK);
                }
                let mut times = Vec::with_capacity(blocks);
                let mut tail = Tail::default();
                let [mut worker_start, mut slowest, mut open_after, mut joined] =
                    std::array::from_fn(|_| Vec::with_capacity(blocks));
                let mut next = Instant::now() + period;
                for _ in 0..blocks {
                    if let Some(wait) = next.checked_duration_since(Instant::now()) {
                        thread::sleep(wait);
                    }
                    reset_tail();
                    let began_ns = tail_now();
                    let began = Instant::now();
                    engine.run_block(RT_BLOCK);
                    times.push(began.elapsed().as_nanos() as f64);
                    next = (next + period).max(Instant::now());
                    match JOBS.load(Ordering::Relaxed) {
                        0 => {}
                        1 => {
                            tail.parallel_blocks += 1;
                            let first = FIRST_ENTER.load(Ordering::Relaxed);
                            let workers_in = JOINED.load(Ordering::Relaxed);
                            let last = LAST_WORKER_ENTER.load(Ordering::Relaxed);
                            worker_start.push(if workers_in > 0 {
                                last.saturating_sub(first) as f64
                            } else {
                                0.0
                            });
                            slowest.push(SLOWEST_TASK.load(Ordering::Relaxed) as f64);
                            open_after.push(first.saturating_sub(began_ns) as f64);
                            joined.push(workers_in as f64);
                        }
                        _ => {
                            tail.parallel_blocks += 1;
                            tail.multi_job_blocks += 1;
                        }
                    }
                }
                tail.worker_start = Spread::of(worker_start);
                tail.slowest_task = Spread::of(slowest);
                tail.open_after = Spread::of(open_after);
                tail.joined = Spread::of(joined);
                if variant != "ordinary" {
                    let generation = workers.post(|_| rt_thread::exit());
                    let _ = workers.wait(generation, RT_WAIT);
                }
                if caller.is_some() {
                    rt_thread::exit();
                }
                drop(engine);
                let row = RtRow {
                    threads,
                    workers: workers.count(),
                    schedule: name.clone(),
                    variant,
                    paths,
                    caller,
                    over_quarter: times.iter().filter(|&&t| t > 0.25 * period_ns).count(),
                    over_period: times.iter().filter(|&&t| t > period_ns).count(),
                    block: Spread::of(times),
                    tail,
                };
                if !json {
                    eprintln!(
                        "signal: bench: realtime: {threads} threads, {name}, {variant}: median {:.1} µs, \
                     p99 {:.1} µs; workers started p99 {:.1} µs, slowest task p99 {:.1} µs",
                        row.block.median / 1e3,
                        row.block.p99 / 1e3,
                        row.tail.worker_start.p99 / 1e3,
                        row.tail.slowest_task.p99 / 1e3
                    );
                }
                rows.push(row);
            }
        }
    }

    let paths = |paths: &[WorkerPath]| -> String {
        let mut names: Vec<String> = paths.iter().map(ToString::to_string).collect();
        names.dedup();
        if names.is_empty() {
            "ordinary".to_owned()
        } else {
            names.join(", ")
        }
    };
    if json {
        let output = json!({
            "label": "measurement",
            "table": "realtime_workers",
            "profile": profile,
            "machine": machine(),
            "sample_rate": SAMPLE_RATE,
            "block_frames": RT_BLOCK,
            "period_ns": period_ns,
            "voices": voices,
            "blocks_per_row": blocks,
            "performance_cores": cores,
            "cores_source": cores_source.name(),
            "policy": {
                "timebase": [timebase.0, timebase.1],
                "period_ticks": policy.period,
                "computation_ticks": policy.computation,
                "constraint_ticks": policy.constraint,
            },
            "rows": rows.iter().map(|r| json!({
                "threads": r.threads,
                "workers": r.workers,
                "schedule": r.schedule,
                "variant": r.variant,
                "caller": r.caller.as_ref().map_or("ordinary".to_owned(), ToString::to_string),
                "paths": r.paths.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "median_ns": r.block.median,
                "p99_ns": r.block.p99,
                "max_ns": r.block.max,
                "median_fraction": r.block.median / period_ns,
                "p99_fraction": r.block.p99 / period_ns,
                "over_25_percent": r.over_quarter,
                "over_period": r.over_period,
                "tail": r.tail.json(),
            })).collect::<Vec<_>>(),
        });
        print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
    } else {
        let mut text = format!(
            "real-time workers: {voices} kick voices, blocks of {RT_BLOCK} frames paced at {:.1} µs, \
             {blocks} blocks per row, {cores} performance cores ({})\n\
             µs per block; workers started (from the first thread in the job) and slowest task: \
             median / p99 / max\n\
             threads  schedule  variant       median     p99     max  >25 %  >100 %  \
             workers started        slowest task           workers",
            period_ns / 1e3,
            cores_source.name()
        );
        let spread = |s: &Spread| {
            format!(
                "{:.1} / {:.1} / {:.1}",
                s.median / 1e3,
                s.p99 / 1e3,
                s.max / 1e3
            )
        };
        for r in &rows {
            text.push_str(&format!(
                "\n  {:>5}  {:<8}  {:<11} {:>7.1} {:>7.1} {:>7.1} {:>6} {:>7}  {:<22} {:<22} {}",
                r.threads,
                r.schedule,
                r.variant,
                r.block.median / 1e3,
                r.block.p99 / 1e3,
                r.block.max / 1e3,
                r.over_quarter,
                r.over_period,
                spread(&r.tail.worker_start),
                spread(&r.tail.slowest_task),
                paths(&r.paths)
            ));
        }
        print_stdout(&text);
    }
    Ok(())
}

/// Voice counts with their timings, in order.
type Measured = BTreeMap<usize, Timing>;

/// The most voices whose median stays within each limit of `within`, by
/// doubling until over the larger limit and then bisecting below each limit,
/// with every count measured (by `timing`, once each).
#[allow(clippy::type_complexity)]
fn most_voices(
    within: [f64; 2],
    mut timing: impl FnMut(usize) -> Result<Timing, Failure>,
) -> Result<(Vec<(usize, Option<Timing>)>, Measured), Failure> {
    let mut measured = Measured::new();
    let mut voice_timing = |n: usize| -> Result<Timing, Failure> {
        if let Some(timing) = measured.get(&n) {
            return Ok(*timing);
        }
        let result = timing(n)?;
        measured.insert(n, result);
        Ok(result)
    };
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
    Ok((most, measured))
}

/// A plan's estimated cost per frame in ns (the engine's, from the static
/// costs), its units and its tasks with one task per unit.
fn estimate_of(plan: &ExecutionPlan, registry: &Registry) -> Result<(u64, usize, usize), Failure> {
    let config = EngineConfig {
        threads: 1,
        schedule: Schedule::FORCED,
        ..EngineConfig::new(SAMPLE_RATE, SEED)
    };
    let engine = Engine::new(plan, registry, config, Vec::new())
        .map_err(|e| Failure::Internal(e.to_string()))?;
    let stats = engine.schedule_stats();
    Ok((stats.estimate_ns, stats.units, stats.tasks))
}

/// Times `plan` for about `seconds` one frame at a time: see [`measure_blocks`].
fn measure(plan: &ExecutionPlan, registry: &Registry, seconds: f64) -> Result<Timing, Failure> {
    measure_blocks(plan, registry, seconds, 1, ONE_THREAD)
}

/// How a case runs: its threads, the calling thread included, and when a
/// segment runs on the workers.
#[derive(Clone, Copy)]
struct Threads {
    threads: usize,
    schedule: Schedule,
}

const ONE_THREAD: Threads = Threads {
    threads: 1,
    schedule: Schedule::DEFAULT,
};

/// Times `plan` for about `seconds` in blocks of `k` frames (a divisor of
/// [`REPEAT_FRAMES`]) on `on.threads` threads: repeats of [`REPEAT_FRAMES`]
/// frames after one untimed repeat.
fn measure_blocks(
    plan: &ExecutionPlan,
    registry: &Registry,
    seconds: f64,
    k: usize,
    on: Threads,
) -> Result<Timing, Failure> {
    let config = EngineConfig {
        block_capacity: k,
        threads: on.threads,
        schedule: on.schedule,
        ..EngineConfig::new(SAMPLE_RATE, SEED)
    };
    let mut engine = Engine::new(plan, registry, config, Vec::new())
        .map_err(|e| Failure::Internal(e.to_string()))?;
    let blocks = REPEAT_FRAMES / k as u64;
    for _ in 0..blocks {
        engine.run_block(k);
    }
    let mut repeats = Vec::new();
    let start = Instant::now();
    while repeats.len() < MIN_REPEATS || start.elapsed().as_secs_f64() < seconds {
        let began = Instant::now();
        for _ in 0..blocks {
            engine.run_block(k);
        }
        repeats.push(began.elapsed().as_nanos() as f64 / REPEAT_FRAMES as f64);
    }
    Ok(timing_of(repeats))
}

/// The median and p99 (nearest rank) of `repeats`.
fn timing_of(mut repeats: Vec<f64>) -> Timing {
    repeats.sort_by(f64::total_cmp);
    let rank = |p: f64| {
        let k = ((p * repeats.len() as f64).ceil() as usize).clamp(1, repeats.len());
        repeats.get(k - 1).copied().unwrap_or_default()
    };
    Timing {
        median: rank(0.5),
        p99: rank(0.99),
        repeats: repeats.len(),
    }
}

/// The GPU partial bank's figures, in ns per block of the bank.
#[cfg(feature = "lab")]
struct GpuBench {
    /// The adapter, if there is one.
    adapter: Option<String>,
    /// [`reference`](signal_lab::gpu_partials::reference): the block on the CPU.
    cpu: Timing,
    /// What the module does on the audio thread per block: the phases and
    /// the job's input, and the submission.
    audio_side: Timing,
    /// From submission to the result in hand, on the calling thread; or why
    /// it was not measured (no usable GPU, or the channel failed).
    round_trip: Result<Timing, String>,
}

#[cfg(feature = "lab")]
impl GpuBench {
    fn json(&self) -> Value {
        use signal_lab::gpu_partials::{BLOCK_FRAMES, LATENCY_BLOCKS, PARTIALS};
        let block_ns = BLOCK_FRAMES as f64 * budget_ns();
        json!({
            "partials": PARTIALS,
            "block_frames": BLOCK_FRAMES,
            "latency_blocks": LATENCY_BLOCKS,
            "block_ns": block_ns,
            "adapter": self.adapter,
            "no_gpu": self.round_trip.as_ref().err(),
            "per_block": {
                "cpu_reference": self.cpu.json(),
                "gpu_round_trip": self.round_trip.as_ref().ok().map(Timing::json),
                "audio_side": self.audio_side.json(),
            },
        })
    }

    fn text(&self) -> String {
        use signal_lab::gpu_partials::{BLOCK_FRAMES, LATENCY_BLOCKS, PARTIALS};
        let block_ns = BLOCK_FRAMES as f64 * budget_ns();
        let row = |name: &str, t: &Timing| {
            format!(
                "\n  {name:<24} {:>12.0} {:>12.0}  {:>6.1} % of a block",
                t.median,
                t.p99,
                100.0 * t.median / block_ns
            )
        };
        let mut text = format!(
            "GPU partial bank (lab.gpu_partials: {PARTIALS} partials, {BLOCK_FRAMES}-frame blocks \
             of {block_ns:.0} ns, {LATENCY_BLOCKS} blocks late; median, p99 ns per block):"
        );
        if let Some(adapter) = &self.adapter {
            text.push_str(&format!("\n  adapter {adapter}"));
        }
        match &self.round_trip {
            Ok(round_trip) => text.push_str(&row("GPU round trip", round_trip)),
            Err(why) => text.push_str(&format!("\n  GPU round trip not measured: {why}")),
        }
        text.push_str(&row("CPU reference", &self.cpu));
        text.push_str(&row("audio side (submit)", &self.audio_side));
        text
    }
}

/// Times the GPU partial bank for about `seconds` per row: the CPU
/// reference of a block, the module's audio-side work per block, and the
/// GPU's round trip (submit, compute on the `signal-gpu` thread, result
/// back), one block at a time, with the parameters changing every block.
#[cfg(feature = "lab")]
fn gpu_partials(seconds: f64, json: bool) -> GpuBench {
    use signal_gpu::GpuService;
    use signal_lab::gpu_partials::{BLOCK_FRAMES, Bank, KERNEL, Params, pack, reference};

    let params = |n: usize| Params {
        freq: 55.0 * (1.0 + 0.01 * (n % 7) as f64),
        ..Params::default()
    };
    let time = |mut block: Box<dyn FnMut(usize) + '_>| {
        let mut repeats = Vec::new();
        let start = Instant::now();
        while repeats.len() < MIN_REPEATS || start.elapsed().as_secs_f64() < seconds {
            let began = Instant::now();
            block(repeats.len());
            repeats.push(began.elapsed().as_nanos() as f64);
        }
        timing_of(repeats)
    };
    let rate = f64::from(SAMPLE_RATE);
    let mut bank = Bank::new(rate);
    let mut out = vec![0.0; BLOCK_FRAMES];
    let cpu = time(Box::new(|n| {
        reference(bank.next_block(params(n)), &mut out)
    }));
    let mut input = vec![0.0f32; KERNEL.input_words];
    let audio_side = time(Box::new(|n| pack(bank.next_block(params(n)), &mut input)));
    let service = GpuService::shared();
    let adapter = service.map(|service| service.adapter().to_owned());
    let channel = match service {
        Some(service) => service.channel(KERNEL, 1).map_err(|e| e.to_string()),
        None => {
            Err(GpuService::shared_error().map_or("no usable GPU".to_owned(), ToString::to_string))
        }
    };
    let round_trip = channel.map(|mut channel| {
        time(Box::new(move |n| {
            let block = bank.next_block(params(n));
            if channel.submit(n as u64, |input| pack(block, input)) {
                let began = Instant::now();
                while channel.peek_done().is_none() && began.elapsed() < GPU_GIVE_UP {
                    std::hint::spin_loop();
                }
                if let Some(job) = channel.take_done() {
                    channel.recycle(job);
                }
            }
        }))
    });
    if !json {
        eprintln!(
            "signal: bench: GPU partial bank: CPU {:.0} ns, GPU {} per block",
            cpu.median,
            round_trip
                .as_ref()
                .map_or("not measured".to_owned(), |t| format!("{:.0} ns", t.median))
        );
    }
    GpuBench {
        adapter,
        cpu,
        audio_side,
        round_trip,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ok<T>(result: Result<T, Failure>) -> T {
        result.unwrap_or_else(|_| panic!("a bench helper failed"))
    }

    /// The written project is the bench's voices graph, instance for
    /// instance and connection for connection.
    #[test]
    fn the_written_project_is_the_voices_graph() {
        let mut registry = Registry::new();
        signal_std::register(&mut registry).unwrap();
        let kick = ok(kick_project());
        for n in [1, 3, 17] {
            let root = ok(voices_graph(&kick.root, n));
            let text = ok(project_text("kick-voices", 96_000, 42, n, &root));
            let mut files: Vec<SourceFile> = KICK
                .iter()
                .filter(|(path, _)| *path != "project.toml")
                .map(|(path, text)| SourceFile {
                    path: (*path).to_owned(),
                    text: (*text).to_owned(),
                })
                .collect();
            files.push(SourceFile {
                path: "project.toml".to_owned(),
                text,
            });
            let written = Project::from_sources(files).unwrap();
            let sorted = |c: &CompositeDef| {
                let mut instances = c.instances.clone();
                instances.sort_by(|a, b| a.name.cmp(&b.name));
                let mut connections = c.connections.clone();
                connections.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
                (instances, connections)
            };
            assert_eq!(sorted(&written.root), sorted(&root), "{n} voices");
            assert_eq!((written.sample_rate, written.seed), (96_000, 42));
            let planned = written.plan(&registry).unwrap().plan;
            let reference = ok(plan_of(&root, &kick.composites, &registry));
            assert_eq!(planned.instances.len(), reference.instances.len());
        }
    }
}
