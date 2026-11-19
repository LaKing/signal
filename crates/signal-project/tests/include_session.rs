//! ADR 0035 §13 (phase 4): a session of a host with includes. Each plan
//! version's copy holds the include files under `includes/<key>/`
//! (`Project::session_copy`), nested the same way for an include's own
//! include, and replay plans from them: a live join of an include from
//! outside the host's folder, then an edit of a file the include includes
//! in turn, on the null driver's real-time run, replays to what was played
//! bit for bit, with the included folders gone.

use std::fs;
use std::path::{Path, PathBuf};

use signal_core::Registry;
use signal_engine::RealtimeConfig;
use signal_output::{Controller, Driver, NullDriver, realtime};
use signal_project::live::send_edit;
use signal_project::session::{self, LiveRecorder, Session, SessionRecorder, replay};
use signal_project::{Project, provenance};

const RATE: u32 = 48_000;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn host(joined: bool) -> String {
    let (entry, wires) = if joined {
        (
            "inc = { project = \"../inc\", set = { \"amp.db\" = -9 } }\n",
            "  { from = \"inc.l\", to = \"out.l\" },\n",
        )
    } else {
        ("", "")
    };
    format!(
        "schema_version = 1\n\n[project]\nname = \"host\"\nsample_rate = {RATE}\nseed = 1\n\n[instances]\n\
         tone = {{ module = \"std.sine_osc\", freq = 330 }}\n{entry}out = {{ module = \"std.audio_out\", gain = 0.5 }}\n\n\
         [wiring]\nconnections = [\n  {{ from = \"tone.out\", to = \"out.r\" }},\n{wires}]\n"
    )
}

const INC: &str = "schema_version = 1\n\n[project]\nname = \"inc\"\nsample_rate = 48000\nseed = 2\n\n[instances]\n\
leaf = { project = \"../leaf\" }\namp = { module = \"std.gain\", db = -6 }\nout = { module = \"std.audio_out\" }\n\n\
[wiring]\nconnections = [\n  { from = \"leaf.l\", to = \"amp.in\" },\n  { from = \"amp.out\", to = \"out.l\" },\n  { from = \"amp.out\", to = \"out.r\" },\n]\n";

fn leaf(freq: u32) -> String {
    format!(
        "schema_version = 1\n\n[project]\nname = \"leaf\"\nsample_rate = 48000\nseed = 3\n\n[instances]\n\
         noise = {{ module = \"std.noise\" }}\nosc = {{ module = \"std.sine_osc\", freq = {freq} }}\nout = {{ module = \"std.audio_out\" }}\n\n\
         [wiring]\nconnections = [\n  {{ from = \"osc.out\", to = \"out.l\" }},\n  {{ from = \"noise.out\", to = \"out.r\" }},\n]\n"
    )
}

/// Plays at least `frames` more frames, polling after every callback.
fn play(
    driver: &mut NullDriver,
    controller: &mut Controller,
    live: &mut LiveRecorder,
    frames: u64,
) {
    driver.run(frames, || controller.poll(live)).unwrap();
    controller.poll(live);
}

fn write(dir: &Path, text: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("project.toml"), text).unwrap();
}

#[test]
fn a_live_join_of_an_include_replays_from_the_session_s_copies() {
    let registry = registry();
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("include-session");
    let _ = fs::remove_dir_all(&base);
    let (host_dir, inc_dir, leaf_dir) = (base.join("host"), base.join("inc"), base.join("leaf"));
    write(&host_dir, &host(false));
    write(&inc_dir, INC);
    write(&leaf_dir, &leaf(220));

    let old = Project::load(&host_dir).unwrap();
    let mut plan = old.plan(&registry).unwrap().plan;
    plan.version = 0;
    let config = RealtimeConfig::with_buffer(RATE, old.seed, 4096);
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let provenance = provenance::build(&old.files);
    let recorder = SessionRecorder::new(&old, &registry, RATE, old.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let mut driver = NullDriver::new(RATE, 2, 21);
    driver.start(runner).unwrap();
    play(&mut driver, &mut controller, &mut live, 3_000);

    // The join.
    write(&host_dir, &host(true));
    let joined = Project::load(&host_dir).unwrap();
    let (_, applied) =
        send_edit(&mut controller, &mut live, &old, joined.clone(), &registry).unwrap();
    assert!(applied.swap.is_some(), "{applied}");
    play(&mut driver, &mut controller, &mut live, 12_000);

    // A file the include includes, edited: the host reloads with it.
    write(&leaf_dir, &leaf(275));
    let edited = Project::load(&host_dir).unwrap();
    let (_, applied) = send_edit(&mut controller, &mut live, &joined, edited, &registry).unwrap();
    assert!(applied.sets > 0 || applied.swap.is_some(), "{applied}");
    while controller.in_flight() > 0 {
        play(&mut driver, &mut controller, &mut live, 1024);
    }
    play(&mut driver, &mut controller, &mut live, 6_000);
    let played = driver.take_output();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();
    assert_eq!(live.refused(), &[] as &[String]);
    let recorder = live.into_recorder();
    assert!(
        recorder
            .edits()
            .iter()
            .any(|e| matches!(e.command, session::Command::Swap { .. }))
    );

    let dir = base.join("session");
    recorder.write(&dir).unwrap();
    let v1 = dir.join("projects/v1");
    let text = fs::read_to_string(v1.join("project.toml")).unwrap();
    assert!(
        text.contains(r#"inc = { project = "includes/inc", set = { "amp.db" = -9 } }"#),
        "only the path is rewritten: {text}"
    );
    assert_eq!(
        fs::read_to_string(v1.join("includes/inc/project.toml")).unwrap(),
        INC.replace("\"../leaf\"", "\"includes/leaf\""),
    );
    assert_eq!(
        fs::read_to_string(v1.join("includes/inc/includes/leaf/project.toml")).unwrap(),
        leaf(220)
    );
    let toml = fs::read_to_string(dir.join("session.toml")).unwrap();
    assert!(
        toml.contains("\"includes/inc/includes/leaf/project.toml\""),
        "{toml}"
    );

    // Replay needs nothing outside the session.
    fs::remove_dir_all(&host_dir).unwrap();
    fs::remove_dir_all(&inc_dir).unwrap();
    fs::remove_dir_all(&leaf_dir).unwrap();
    let session = Session::load(&dir, &registry).unwrap();
    let replayed = replay(&session, &registry, None, 1).unwrap();
    let replayed: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(replayed.len(), played.len());
    assert!(
        replayed == played,
        "the replay is what was played, bit for bit"
    );
    assert!(
        played.chunks(2).any(|c| c[0] != 0.0),
        "the include was heard"
    );

    // A changed copy is refused, naming the include's file.
    let leaf_copy = v1.join("includes/inc/includes/leaf/project.toml");
    fs::write(&leaf_copy, leaf(221)).unwrap();
    let error = Session::load(&dir, &registry).err().unwrap().to_string();
    assert!(
        error.contains("projects/v1/includes/inc/includes/leaf/project.toml"),
        "{error}"
    );
}
