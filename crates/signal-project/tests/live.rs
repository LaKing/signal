//! `signal_project::live::send_edit` (M2-4) on a real-time run with the null
//! driver: a removal (suspends at the default frame, the swap at the removal
//! frame) is recorded as applied and replays to what was played; an edit
//! larger than the command queue can take, or whose swap cannot be prepared,
//! is refused before anything is sent, so no instance is left suspended.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use signal_core::{ModuleType, PortDescriptor, Primitive, RateDivisor, Registry};
use signal_engine::{Controller, Driver, NullDriver, RealtimeConfig, Runner, realtime};
use signal_project::live::{Applied, EditError, ReloadError, reload, send_edit};
use signal_project::session::{self, LiveRecorder, Session, SessionRecorder, replay};
use signal_project::{Project, SourceFile, provenance};

const RATE: u32 = 48_000;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// The standard registry plus `test.slow_lfo`: `std.lfo` with every port at
/// rate divisor 2, which plans but which the engine refuses to run (M1 runs
/// divisor 1 only), so a swap adding it fails to prepare.
fn registry_with_slow_lfo() -> Registry {
    let mut registry = registry();
    let lfo = *registry.get("std.lfo").unwrap();
    let mut descriptor = lfo.descriptor;
    let ports: Vec<PortDescriptor> = descriptor
        .ports
        .iter()
        .map(|p| PortDescriptor {
            rate: RateDivisor::new(2).unwrap(),
            ..*p
        })
        .collect();
    descriptor.ports = Box::leak(ports.into_boxed_slice());
    descriptor.type_name = "test.slow_lfo";
    registry
        .register(ModuleType {
            descriptor,
            create: lfo.create,
        })
        .unwrap();
    registry
}

/// A tone into the stereo sink, and `extra` unconnected oscillators
/// `x0`, `x1`, … minus those in `without`.
fn project(extra: usize, without: &[usize]) -> Project {
    project_with(extra, without, "")
}

/// [`project`] with `more` (instance lines) added.
fn project_with(extra: usize, without: &[usize], more: &str) -> Project {
    let mut text = format!(
        "schema_version = 1\n\n[project]\nname = \"live\"\nsample_rate = {RATE}\nseed = 5\n\n[instances]\n\
         tone = {{ module = \"std.sine_osc\", freq = 330 }}\n\
         out = {{ module = \"std.audio_out\", gain = 0.5 }}\n"
    );
    for i in (0..extra).filter(|i| !without.contains(i)) {
        text.push_str(&format!(
            "x{i} = {{ module = \"std.sine_osc\", freq = {} }}\n",
            100 + i
        ));
    }
    text.push_str(more);
    text.push_str(
        "\n[wiring]\nconnections = [\n  { from = \"tone.out\", to = \"out.l\" },\n  { from = \"tone.out\", to = \"out.r\" },\n]\n",
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap()
}

/// A null-driver run of `old`: the driver, its Controller and the recorder.
fn start(old: &Project, registry: &Registry) -> (NullDriver, Controller, LiveRecorder) {
    let mut plan = old.plan(registry).unwrap().plan;
    plan.version = 0;
    let (runner, controller) =
        realtime(&plan, registry, RealtimeConfig::new(RATE, old.seed)).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &old.files);
    let recorder = SessionRecorder::new(old, registry, RATE, old.seed, provenance).unwrap();
    let mut driver = NullDriver::new(RATE, 2, 3);
    driver.start(runner).unwrap();
    (
        driver,
        controller,
        LiveRecorder::new(recorder, registry.clone()),
    )
}

/// Runs `frames` frames in callbacks of up to 256, polling between them.
fn run(driver: &mut NullDriver, controller: &mut Controller, live: &mut LiveRecorder, frames: u64) {
    for _ in 0..frames / 256 {
        driver.run(256, || {}).unwrap();
        controller.poll(live);
    }
}

fn kinds(edits: &[session::Edit]) -> Vec<&'static str> {
    edits
        .iter()
        .map(|e| match e.command {
            session::Command::Swap { .. } => "swap",
            session::Command::Edit { .. } => "suspend",
            session::Command::Event { .. } => "event",
        })
        .collect()
}

/// The removal protocol through the Controller: three instances suspended at
/// the default frame, the swap at the removal frame (5 ms later), recorded as
/// applied; the session replays to the null driver's output.
#[test]
fn a_removal_suspends_then_swaps_and_replays() {
    let registry = registry();
    let old = project(5, &[]);
    let (mut driver, mut controller, mut live) = start(&old, &registry);
    run(&mut driver, &mut controller, &mut live, 1_024);

    let new = project(5, &[1, 2, 4]);
    let (_, applied) = send_edit(&mut controller, &mut live, &old, new, &registry).unwrap();
    assert_eq!(
        applied,
        Applied {
            sets: 0,
            swap: Some(1),
            removed: 3
        }
    );
    run(&mut driver, &mut controller, &mut live, 8_192);
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();
    assert_eq!(live.refused(), &[] as &[String]);
    assert_eq!(controller.in_flight(), 0);

    let recorder = live.into_recorder();
    let edits = recorder.edits();
    assert_eq!(kinds(&edits), ["suspend", "suspend", "suspend", "swap"]);
    let suspend = edits[0].frame.0;
    assert!(edits[..3].iter().all(|e| e.frame.0 == suspend));
    // F = 5 ms at 48 kHz.
    assert_eq!(edits[3].frame.0, suspend + 240);

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("live/removal");
    let _ = std::fs::remove_dir_all(&dir);
    recorder.write(&dir).unwrap();
    let session = Session::load(&dir, &registry).unwrap();
    let replayed = replay(&session, &registry, None).unwrap();
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(samples, driver.output(), "replay equals what was played");
}

/// auditor-1's probe: removing more instances at once than the command queue
/// can take. The edit is refused before anything is sent: no suspend reaches
/// the runner, nothing is in flight, and the old version keeps playing with
/// every instance running.
#[test]
fn an_edit_larger_than_the_command_queue_sends_nothing() {
    let registry = registry();
    let extra = 1_100;
    let old = project(extra, &[]);
    let (mut driver, mut controller, mut live) = start(&old, &registry);
    run(&mut driver, &mut controller, &mut live, 512);
    assert!(
        controller.available() < extra,
        "the probe removes more than the queue takes ({})",
        controller.available()
    );

    let all: Vec<usize> = (0..extra).collect();
    let new = project(extra, &all);
    let Err(EditError::Refused(message)) =
        send_edit(&mut controller, &mut live, &old, new, &registry)
    else {
        panic!("expected a refusal");
    };
    assert!(message.contains("nothing was sent"), "{message}");
    assert_eq!(controller.in_flight(), 0);
    assert_eq!(controller.command_overflows(), 0, "nothing was pushed");

    run(&mut driver, &mut controller, &mut live, 4_096);
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();
    assert_eq!(live.refused(), &[] as &[String]);
    assert_eq!(
        live.recorder().edits(),
        Vec::new(),
        "no suspend, no swap: every instance still runs in the old version"
    );
    assert_eq!(runner.status().version, 0);
}

/// auditor-1's `engine-error` probe: removing 500 instances while adding one
/// the engine cannot run, so the swap fails to prepare. (500 removals need
/// 1 001 commands with their resumes, within the 1 024 the queue takes, so
/// the room check passes and the preparation is what refuses.) The swap is prepared
/// before any suspend is sent: the error is reported, nothing was sent, no
/// instance is suspended, and the old version keeps playing.
#[test]
fn an_edit_whose_swap_cannot_be_prepared_sends_nothing() {
    let registry = registry_with_slow_lfo();
    let extra = 500;
    let old = project(extra, &[]);
    let (mut driver, mut controller, mut live) = start(&old, &registry);
    run(&mut driver, &mut controller, &mut live, 512);
    assert!(2 * extra < controller.available(), "the room check passes");

    let all: Vec<usize> = (0..extra).collect();
    let new = project_with(extra, &all, "bad = { module = \"test.slow_lfo\" }\n");
    let Err(EditError::Refused(message)) =
        send_edit(&mut controller, &mut live, &old, new, &registry)
    else {
        panic!("expected a refusal");
    };
    assert!(message.contains("rate divisor"), "{message}");
    assert!(message.contains("nothing was sent"), "{message}");
    assert_eq!(controller.in_flight(), 0);
    assert_eq!(controller.command_overflows(), 0);

    run(&mut driver, &mut controller, &mut live, 4_096);
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();
    assert_eq!(live.refused(), &[] as &[String]);
    assert_eq!(
        live.recorder().edits(),
        Vec::new(),
        "no suspend was sent, so none is left without its resume"
    );
    assert_eq!(runner.status().version, 0);
}

/// `std.lfo`'s constructor, for [`slow_lfo`].
static LFO: OnceLock<fn() -> Primitive> = OnceLock::new();

/// `std.lfo`, built slowly: preparing a swap that adds it takes 300 ms on
/// the control thread while the runner plays on.
fn slow_lfo() -> Primitive {
    thread::sleep(Duration::from_millis(300));
    (LFO.get().expect("registered"))()
}

/// The standard registry plus `test.sleepy_lfo` ([`slow_lfo`]).
fn registry_with_sleepy_lfo() -> Registry {
    let mut registry = registry();
    let lfo = *registry.get("std.lfo").unwrap();
    let _ = LFO.set(lfo.create);
    let mut descriptor = lfo.descriptor;
    descriptor.type_name = "test.sleepy_lfo";
    registry
        .register(ModuleType {
            descriptor,
            create: slow_lfo,
        })
        .unwrap();
    registry
}

/// auditor-1's late-removal probe: the runner plays on its own thread in
/// 128-frame callbacks at real-time pace while an edit removes the audible
/// `/tone` and adds an instance whose preparation takes 300 ms (more than
/// 14 000 frames). The frames of the suspend and the swap come from a status
/// polled after the preparation, so the removal keeps its fade: the swap is
/// exactly F frames after the suspend, nothing is refused or discarded, and
/// the session replays to what was played.
#[test]
fn a_slow_preparation_keeps_the_removal_fade_while_playing() {
    let registry = registry_with_sleepy_lfo();
    let old = project_with(1, &[], "");
    let mut plan = old.plan(&registry).unwrap().plan;
    plan.version = 0;
    // A 512-frame buffer: the default stamp is 1 024 frames ahead.
    let config = RealtimeConfig::with_buffer(RATE, old.seed, 512);
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &old.files);
    let recorder = SessionRecorder::new(&old, &registry, RATE, old.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());

    let stop = Arc::new(AtomicBool::new(false));
    let playing = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let mut runner: Runner = runner;
            let mut out = vec![0.0f32; 128 * 2];
            let mut played = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                runner.process(&mut out, 2);
                played.extend_from_slice(&out);
                thread::sleep(Duration::from_secs_f64(128.0 / f64::from(RATE)));
            }
            (runner, played)
        })
    };
    let wait = |controller: &mut Controller, live: &mut LiveRecorder, ms: u64| {
        for _ in 0..ms {
            controller.poll(live);
            thread::sleep(Duration::from_millis(1));
        }
    };
    wait(&mut controller, &mut live, 50);

    // /tone removed (out.l and out.r keep x0), the sleepy instance added.
    let text = old.files[0]
        .text
        .replace("tone = { module = \"std.sine_osc\", freq = 330 }\n", "")
        .replace("tone.out", "x0.out")
        .replace(
            "[wiring]",
            "sleepy = { module = \"test.sleepy_lfo\" }\n\n[wiring]",
        );
    let new = Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap();
    let (_, applied) = send_edit(&mut controller, &mut live, &old, new, &registry).unwrap();
    assert_eq!(applied.removed, 1);
    assert_eq!(applied.swap, Some(1));

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while controller.in_flight() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the edit never applied"
        );
        wait(&mut controller, &mut live, 1);
    }
    wait(&mut controller, &mut live, 100);
    stop.store(true, Ordering::Relaxed);
    let (runner, played) = playing.join().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();

    assert_eq!(live.refused(), &[] as &[String], "no command was refused");
    let status = runner.status();
    assert_eq!(
        status.input_discards,
        Default::default(),
        "nothing discarded"
    );
    assert_eq!(status.version, 1);
    let recorder = live.into_recorder();
    let edits = recorder.edits();
    assert_eq!(kinds(&edits), ["suspend", "swap"]);
    // F = 5 ms at 48 kHz: the removal kept its fade even if the commands
    // were late.
    assert_eq!(
        edits[1].frame.0,
        edits[0].frame.0 + 240,
        "the swap follows the suspend by the fade (late: {})",
        status.late
    );

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("live/slow-removal");
    let _ = std::fs::remove_dir_all(&dir);
    recorder.write(&dir).unwrap();
    let session = Session::load(&dir, &registry).unwrap();
    let replayed = replay(&session, &registry, None).unwrap();
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(samples.len(), played.len());
    assert_eq!(samples, played, "replay equals what was played");
}

/// `reload` (the `--watch` and Signal.app path): a folder that does not load
/// keeps the running project, with diagnostics located in the folder; a
/// parameter change is sent and becomes the running project.
#[test]
fn reload_keeps_the_last_good_version_and_applies_a_good_one() {
    let registry = registry();
    let dir = std::env::temp_dir().join(format!("signal-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let text = project(0, &[]).files[0].text.clone();
    std::fs::write(dir.join("project.toml"), &text).unwrap();
    let mut current = Project::load(&dir).unwrap();
    let (mut driver, mut controller, mut live) = start(&current, &registry);
    run(&mut driver, &mut controller, &mut live, 1_024);

    std::fs::write(dir.join("project.toml"), "schema_version = [").unwrap();
    let error = reload(&mut controller, &mut live, &mut current, &dir, &registry).unwrap_err();
    let ReloadError::Invalid(diagnostics) = error else {
        panic!("expected diagnostics, got {error:?}");
    };
    assert!(diagnostics[0].file.starts_with(dir.to_str().unwrap()));
    assert!(current.files[0].text.contains("freq = 330"), "kept");

    std::fs::write(
        dir.join("project.toml"),
        text.replace("freq = 330", "freq = 440"),
    )
    .unwrap();
    let applied = reload(&mut controller, &mut live, &mut current, &dir, &registry).unwrap();
    assert_eq!((applied.sets, applied.swap), (1, None));
    assert_eq!(applied.to_string(), "applied 1 sets");
    assert!(current.files[0].text.contains("freq = 440"), "replaced");
    std::fs::remove_dir_all(&dir).unwrap();
}
