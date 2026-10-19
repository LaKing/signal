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
use signal_engine::{Engine, EngineConfig, EngineError, ExternalEvent, HookPoint, Schedule};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, InstanceDef, build_plan};
use signal_output::{
    Render, VectorTap, VectorTrack, render_blocks, render_blocks_tapped, render_with,
};
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
        signal_output::render_blocks(&mut engine, VOICES_FRAMES, 256, |_| Ok(None)).unwrap();
    assert_eq!(hashes(&rendered), reference, "32 voices, default schedule");
    let stats = engine.schedule_stats();
    assert!(stats.tasks <= stats.units, "{stats:?}");
    assert_no_violation();
}

/// M2.10-5 AC5: under the default schedule (the worker budget on), 80 kick
/// voices get every thread on 2, 8, 12 (the real-time auto count of the
/// M5 Max) and 18 (all its performance cores); the kick alone, one voice,
/// does not get the 12. The values are the budget rule's for these plans
/// (`tasks.rs`, `worker_budget`), read back from the engine.
#[test]
fn eighty_kick_voices_get_every_thread() {
    let registry = registry();
    let budget = |plan: &ExecutionPlan, rate, seed, threads| {
        let config = EngineConfig {
            threads,
            ..EngineConfig::new(rate, seed)
        };
        let engine = Engine::new(plan, &registry, config, Vec::new()).unwrap();
        engine.schedule_stats().worker_budget
    };
    let (plan, rate, seed) = voices(80);
    for threads in [2, 8, 12, 18] {
        assert_eq!(budget(&plan, rate, seed, threads), threads, "80 voices");
    }
    let (kick, rate, seed) = voices(1);
    let one = budget(&kick, rate, seed, 12);
    assert!(one < 12, "one voice: {one}");
}

/// M2.10-6 AC4–AC6: in real time at 128-frame blocks (the real-time
/// default), the projects whose serial estimate fits in `SERIAL_SHARE` of
/// the block's period (the studio, kick, pulse, physical-av, 4 kick voices)
/// get one thread, so no worker is woken for them, while 80 kick voices
/// still get every thread on 2, 8, 12 and 18; also after a swap from the
/// kick to the 80 voices, as `prepare_swap` builds the budget again.
#[test]
fn in_real_time_light_projects_run_alone_and_eighty_voices_get_every_thread() {
    let registry = registry();
    let config = |rate, seed, threads| EngineConfig {
        threads,
        block_capacity: 128,
        realtime: true,
        ..EngineConfig::new(rate, seed)
    };
    let budget = |plan: &ExecutionPlan, rate, seed, threads| {
        let engine = Engine::new(plan, &registry, config(rate, seed, threads), Vec::new()).unwrap();
        engine.schedule_stats().worker_budget
    };
    for name in ["studio", "kick", "pulse", "physical-av"] {
        let project = Project::load(&repository().join("projects").join(name)).unwrap();
        let plan = project.plan(&registry).unwrap().plan;
        for threads in [12, 18] {
            let got = budget(&plan, project.sample_rate, project.seed, threads);
            assert_eq!(got, 1, "{name} on {threads} threads");
        }
    }
    let (four, rate, seed) = voices(4);
    assert_eq!(budget(&four, rate, seed, 12), 1, "4 voices");
    let (eighty, rate, seed) = voices(80);
    for threads in [2, 8, 12, 18] {
        assert_eq!(budget(&eighty, rate, seed, threads), threads, "80 voices");
    }
    let (kick, rate, seed) = voices(1);
    let mut engine = Engine::new(&kick, &registry, config(rate, seed, 12), Vec::new()).unwrap();
    assert_eq!(engine.schedule_stats().worker_budget, 1);
    let mut swap = engine.prepare_swap(&eighty, &registry).unwrap();
    engine.commit_swap(&mut swap, Frame(0)).unwrap();
    assert_eq!(engine.schedule_stats().worker_budget, 12, "after the swap");
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

/// M2.10-1 AC14: a graph of the new core types (a phase from `std.frames`
/// and `std.sample_rate`, `std.sin_cycles`, `std.compare`, `std.round`,
/// `std.mod`, `std.and`, `std.select` between `std.pow` and `std.div`, and
/// `std.log` into `std.tanh`), with `std.const` values set mid-block, renders
/// the same audio and log per frame, at k = 1, 2, 3, 7, 64, 128, 256 and a
/// random partition, and on 1, 2, 3, 4 and 8 threads at k = 1, 64 and 256.
#[test]
fn a_graph_of_core_types_is_bit_identical_in_blocks_and_on_threads() {
    let project = project(
        "sr = { module = \"std.sample_rate\" }\n\
         n = { module = \"std.frames\" }\n\
         t = { module = \"std.mul\" }\n\
         hz = { module = \"std.const\", value = 220 }\n\
         ph = { module = \"std.mul\" }\n\
         sine = { module = \"std.sin_cycles\" }\n\
         th = { module = \"std.const\", value = 0.3 }\n\
         cmp = { module = \"std.compare\", mode = 4 }\n\
         rnd = { module = \"std.round\" }\n\
         two = { module = \"std.const\", value = 2 }\n\
         par = { module = \"std.mod\" }\n\
         both = { module = \"std.and\" }\n\
         mag = { module = \"std.abs\" }\n\
         ex = { module = \"std.const\", value = 1.5 }\n\
         pw = { module = \"std.pow\" }\n\
         three = { module = \"std.const\", value = 3 }\n\
         third = { module = \"std.div\" }\n\
         sel = { module = \"std.select\" }\n\
         wrap = { module = \"std.const\", value = 0.37 }\n\
         m = { module = \"std.mod\" }\n\
         ln = { module = \"std.log\" }\n\
         sh = { module = \"std.tanh\" }\n\
         out = { module = \"std.audio_out\" }\n",
        "  { from = \"n.lo\", to = \"t.a\" },\n\
         \x20 { from = \"sr.period\", to = \"t.b\" },\n\
         \x20 { from = \"t.out\", to = \"ph.a\" },\n\
         \x20 { from = \"hz.out\", to = \"ph.b\" },\n\
         \x20 { from = \"ph.out\", to = \"sine.in\" },\n\
         \x20 { from = \"sine.out\", to = \"cmp.a\" },\n\
         \x20 { from = \"th.out\", to = \"cmp.b\" },\n\
         \x20 { from = \"ph.out\", to = \"rnd.in\" },\n\
         \x20 { from = \"rnd.out\", to = \"par.a\" },\n\
         \x20 { from = \"two.out\", to = \"par.b\" },\n\
         \x20 { from = \"cmp.out\", to = \"both.a\" },\n\
         \x20 { from = \"par.out\", to = \"both.b\" },\n\
         \x20 { from = \"sine.out\", to = \"mag.in\" },\n\
         \x20 { from = \"mag.out\", to = \"pw.a\" },\n\
         \x20 { from = \"ex.out\", to = \"pw.b\" },\n\
         \x20 { from = \"sine.out\", to = \"third.a\" },\n\
         \x20 { from = \"three.out\", to = \"third.b\" },\n\
         \x20 { from = \"both.out\", to = \"sel.gate\" },\n\
         \x20 { from = \"pw.out\", to = \"sel.a\" },\n\
         \x20 { from = \"third.out\", to = \"sel.b\" },\n\
         \x20 { from = \"ph.out\", to = \"m.a\" },\n\
         \x20 { from = \"wrap.out\", to = \"m.b\" },\n\
         \x20 { from = \"m.out\", to = \"ln.in\" },\n\
         \x20 { from = \"ln.out\", to = \"sh.in\" },\n\
         \x20 { from = \"sel.out\", to = \"out.l\", scale = 0.5 },\n\
         \x20 { from = \"sh.out\", to = \"out.r\", scale = 0.5 },\n",
    );
    let plan = project.plan(&registry()).unwrap().plan;
    let events = [(37, "/hz.value", 330.0), (1_000, "/th.value", -0.2)];
    let run = |partition, threads| {
        let mut pushed = false;
        render(&plan, (8_000, 5, 4_000), partition, threads, |engine| {
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
    let reference = run(Partition::PerFrame, 1);
    // Neither channel is silent or constant.
    assert_eq!(reference.channels, 2);
    let left: Vec<f64> = reference.samples.iter().step_by(2).copied().collect();
    let right: Vec<f64> = reference
        .samples
        .iter()
        .skip(1)
        .step_by(2)
        .copied()
        .collect();
    assert!(left.iter().any(|&x| x > 0.0) && left.iter().any(|&x| x < 0.0));
    assert!(right.iter().any(|&x| x != right[0]));
    let reference = hashes(&reference);
    let runs = partitions().map(|p| (p, 1)).chain(thread_grid());
    for (partition, threads) in runs {
        assert_eq!(
            hashes(&run(partition, threads)),
            reference,
            "{partition:?}, {threads} threads"
        );
    }
    assert_no_violation();
}

/// M2.10-2 AC8: a graph of core set II renders the same audio and log per
/// frame, in every partition (k = 1, 2, 3, 7, 64, 128, 256 and a random
/// one), and on 1, 2, 3, 4 and 8 threads at k = 1, 64 and 256: phasors (one
/// reset by another's wrap), a counter, an integrator, a buffer written and
/// read by moving taps (Hermite), a wrapping table, two `std.random`s (one
/// triggered through a `std.clock_div`), `std.to_events` into latches
/// directly and through `std.route`, two to_events meeting in one latch, and
/// pack, vadd, vmul, reduce, index and unpack; with values set mid-block,
/// among them the buffer's `size` (64 to 8 at frame 1 001), which it reads
/// on every frame.
#[test]
fn a_graph_of_core_set_two_is_bit_identical_in_blocks_and_on_threads() {
    let project = project(
        "clk = { module = \"std.phasor\", freq = 50 }\n\
         fast = { module = \"std.phasor\", freq = 330, reset_phase = 0.25 }\n\
         slow = { module = \"std.phasor\", freq = 7 }\n\
         cnt = { module = \"std.counter\", step = 2, modulo = 5 }\n\
         integ = { module = \"std.integrator\", cutoff = 200 }\n\
         rnd = { module = \"std.random\", lo = -1, hi = 1 }\n\
         cd = { module = \"std.clock_div\", divide = 3 }\n\
         rnd2 = { module = \"std.random\" }\n\
         n = { module = \"std.frames\" }\n\
         on = { module = \"std.const\", value = 1 }\n\
         buf = { module = \"std.buffer1k\", size = 64, interp = 2 }\n\
         tab = { module = \"std.table4\", v1 = 1, v2 = -1, v3 = 0.5, wrap = 1, interp = 2 }\n\
         te = { module = \"std.to_events\", threshold = 0.01 }\n\
         te2 = { module = \"std.to_events\", on_change = 0 }\n\
         lat = { module = \"std.latch\" }\n\
         rt = { module = \"std.route\" }\n\
         la = { module = \"std.latch\" }\n\
         lb = { module = \"std.latch\" }\n\
         pk = { module = \"std.pack4\" }\n\
         vs = { module = \"std.vadd4\" }\n\
         vm = { module = \"std.vmul4\" }\n\
         red = { module = \"std.reduce4\" }\n\
         ix = { module = \"std.index4\" }\n\
         up = { module = \"std.unpack4\" }\n\
         out = { module = \"std.audio_out\" }\n",
        "  { from = \"clk.wrap\", to = \"fast.reset\" },\n\
         \x20 { from = \"clk.wrap\", to = \"cnt.trigger\" },\n\
         \x20 { from = \"clk.wrap\", to = \"integ.reset\" },\n\
         \x20 { from = \"clk.wrap\", to = \"rnd.trigger\" },\n\
         \x20 { from = \"fast.wrap\", to = \"cd.in\" },\n\
         \x20 { from = \"cd.out\", to = \"rnd2.trigger\" },\n\
         \x20 { from = \"fast.phase\", to = \"integ.in\", scale = 2 },\n\
         \x20 { from = \"n.lo\", to = \"buf.index\" },\n\
         \x20 { from = \"fast.phase\", to = \"buf.value\" },\n\
         \x20 { from = \"on.out\", to = \"buf.gate\" },\n\
         \x20 { from = \"slow.phase\", to = \"buf.read0\", scale = 64 },\n\
         \x20 { from = \"n.lo\", to = \"buf.read1\" },\n\
         \x20 { from = \"fast.phase\", to = \"tab.pos\", scale = 4 },\n\
         \x20 { from = \"integ.out\", to = \"te.in\" },\n\
         \x20 { from = \"clk.wrap\", to = \"te.trigger\" },\n\
         \x20 { from = \"fast.phase\", to = \"te2.in\" },\n\
         \x20 { from = \"clk.wrap\", to = \"te2.trigger\" },\n\
         \x20 { from = \"te.out\", to = \"lat.in\" },\n\
         \x20 { from = \"te2.out\", to = \"lat.in\" },\n\
         \x20 { from = \"te.out\", to = \"rt.in\" },\n\
         \x20 { from = \"rnd.out\", to = \"rt.gate\" },\n\
         \x20 { from = \"rt.a\", to = \"la.in\" },\n\
         \x20 { from = \"rt.b\", to = \"lb.in\" },\n\
         \x20 { from = \"fast.phase\", to = \"pk.e0\" },\n\
         \x20 { from = \"integ.out\", to = \"pk.e1\" },\n\
         \x20 { from = \"rnd.out\", to = \"pk.e2\" },\n\
         \x20 { from = \"cnt.count\", to = \"pk.e3\" },\n\
         \x20 { from = \"pk.out\", to = \"vs.a\" },\n\
         \x20 { from = \"pk.out\", to = \"vs.b\" },\n\
         \x20 { from = \"pk.out\", to = \"vm.a\" },\n\
         \x20 { from = \"vs.out\", to = \"vm.b\" },\n\
         \x20 { from = \"vm.out\", to = \"red.in\" },\n\
         \x20 { from = \"vs.out\", to = \"ix.in\" },\n\
         \x20 { from = \"fast.phase\", to = \"ix.pos\", scale = 3 },\n\
         \x20 { from = \"vm.out\", to = \"up.in\" },\n\
         \x20 { from = \"buf.out0\", to = \"out.l\", scale = 0.1 },\n\
         \x20 { from = \"buf.out1\", to = \"out.l\", scale = 0.1 },\n\
         \x20 { from = \"tab.out\", to = \"out.l\", scale = 0.1 },\n\
         \x20 { from = \"lat.out\", to = \"out.l\", scale = 0.1 },\n\
         \x20 { from = \"la.out\", to = \"out.l\", scale = 0.05 },\n\
         \x20 { from = \"red.sum\", to = \"out.l\", scale = 0.01 },\n\
         \x20 { from = \"ix.out\", to = \"out.l\", scale = 0.05 },\n\
         \x20 { from = \"up.e3\", to = \"out.l\", scale = 0.01 },\n\
         \x20 { from = \"integ.out\", to = \"out.r\", scale = 0.1 },\n\
         \x20 { from = \"rnd.out\", to = \"out.r\", scale = 0.1 },\n\
         \x20 { from = \"rnd2.out\", to = \"out.r\", scale = 0.1 },\n\
         \x20 { from = \"lb.out\", to = \"out.r\", scale = 0.05 },\n\
         \x20 { from = \"red.mean\", to = \"out.r\", scale = 0.05 },\n\
         \x20 { from = \"red.min\", to = \"out.r\", scale = 0.05 },\n\
         \x20 { from = \"red.max\", to = \"out.r\", scale = 0.05 },\n\
         \x20 { from = \"cnt.count\", to = \"out.r\", scale = 0.01 },\n",
    );
    let plan = project.plan(&registry()).unwrap().plan;
    let events = [
        (37, "/rnd.hi", 2.0),
        (1_000, "/cnt.step", -1.0),
        (1_001, "/buf.size", 8.0),
        (2_500, "/fast.freq", 500.0),
    ];
    let run = |partition, threads| {
        let mut pushed = false;
        render(&plan, (8_000, 5, 4_000), partition, threads, |engine| {
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
    let reference = run(Partition::PerFrame, 1);
    assert_eq!(reference.channels, 2);
    for channel in 0..2 {
        let samples: Vec<f64> = reference
            .samples
            .iter()
            .skip(channel)
            .step_by(2)
            .copied()
            .collect();
        assert!(
            samples.iter().any(|&x| x != samples[0]),
            "channel {channel}"
        );
    }
    let reference = hashes(&reference);
    let runs = partitions().map(|p| (p, 1)).chain(thread_grid());
    for (partition, threads) in runs {
        assert_eq!(
            hashes(&run(partition, threads)),
            reference,
            "{partition:?}, {threads} threads"
        );
    }
    assert_no_violation();
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

/// [`render`] with the vector outputs `taps` snapshotted
/// ([`render_blocks_tapped`]); the per-frame partition runs blocks of one
/// frame.
fn render_tapped(
    plan: &ExecutionPlan,
    (sample_rate, seed, frames): (u32, u64, u64),
    partition: Partition,
    threads: usize,
    mut before: impl FnMut(&mut Engine) -> Result<Option<Frame>, EngineError>,
    taps: &[VectorTap],
) -> (Render, Vec<VectorTrack>) {
    let registry = registry();
    let config = config(sample_rate, seed, threads);
    let mut engine = Engine::new(plan, &registry, config, Vec::new()).unwrap();
    let _threaded = (threads > 1).then(|| {
        engine.set_task_hook(Some(workers_take_part));
        THREADED.lock().unwrap_or_else(|e| e.into_inner())
    });
    // `before` swaps nothing here: the one plan is every version's.
    let plans = std::slice::from_ref(plan);
    let rendered = match partition {
        Partition::PerFrame => render_blocks_tapped(&mut engine, frames, 1, before, taps, plans),
        Partition::Blocks(k) => render_blocks_tapped(&mut engine, frames, k, before, taps, plans),
        Partition::Random(seed) => {
            let mut rng = SplitMix64::new(seed);
            let cut = |engine: &mut Engine| {
                let next = before(engine)?;
                let cut = Frame(engine.next_frame().0 + 1 + rng.next_u64() % 256);
                Ok(Some(next.map_or(cut, |at: Frame| at.min(cut))))
            };
            render_blocks_tapped(&mut engine, frames, 256, cut, taps, plans)
        }
    }
    .unwrap();
    assert_eq!(engine.log_overflows(), 0);
    assert_eq!(engine.input_overflows(), 0);
    if threads > 1 {
        let stats = engine.schedule_stats();
        assert!(stats.worker_tasks > 0, "{stats:?}");
    }
    rendered
}

fn tap(port: &str, rows: usize, decimation: u64) -> VectorTap {
    VectorTap {
        port: port.parse().unwrap(),
        rows,
        decimation,
    }
}

/// M2.8-7, operator-request:007 condition 3: an event into `std.count` acts
/// on a divisor-800 type at the first tick at or after its frame, once,
/// whatever the partition or thread count. Particles that live for a minute
/// count the emissions: +1 at 0; +3 at 800 (frames 1, 799, 800); +3 at
/// 1600 (801, 1599, 1600); none at 2400; +6 at 3200 (2405 and five at 3000).
#[test]
fn events_act_at_the_first_tick_at_or_after_their_frame() {
    let project = project_at(
        48_000,
        "c = { module = \"std.count\" }\n\
         p = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\n\
         out = { module = \"std.audio_out\" }\n",
        "  { from = \"c.window\", to = \"p.emit\" },\n",
    );
    let plan = project.plan(&registry()).unwrap().plan;
    let mut frames: Vec<u64> = vec![0, 1, 799, 800, 801, 1599, 1600, 2405];
    frames.extend([3000; 5]);
    let run = |partition, threads| {
        let mut next = 0;
        let before = |engine: &mut Engine| {
            engine.reclaim_externals().for_each(drop);
            let now = engine.next_frame().0;
            while let Some(&at) = frames.get(next).filter(|&&at| at == now) {
                next += 1;
                let event = ExternalEvent {
                    frame: Frame(at),
                    sequence: next as u64,
                    target: "/c.in".parse().unwrap(),
                    payload: EventPayload::Trigger,
                };
                engine.push_external(event).unwrap();
            }
            Ok(frames.get(next).map(|&at| Frame(at)))
        };
        // At the port's ticks: a tap between them is refused (M2.8-9).
        let taps = [tap("/p.life", 64, 800)];
        let (_, tracks) =
            render_tapped(&plan, (48_000, 5, 4_000), partition, threads, before, &taps);
        tracks[0]
            .snapshots
            .iter()
            .map(|(frame, life)| (frame.0, life.iter().filter(|&&l| l > 0.0).count()))
            .collect::<Vec<_>>()
    };
    let live = run(Partition::Blocks(1), 1);
    // Tick by tick: the count changes at the ticks the events reach.
    let changes: Vec<(u64, usize)> = live
        .windows(2)
        .filter(|w| w[0].1 != w[1].1)
        .map(|w| (w[1].0, w[1].1))
        .collect();
    assert_eq!(live[0], (0, 1));
    assert_eq!(changes, vec![(800, 4), (1600, 7), (3200, 13)]);
    for (partition, threads) in [
        (Partition::Blocks(3), 1),
        (Partition::Blocks(64), 1),
        (Partition::Blocks(256), 1),
        (Partition::Random(11), 1),
        (Partition::Blocks(1), 4),
        (Partition::Blocks(64), 4),
    ] {
        assert_eq!(
            run(partition, threads),
            live,
            "{partition:?}, {threads} threads"
        );
    }
}

/// M2.8-7 AC1: the physical-av demo, its visual-rate types and the count
/// bridge included, is bit-identical in every partition and on workers:
/// the audio, the log and every row of every vector output at every tick
/// (the bridge readings every frame, the audio membrane's 256 rows every 97
/// frames). The visual streams move, so the comparison is not of zeros.
#[test]
fn physical_av_is_bit_identical_in_blocks_and_on_threads() {
    let project = Project::load(&repository().join("projects/physical-av")).unwrap();
    let plan = project.plan(&registry()).unwrap().plan;
    let taps = [
        tap("/membrane_v.z", 256, 800),
        tap("/sparks.pos", 128, 800),
        tap("/sparks.vel", 128, 800),
        tap("/sparks.life", 64, 800),
        tap("/bodies.bodies", 24, 800),
        tap("/bodies.vel", 12, 800),
        tap("/hits.window", signal_std::BRIDGE_ROWS, 1),
        tap("/kicks.window", signal_std::BRIDGE_ROWS, 1),
        tap("/membrane_a.z", 256, 97),
    ];
    let run = (project.sample_rate, project.seed, 96_000);
    let (reference, tracks) =
        render_tapped(&plan, run, Partition::Blocks(1), 1, |_| Ok(None), &taps);
    for (track, tap) in tracks.iter().zip(&taps) {
        let first = &track.snapshots[0].1;
        let moving = track.snapshots.iter().any(|(_, rows)| rows != first);
        assert!(moving, "{} never changes", tap.port);
        assert_eq!(
            track.snapshots.len() as u64,
            96_000_u64.div_ceil(tap.decimation)
        );
    }
    let bits = |t: &VectorTrack| -> Vec<(u64, Vec<u64>)> {
        t.snapshots
            .iter()
            .map(|(f, rows)| (f.0, rows.iter().map(|x| x.to_bits()).collect()))
            .collect()
    };
    let runs = partitions().map(|p| (p, 1)).chain(
        [2, 4, 8]
            .into_iter()
            .flat_map(|t| [64, 256].map(|k| (Partition::Blocks(k), t))),
    );
    for (partition, threads) in runs {
        let (rendered, other) = render_tapped(&plan, run, partition, threads, |_| Ok(None), &taps);
        assert_eq!(
            hashes(&rendered),
            hashes(&reference),
            "{partition:?}, {threads} threads"
        );
        for ((a, b), tap) in tracks.iter().zip(&other).zip(&taps) {
            assert!(
                bits(a) == bits(b),
                "{}: {partition:?}, {threads} threads",
                tap.port
            );
        }
    }
    assert_no_violation();
}

/// A one-file project at `rate`.
fn project_at(rate: u32, instances: &str, connections: &str) -> Project {
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"bridge\"\nsample_rate = {rate}\nseed = 5\n\n\
         [instances]\n{instances}\n[wiring]\nconnections = [\n{connections}]\n"
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap()
}
