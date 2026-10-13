//! Block execution (M2.6-1, ADR 0019 §1): the determinism matrix (AC1), the
//! cycle units (AC3) and `signal render --block` (C5). Audio and log are
//! bit-identical for every block size and a seeded random partition, and
//! identical to the per-frame engine: the kick, pulse and session hashes
//! below were recorded with the per-frame engine at 72886ec, before block
//! execution existed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use signal_core::{Curve, EventPayload, Frame, Registry, SplitMix64};
use signal_engine::{
    Engine, EngineConfig, EngineError, ExternalEvent, HookPoint, Render, Schedule, render_blocks,
    render_with,
};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, InstanceDef, build_plan};
use signal_project::{Project, SourceFile};

/// The block sizes of the matrix (AC1).
const SIZES: [usize; 7] = [1, 2, 3, 7, 64, 128, 256];

/// M2.6-2's thread counts and block sizes (ADR 0019 §2).
const THREADS: [usize; 5] = [1, 2, 3, 4, 8];
const THREAD_SIZES: [usize; 3] = [1, 64, 256];

/// Frames of the kick and pulse rows (about 2 s at 96 kHz).
const PROJECT_FRAMES: u64 = 200_000;

/// (audio FNV, log FNV) of the per-frame engine at 72886ec, recorded with
/// `signal --json render projects/<p> --frames 200000` built from the
/// unmodified worktree, before any edit of M2.6-1.
const KICK_PER_FRAME: (&str, &str) = ("bb216fac2da73b75", "0da70e67fa22289f");
const PULSE_PER_FRAME: (&str, &str) = ("d449c12fda759cbd", "a0635e895edc5db3");
/// The same for the scripted session, recorded with [`session_script`] driven
/// by `render_with` (the per-frame driver) in a `git archive 72886ec` extract.
const SESSION_PER_FRAME: (&str, &str) = ("ce683a958730ddbd", "e9834facf22432ea");

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn hashes(render: &Render) -> (String, String) {
    (
        format!("{:016x}", render.audio_fnv1a64()),
        format!("{:016x}", render.log_fnv1a64()),
    )
}

/// How the frames are split into blocks.
#[derive(Clone, Copy, Debug)]
enum Partition {
    /// One frame per call, through `render_with`: the per-frame driver.
    PerFrame,
    Blocks(usize),
    Random(u64),
}

fn partitions() -> impl Iterator<Item = Partition> {
    SIZES
        .into_iter()
        .map(Partition::Blocks)
        .chain([Partition::Random(7)])
}

/// The configuration on `threads` threads: with more than one, every segment
/// of a plan with two tasks or more runs on the workers, one task per unit.
fn config(sample_rate: u32, seed: u64, threads: usize) -> EngineConfig {
    EngineConfig {
        threads,
        schedule: if threads > 1 {
            Schedule::FORCED
        } else {
            Schedule::DEFAULT
        },
        ..EngineConfig::new(sample_rate, seed)
    }
}

/// Renders on workers in this file run one at a time, so that
/// [`WORKER_TASKS`] counts only theirs.
static THREADED: Mutex<()> = Mutex::new(());

/// Tasks a worker started in the threaded render now running.
static WORKER_TASKS: AtomicU64 = AtomicU64::new(0);

/// The task hook of a threaded render: the calling thread waits (up to
/// 100 ms) at the start of each job until a worker has started a task, so
/// that workers do run tasks even where the calling thread alone would be
/// quicker (release builds). Timing only: it changes no output.
fn workers_take_part(point: HookPoint) {
    match point {
        HookPoint::Enter { lane: 0 } => {
            let before = WORKER_TASKS.load(Ordering::SeqCst);
            let start = Instant::now();
            while WORKER_TASKS.load(Ordering::SeqCst) == before
                && start.elapsed() < Duration::from_millis(100)
            {
                std::hint::spin_loop();
            }
        }
        HookPoint::Task { lane, .. } if lane > 0 => {
            WORKER_TASKS.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
}

/// Renders `frames` of `plan` split by `partition` on `threads` threads,
/// calling `before` (which returns the next frame it must run before) as
/// `render_blocks` does. With workers, asserts that they ran tasks.
fn render(
    plan: &ExecutionPlan,
    (sample_rate, seed, frames): (u32, u64, u64),
    partition: Partition,
    threads: usize,
    mut before: impl FnMut(&mut Engine) -> Result<Option<Frame>, EngineError>,
) -> Render {
    let registry = registry();
    let config = config(sample_rate, seed, threads);
    let mut engine = Engine::new(plan, &registry, config, Vec::new()).unwrap();
    let _threaded = (threads > 1).then(|| {
        engine.set_task_hook(Some(workers_take_part));
        THREADED.lock().unwrap_or_else(|e| e.into_inner())
    });
    let render = match partition {
        Partition::PerFrame => render_with(&mut engine, frames, |e| before(e).map(|_| ())),
        Partition::Blocks(k) => render_blocks(&mut engine, frames, k, before),
        Partition::Random(seed) => {
            let mut rng = SplitMix64::new(seed);
            render_blocks(&mut engine, frames, 256, |engine| {
                let next = before(engine)?;
                let cut = Frame(engine.next_frame().0 + 1 + rng.next_u64() % 256);
                Ok(Some(next.map_or(cut, |at| at.min(cut))))
            })
        }
    }
    .unwrap();
    assert_eq!(engine.log_overflows(), 0);
    assert_eq!(engine.input_overflows(), 0);
    if threads > 1 {
        let stats = engine.schedule_stats();
        assert!(stats.worker_tasks > 0, "{stats:?}");
    }
    render
}

/// The thread grid of M2.6-2's matrix (ADR 0019 §2): every thread count at
/// block sizes 1, 64 and 256.
fn thread_grid() -> impl Iterator<Item = (Partition, usize)> {
    THREADS
        .into_iter()
        .flat_map(|threads| THREAD_SIZES.map(|k| (Partition::Blocks(k), threads)))
}

/// Nothing was refused; and where the checked mode's exclusivity tracker is
/// compiled in (debug builds), it ran.
fn assert_no_violation() {
    let exclusivity = signal_engine::exclusivity();
    assert_eq!(exclusivity.violations, 0);
    if exclusivity.checked {
        assert!(exclusivity.claims > 0);
    }
}

fn project_matrix(
    name: &str,
    per_frame: (&str, &str),
    runs: impl Iterator<Item = (Partition, usize)>,
) {
    let project = Project::load(&repository().join("projects").join(name)).unwrap();
    let plan = project.plan(&registry()).unwrap().plan;
    let expected = (per_frame.0.to_owned(), per_frame.1.to_owned());
    for (partition, threads) in runs {
        let run = (project.sample_rate, project.seed, PROJECT_FRAMES);
        let rendered = render(&plan, run, partition, threads, |_| Ok(None));
        assert_eq!(
            hashes(&rendered),
            expected,
            "{name}, {partition:?}, {threads} threads"
        );
    }
}

/// AC1: the kick project at every block size equals the per-frame engine.
#[test]
fn kick_is_bit_identical_in_blocks() {
    project_matrix("kick", KICK_PER_FRAME, partitions().map(|p| (p, 1)));
}

/// M2.6-2 AC1: on 1, 2, 3, 4 and 8 threads at k = 1, 64 and 256, every
/// segment on the workers, the kick equals the per-frame engine.
#[test]
fn kick_is_bit_identical_on_threads() {
    project_matrix("kick", KICK_PER_FRAME, thread_grid());
    assert_no_violation();
}

/// M2.6-2 AC1: the pulse project (a loop through `std.delay`) likewise.
#[test]
fn pulse_is_bit_identical_on_threads() {
    project_matrix("pulse", PULSE_PER_FRAME, thread_grid());
    assert_no_violation();
}

/// AC1: the pulse project (a loop through `std.delay`) likewise.
#[test]
fn pulse_is_bit_identical_in_blocks() {
    project_matrix("pulse", PULSE_PER_FRAME, partitions().map(|p| (p, 1)));
}

/// Input pushed during the scripted session.
enum Cmd {
    /// An external event and its stamp (earlier than the push: late).
    Event(u64, &'static str, EventPayload),
    Suspend(u64, &'static str),
    Resume(u64, &'static str),
    Swap(usize),
}

/// The scripted session (`tests/fixtures/session/v0..v2`): sets and a ramp
/// pushed ahead to land inside blocks, late events, a structural swap that
/// adds `/osc2`, its suspend-then-remove at the fade's end (`removal`), and a
/// suspend and resume of `/env`. (frame pushed before, command).
fn session_script(removal: u64) -> Vec<(u64, Cmd)> {
    use EventPayload::{Ramp, Set, Trigger};
    vec![
        (0, Cmd::Event(100, "/osc.freq", Set(550.0))),
        (
            0,
            Cmd::Event(
                200,
                "/out.gain",
                Ramp {
                    to: 0.25,
                    frames: 400,
                    curve: Curve::Linear,
                },
            ),
        ),
        (0, Cmd::Event(250, "/env.trigger", Trigger)),
        (300, Cmd::Event(290, "/osc.freq", Set(500.0))),
        (700, Cmd::Event(650, "/env.trigger", Trigger)),
        (1000, Cmd::Swap(1)),
        (1000, Cmd::Event(1000, "/osc.freq", Set(470.0))),
        (1000, Cmd::Event(1000, "/osc2.freq", Set(990.0))),
        (1000, Cmd::Event(1200, "/osc2.freq", Set(330.0))),
        (1000, Cmd::Suspend(1500, "/osc2")),
        (removal, Cmd::Swap(2)),
        (removal, Cmd::Suspend(2500, "/env")),
        (removal, Cmd::Resume(2600, "/env")),
        (2000, Cmd::Event(1990, "/env.trigger", Trigger)),
    ]
}

/// Pushes the session's commands due at the next frame; returns the next
/// command's frame.
fn session_step(
    engine: &mut Engine,
    plans: &[ExecutionPlan],
    registry: &Registry,
    script: &[(u64, Cmd)],
    next: &mut usize,
) -> Result<Option<Frame>, EngineError> {
    engine.reclaim_externals().for_each(drop);
    engine.reclaim_edits().for_each(drop);
    let now = engine.next_frame();
    while let Some((_, cmd)) = script.get(*next).filter(|(at, _)| *at == now.0) {
        *next += 1;
        match *cmd {
            Cmd::Event(stamp, target, payload) => engine
                .push_external(ExternalEvent {
                    frame: Frame(stamp),
                    sequence: *next as u64,
                    target: target.parse().unwrap(),
                    payload,
                })
                .unwrap(),
            Cmd::Suspend(frame, a) => engine
                .push_suspend(a.parse().unwrap(), Frame(frame))
                .unwrap(),
            Cmd::Resume(frame, a) => engine
                .push_resume(a.parse().unwrap(), Frame(frame))
                .unwrap(),
            Cmd::Swap(v) => {
                let mut swap = engine.prepare_swap(&plans[v], registry)?;
                engine.commit_swap(&mut swap, now)?;
            }
        }
    }
    Ok(script.get(*next).map(|(at, _)| Frame(*at)))
}

/// AC1: the scripted session (sets, a structural swap, a suspend-then-remove
/// and late events) at every block size equals the per-frame engine.
#[test]
fn a_scripted_session_is_bit_identical_in_blocks() {
    session_matrix(
        [Partition::PerFrame]
            .into_iter()
            .chain(partitions())
            .map(|p| (p, 1)),
    );
}

/// M2.6-2 AC1: the scripted session on 1, 2, 3, 4 and 8 threads at k = 1, 64
/// and 256 (every segment on the workers, swaps included) likewise.
#[test]
fn a_scripted_session_is_bit_identical_on_threads() {
    session_matrix(thread_grid());
    assert_no_violation();
}

fn session_matrix(runs: impl Iterator<Item = (Partition, usize)>) {
    let registry = registry();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/session");
    let projects: Vec<Project> = (0..3)
        .map(|v| Project::load(&dir.join(format!("v{v}"))).unwrap())
        .collect();
    let plans: Vec<ExecutionPlan> = projects
        .iter()
        .enumerate()
        .map(|(v, project)| {
            let mut plan = project.plan(&registry).unwrap().plan;
            plan.version = v as u64;
            plan
        })
        .collect();
    let (rate, seed) = (projects[0].sample_rate, projects[0].seed);
    // F = 40 frames at 8 kHz.
    let script = session_script(1540);
    let expected = (
        SESSION_PER_FRAME.0.to_owned(),
        SESSION_PER_FRAME.1.to_owned(),
    );
    for (partition, threads) in runs {
        let mut next = 0;
        let rendered = render(
            &plans[0],
            (rate, seed, 3_000),
            partition,
            threads,
            |engine| session_step(engine, &plans, &registry, &script, &mut next),
        );
        assert_eq!(next, script.len());
        assert_eq!(
            hashes(&rendered),
            expected,
            "session, {partition:?}, {threads} threads"
        );
    }
}

/// The kick project with its `drums` instance repeated `n` times (every
/// connection to and from it repeated) and the copies summed by a tree of
/// 4-input `std.mix` into the sink: `signal bench`'s kick voices.
fn voices(n: usize) -> (ExecutionPlan, u32, u64) {
    let project = Project::load(&repository().join("projects/kick")).unwrap();
    let kick = &project.root;
    let drums = kick.instances.iter().find(|i| i.name == "drums").unwrap();
    let mut root = CompositeDef {
        instances: kick
            .instances
            .iter()
            .filter(|i| i.name != "drums")
            .cloned()
            .collect(),
        connections: Vec::new(),
        ..kick.clone()
    };
    let mut outputs: Vec<String> = Vec::new();
    for i in 0..n {
        let name = format!("drums_{i}");
        root.instances.push(InstanceDef {
            name: name.clone(),
            ..drums.clone()
        });
        outputs.push(format!("{name}.out"));
    }
    let connect = |from: &str, to: &str, scale| ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale,
    };
    let mut sink_inputs = Vec::new();
    for c in &kick.connections {
        match (c.from.split_once('.'), c.to.split_once('.')) {
            (Some(("drums", _)), _) => sink_inputs.push(c.to.clone()),
            (_, Some(("drums", port))) => {
                for i in 0..n {
                    let to = format!("drums_{i}.{port}");
                    root.connections.push(connect(&c.from, &to, c.scale));
                }
            }
            _ => root.connections.push(c.clone()),
        }
    }
    let mut level = 0;
    while outputs.len() > 1 || level == 0 {
        let mut next = Vec::new();
        for (j, group) in outputs.chunks(4).enumerate() {
            let name = format!("mix_{level}_{j}");
            root.instances.push(InstanceDef {
                name: name.clone(),
                module: "std.mix".to_owned(),
                parameters: Default::default(),
            });
            for (input, from) in ["a", "b", "c", "d"].iter().zip(group) {
                root.connections
                    .push(connect(from, &format!("{name}.{input}"), None));
            }
            next.push(format!("{name}.out"));
        }
        outputs = next;
        level += 1;
    }
    for to in sink_inputs {
        for from in &outputs {
            root.connections.push(connect(from, &to, None));
        }
    }
    let plan = build_plan(&root, &project.composites, &registry(), 0).unwrap();
    (plan, project.sample_rate, project.seed)
}

/// Frames of the 32-voice rows.
const VOICES_FRAMES: u64 = 20_000;

/// M2.6-2 AC1: a 32-voice kick (about 300 instances) on 1, 2, 3, 4 and 8
/// threads at k = 1, 64 and 256 equals its own per-frame render.
#[test]
fn thirty_two_kick_voices_are_bit_identical_on_threads() {
    let (plan, rate, seed) = voices(32);
    let run = (rate, seed, VOICES_FRAMES);
    let reference = hashes(&render(&plan, run, Partition::PerFrame, 1, |_| Ok(None)));
    for (partition, threads) in thread_grid() {
        let rendered = render(&plan, run, partition, threads, |_| Ok(None));
        assert_eq!(
            hashes(&rendered),
            reference,
            "32 voices, {partition:?}, {threads} threads"
        );
    }
    // The default schedule (tasks chained by cost) gives the same.
    let registry = registry();
    let mut engine = Engine::new(
        &plan,
        &registry,
        EngineConfig {
            threads: 8,
            ..EngineConfig::new(rate, seed)
        },
        Vec::new(),
    )
    .unwrap();
    let rendered =
        signal_engine::render_blocks(&mut engine, VOICES_FRAMES, 256, |_| Ok(None)).unwrap();
    assert_eq!(hashes(&rendered), reference, "32 voices, default schedule");
    let stats = engine.schedule_stats();
    assert!(stats.tasks <= stats.units, "{stats:?}");
    assert_no_violation();
}

/// A one-file project at 8 kHz.
fn project(instances: &str, connections: &str) -> Project {
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"loop\"\nsample_rate = 8000\nseed = 5\n\n\
         [instances]\n{instances}\n[wiring]\nconnections = [\n{connections}]\n"
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap()
}

/// Renders `project` per frame and in every partition, with parameter events
/// pushed at frames inside the blocks, and requires the same audio and log.
fn loop_matrix(project: &Project, events: &[(u64, &'static str, f64)]) {
    let plan = project.plan(&registry()).unwrap().plan;
    let run = |partition| {
        let mut pushed = false;
        render(&plan, (8_000, 5, 4_000), partition, 1, |engine| {
            if !pushed {
                pushed = true;
                for (n, &(frame, target, value)) in events.iter().enumerate() {
                    engine
                        .push_external(ExternalEvent {
                            frame: Frame(frame),
                            sequence: n as u64,
                            target: target.parse().unwrap(),
                            payload: EventPayload::Set(value),
                        })
                        .unwrap();
                }
            }
            Ok(None)
        })
    };
    let reference = hashes(&run(Partition::PerFrame));
    for partition in partitions() {
        assert_eq!(hashes(&run(partition)), reference, "{partition:?}");
    }
}

/// AC3: a feedback loop through `std.unit_delay` (with a direct gain in the
/// loop) runs in one-frame sub-blocks and matches per-frame exactly.
#[test]
fn a_loop_through_a_unit_delay_matches_per_frame() {
    let project = project(
        "osc = { module = \"std.sine_osc\", freq = 440 }\n\
         sum = { module = \"std.add\" }\n\
         amp = { module = \"std.gain\", db = -1 }\n\
         fb = { module = \"std.unit_delay\" }\n\
         out = { module = \"std.audio_out\" }\n",
        "  { from = \"osc.out\", to = \"sum.a\" },\n\
         \x20 { from = \"fb.out\", to = \"sum.b\", scale = 0.7 },\n\
         \x20 { from = \"sum.out\", to = \"amp.in\" },\n\
         \x20 { from = \"amp.out\", to = \"fb.in\" },\n\
         \x20 { from = \"amp.out\", to = \"out.l\" },\n\
         \x20 { from = \"fb.out\", to = \"out.r\" },\n",
    );
    loop_matrix(
        &project,
        &[(37, "/osc.freq", 880.0), (1_000, "/amp.db", -6.0)],
    );
}

/// AC3: a feedback loop closed by `std.delay` at 1, 2 and 3 frames (0.125 ms
/// per frame at 8 kHz) matches per-frame exactly.
#[test]
fn a_loop_through_std_delay_at_one_to_three_frames_matches_per_frame() {
    for frames in [1.0, 2.0, 3.0] {
        let time_ms = frames * 0.125;
        let project = project(
            &format!(
                "osc = {{ module = \"std.sine_osc\", freq = 300 }}\n\
                 sum = {{ module = \"std.add\" }}\n\
                 dly = {{ module = \"std.delay\", time_ms = {time_ms} }}\n\
                 out = {{ module = \"std.audio_out\" }}\n"
            ),
            "  { from = \"osc.out\", to = \"sum.a\" },\n\
             \x20 { from = \"dly.out\", to = \"sum.b\", scale = 0.6 },\n\
             \x20 { from = \"sum.out\", to = \"dly.in\" },\n\
             \x20 { from = \"sum.out\", to = \"out.l\" },\n\
             \x20 { from = \"dly.out\", to = \"out.r\" },\n",
        );
        loop_matrix(&project, &[(501, "/osc.freq", 450.0)]);
    }
}

fn signal(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .expect("the signal binary runs")
}

/// C5: `signal render --block <k>` writes the same files as the default
/// (256), and block sizes outside 1–4096 are refused.
#[test]
fn render_block_option_changes_nothing_in_the_output() {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("blocks");
    let _ = fs::remove_dir_all(&tmp);
    let kick = repository().join("projects/kick");
    let kick = kick.to_str().unwrap();
    let render_to = |name: &str, extra: &[&str]| {
        let out = tmp.join(name);
        let mut args = vec!["render", kick, "--frames", "20000", "--out"];
        args.push(out.to_str().unwrap());
        args.extend_from_slice(extra);
        let output = signal(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        out
    };
    let default = render_to("default", &[]);
    // M2.6-2 AC9: `--threads` (default 0, auto) changes nothing either.
    let options: [&[&str]; 6] = [
        &["--block", "1"],
        &["--block", "7"],
        &["--block", "4096"],
        &["--threads", "1"],
        &["--threads", "4"],
        &["--threads", "3", "--block", "64"],
    ];
    for option in options {
        let k = option.join(" ");
        let other = render_to(&k.replace(' ', "_"), option);
        for file in ["audio.wav", "log.jsonl"] {
            let bytes = |dir: &Path| fs::read(dir.join(file)).unwrap();
            assert!(bytes(&other) == bytes(&default), "--block {k}: {file}");
        }
        // The manifest's provenance names the build; its hashes must match.
        let manifest_hashes = |dir: &Path| {
            fs::read_to_string(dir.join("manifest.json"))
                .unwrap()
                .lines()
                .filter(|l| l.contains("fnv1a64") || l.contains("\"frames\""))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            manifest_hashes(&other),
            manifest_hashes(&default),
            "--block {k}: manifest"
        );
    }
    for k in ["0", "4097"] {
        let out = tmp.join("refused");
        let output = signal(&[
            "render",
            kick,
            "--frames",
            "10",
            "--out",
            out.to_str().unwrap(),
            "--block",
            k,
        ]);
        assert!(!output.status.success(), "--block {k} is refused");
        assert!(!out.exists());
    }
}
