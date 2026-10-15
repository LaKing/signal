//! Sessions and `signal replay` (M2-2): a scripted live session recorded with
//! `SessionRecorder` replays bit-identically (V10, offline form), replay does
//! not depend on how or when the session was made, and malformed sessions
//! fail with located errors and exit code 1.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use signal_core::{Curve, EventPayload, Frame, InstanceAddress, PortAddress, Registry};
use signal_engine::{Engine, EngineConfig, EngineError, Provenance};
use signal_output::{Render, render_with};
use signal_project::session::{self, EditKind, Session, SessionRecorder, replay};
use signal_project::{Project, ProjectEdit, diff};

/// The frames of the scripted session.
const FRAMES: u64 = 3_000;
/// The frame of the swap that adds `/osc2`.
const ADD: u64 = 1_000;
/// The frame `/osc2` is suspended at, to remove it.
const REMOVE: u64 = 1_500;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn version(n: u32) -> Project {
    Project::load(&fixture(&format!("session/v{n}"))).unwrap()
}

/// A fresh folder for one test, inside the target directory.
fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("session")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .expect("the signal binary runs")
}

fn provenance() -> Provenance {
    Provenance {
        signal_revision: "session-test".to_owned(),
        signal_dirty: Some(false),
        rustc_version: "rustc (test)".to_owned(),
        project_file_hashes: Default::default(),
        cargo_lock_hash: "0000000000000000".to_owned(),
    }
}

fn port(text: &str) -> PortAddress {
    text.parse().unwrap()
}

fn addr(text: &str) -> InstanceAddress {
    text.parse().unwrap()
}

/// Records and pushes an event.
fn send(
    recorder: &mut SessionRecorder,
    engine: &mut Engine,
    stamp: u64,
    target: &str,
    payload: EventPayload,
) {
    let event = recorder
        .event(Frame(stamp), engine.next_frame(), port(target), payload)
        .unwrap();
    engine.push_external(event.clone()).unwrap();
    recorder.applied_event(&event).unwrap();
}

/// Records and pushes a suspend or resume.
fn edit(recorder: &mut SessionRecorder, engine: &mut Engine, kind: EditKind, address: &str) {
    let pending = recorder
        .edit(
            kind,
            engine.next_frame(),
            engine.next_frame(),
            addr(address),
        )
        .unwrap();
    match kind {
        EditKind::Suspend => engine.push_suspend(pending.address.clone(), pending.frame),
        EditKind::Resume => engine.push_resume(pending.address.clone(), pending.frame),
    }
    .unwrap();
    recorder.applied_edit(pending).unwrap();
}

/// Swaps from `old` to `new` at the next frame, as a file-watching driver
/// would: the diff's plan, then the survivors' sets. Returns the diff.
fn swap(
    recorder: &mut SessionRecorder,
    engine: &mut Engine,
    old: &Project,
    new: &Project,
    registry: &Registry,
) -> Result<ProjectEdit, EngineError> {
    let edit = diff(old, new, registry).unwrap();
    let ProjectEdit::Structural { plan, sets, .. } = &edit else {
        panic!("expected a structural edit");
    };
    let mut plan = plan.clone();
    plan.version = recorder.next_version();
    let frame = engine.next_frame();
    let mut prepared = engine.prepare_swap(&plan, registry)?;
    engine.commit_swap(&mut prepared, frame)?;
    assert_eq!(
        recorder.applied_swap(frame, new, registry).unwrap(),
        plan.version
    );
    for change in sets {
        send(
            recorder,
            engine,
            frame.0,
            &change.port.to_string(),
            change.payload,
        );
    }
    Ok(edit)
}

/// The scripted live session: parameter sets, a ramp, a trigger, an event
/// stamped late, a swap that adds `/osc2`, its removal (suspend, then a swap
/// at the fade's end), and a suspend and resume. Returns the live render and
/// the recorder.
fn live_session() -> (Render, SessionRecorder) {
    let registry = registry();
    let (v0, v1, v2) = (version(0), version(1), version(2));
    let mut recorder =
        SessionRecorder::new(&v0, &registry, v0.sample_rate, v0.seed, provenance()).unwrap();
    let mut plan = v0.plan(&registry).unwrap().plan;
    plan.version = 0;
    let config = EngineConfig::new(v0.sample_rate, v0.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let mut removal = None;
    let rendered = render_with(&mut engine, FRAMES, |engine| {
        engine.reclaim_externals().for_each(drop);
        engine.reclaim_edits().for_each(drop);
        let now = engine.next_frame().0;
        match now {
            100 => send(
                &mut recorder,
                engine,
                now,
                "/osc.freq",
                EventPayload::Set(550.0),
            ),
            200 => send(
                &mut recorder,
                engine,
                now,
                "/out.gain",
                EventPayload::Ramp {
                    to: 0.25,
                    frames: 400,
                    curve: Curve::Linear,
                },
            ),
            250 => send(
                &mut recorder,
                engine,
                now,
                "/env.trigger",
                EventPayload::Trigger,
            ),
            // Stamped for a frame that has already run: effective at 300.
            300 => send(
                &mut recorder,
                engine,
                290,
                "/osc.freq",
                EventPayload::Set(500.0),
            ),
            ADD => {
                let edit = swap(&mut recorder, engine, &v0, &v1, &registry)?;
                let ProjectEdit::Structural {
                    added,
                    removed,
                    sets,
                    ..
                } = edit
                else {
                    unreachable!()
                };
                assert_eq!(added, vec![addr("/osc2")]);
                assert!(removed.is_empty());
                assert_eq!(sets.len(), 1, "the file changed /osc.freq");
                send(
                    &mut recorder,
                    engine,
                    now,
                    "/osc2.freq",
                    EventPayload::Set(990.0),
                );
            }
            REMOVE => {
                edit(&mut recorder, engine, EditKind::Suspend, "/osc2");
                removal = Some(engine.removal_swap_frame(Frame(now)).0);
            }
            2_500 => edit(&mut recorder, engine, EditKind::Suspend, "/env"),
            2_600 => edit(&mut recorder, engine, EditKind::Resume, "/env"),
            _ if Some(now) == removal => {
                let edit = swap(&mut recorder, engine, &v1, &v2, &registry)?;
                let ProjectEdit::Structural { removed, .. } = edit else {
                    unreachable!()
                };
                assert_eq!(removed, vec![addr("/osc2")]);
            }
            _ => {}
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(engine.input_discards(), Default::default());
    (rendered, recorder)
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Replays `dir` with the binary (`--json`, plus `extra`); returns its JSON.
fn replayed(dir: &Path, extra: &[&str]) -> Value {
    let dir = dir.display().to_string();
    let mut args = vec!["replay", dir.as_str(), "--json"];
    args.extend_from_slice(extra);
    let output = signal(&args);
    assert!(output.status.success(), "{}", stderr(&output));
    stdout_json(&output)
}

fn hashes(json: &Value) -> (String, String) {
    (
        json["manifest"]["audio_fnv1a64"]
            .as_str()
            .unwrap()
            .to_owned(),
        json["manifest"]["log_fnv1a64"].as_str().unwrap().to_owned(),
    )
}

fn hashes_of(render: &Render) -> (String, String) {
    (
        format!("{:016x}", render.audio_fnv1a64()),
        format!("{:016x}", render.log_fnv1a64()),
    )
}

/// Every file under `dir`, by relative path, with its bytes.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap().display().to_string();
                files.push((relative, fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

fn copy_tree(from: &Path, to: &Path) {
    for (relative, bytes) in tree(from) {
        let path = to.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

/// A recorded session, written once per caller into `name`.
fn recorded(name: &str) -> (Render, PathBuf) {
    let (rendered, recorder) = live_session();
    let dir = out_dir(name);
    recorder.write(&dir).unwrap();
    (rendered, dir)
}

/// AC2 (V10, offline form), S2-format, S3-recorder, S3-replay.
#[test]
fn live_session_replays_bit_identically() {
    let (live, dir) = recorded("ac2/session");

    // The directory: session.toml, a project copy per version, edits.jsonl.
    let session = fs::read_to_string(dir.join("session.toml")).unwrap();
    for expected in [
        "format_version = 1",
        "sample_rate = 8000",
        "seed = 3",
        "conversion = \"device\"",
        "[provenance]",
        "signal_revision = \"session-test\"",
        "[[versions]]",
    ] {
        assert!(session.contains(expected), "{expected} in\n{session}");
    }
    assert_eq!(session.matches("[[versions]]").count(), 3);
    for n in 0..3 {
        assert_eq!(
            fs::read(dir.join(format!("projects/v{n}/project.toml"))).unwrap(),
            fs::read(fixture(&format!("session/v{n}/project.toml"))).unwrap(),
            "projects/v{n} is an exact copy"
        );
    }
    let edits = fs::read_to_string(dir.join("edits.jsonl")).unwrap();
    for kind in ["set", "ramp", "trigger", "suspend", "resume", "swap"] {
        assert!(edits.contains(&format!(r#""kind":"{kind}""#)), "{kind}");
    }
    let lines: Vec<&str> = edits.lines().collect();
    assert!(
        lines[0]
            .starts_with(r#"{"frame":100,"seq":1,"kind":"set","target":"/osc.freq","value":550"#)
    );
    // The late event is recorded at its effective frame, as the log has it.
    assert!(
        lines.contains(&r#"{"frame":300,"seq":4,"kind":"set","target":"/osc.freq","value":500.0}"#)
    );
    assert!(lines.contains(&r#"{"frame":1000,"seq":5,"kind":"swap","version":1}"#));
    let log = String::from_utf8(live.log.clone()).unwrap();
    assert!(log.contains(r#"{"type":"event","frame":300,"target":"/osc.freq","payload":{"set":500.0},"source":{"external":{"sequence":4}}}"#));
    assert_eq!(log.matches(r#""type":"swap""#).count(), 2);
    assert!(log.contains(r#"{"type":"suspend","frame":1500,"address":"/osc2"}"#));

    // `signal replay` reproduces the audio and the log bit for bit.
    let out = out_dir("ac2/replay");
    let json = replayed(
        &dir,
        &[
            "--out",
            &out.display().to_string(),
            "--frames",
            &FRAMES.to_string(),
        ],
    );
    assert_eq!(hashes(&json), hashes_of(&live));
    assert_eq!(json["frames"], FRAMES);
    assert_eq!(json["skipped"], 0);
    let reference = out_dir("ac2/live");
    live.write(&reference, provenance()).unwrap();
    for file in ["audio.wav", "log.jsonl"] {
        assert_eq!(
            fs::read(out.join(file)).unwrap(),
            fs::read(reference.join(file)).unwrap(),
            "{file}"
        );
    }
    for file in ["audio.wav", "manifest.json", "log.jsonl", "metrics.json"] {
        assert!(out.join(file).is_file(), "{file}");
    }

    // The text form prints the audio hash.
    let output = signal(&[
        "replay",
        &dir.display().to_string(),
        "--frames",
        &FRAMES.to_string(),
    ]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains(&format!("audio_fnv1a64 {}", hashes_of(&live).0)),
        "{text}"
    );
}

/// AC3: the same hashes on every replay, in another process or in this one,
/// from a recording made twice or a session written by hand; the format
/// holds no clock. Without `--frames` the replay ends one second after the
/// last command.
#[test]
fn replay_does_not_depend_on_when_or_how_the_session_was_made() {
    let (live, dir) = recorded("ac3/first");
    let (_, again) = recorded("ac3/second");
    assert_eq!(
        tree(&dir),
        tree(&again),
        "recording the script twice gives the same bytes"
    );

    let frames = FRAMES.to_string();
    let first = replayed(&dir, &["--frames", &frames]);
    let second = replayed(&again, &["--frames", &frames]);
    assert_eq!(hashes(&first), hashes_of(&live));
    assert_eq!(hashes(&second), hashes_of(&live));

    let registry = registry();
    let session = Session::load(&dir, &registry).unwrap();
    let in_process = replay(&session, &registry, Some(FRAMES), 1).unwrap();
    assert_eq!(hashes_of(&in_process.render), hashes_of(&live));

    let default = replayed(&dir, &[]);
    let last = session.edits.last().unwrap().frame.0;
    assert_eq!(default["frames"], last + 1 + 8_000);
    assert_eq!(hashes(&default), hashes(&replayed(&dir, &[])));

    // Written by hand: a set to the instance a swap adds, at the swap's frame
    // with a lower seq, is accepted and applies after the swap.
    let hand = fixture("session-hand");
    let once = replayed(&hand, &[]);
    assert_eq!(hashes(&once), hashes(&replayed(&hand, &[])));
    let out = out_dir("ac3/hand");
    replayed(&hand, &["--out", &out.display().to_string()]);
    let log = fs::read_to_string(out.join("log.jsonl")).unwrap();
    assert!(log.contains(r#"{"type":"swap","frame":100,"version":1,"plan_version":1}"#));
    assert!(log.contains(r#"{"type":"event","frame":100,"target":"/osc2.freq","payload":{"set":880.0},"source":{"external":{"sequence":3}}}"#), "{log}");
}

/// auditor-1-002: a frame with more commands than the default queue
/// capacity replays, with every event in the log.
#[test]
fn a_frame_with_more_commands_than_the_default_queue_replays() {
    const COMMANDS: usize = 5_000;
    let registry = registry();
    let v0 = version(0);
    let mut recorder =
        SessionRecorder::new(&v0, &registry, v0.sample_rate, v0.seed, provenance()).unwrap();
    let plan = v0.plan(&registry).unwrap().plan;
    let config = EngineConfig {
        queue_capacity: COMMANDS,
        ..EngineConfig::new(v0.sample_rate, v0.seed)
    };
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    for i in 0..COMMANDS {
        let (target, value) = if i % 2 == 0 {
            ("/osc.freq", 200.0 + i as f64 / 10.0)
        } else {
            ("/out.gain", 0.25 + i as f64 / 100_000.0)
        };
        send(
            &mut recorder,
            &mut engine,
            0,
            target,
            EventPayload::Set(value),
        );
    }
    let live = render_with(&mut engine, 2, |_| Ok(())).unwrap();
    let dir = out_dir("queue/session");
    recorder.write(&dir).unwrap();

    let out = out_dir("queue/replay");
    let json = replayed(
        &dir,
        &["--frames", "2", "--out", &out.display().to_string()],
    );
    assert_eq!(hashes(&json), hashes_of(&live));
    let log = fs::read_to_string(out.join("log.jsonl")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|l| l.contains(r#""type":"event""#))
            .count(),
        COMMANDS
    );
    // In process: no discards and no overflow, or replay fails.
    let session = Session::load(&dir, &registry).unwrap();
    replay(&session, &registry, Some(2), 1).unwrap();
}

/// The value of every `set` and `ramp`, as bits, in order.
fn values_of(edits: &[session::Edit]) -> Vec<u64> {
    edits
        .iter()
        .filter_map(|e| match e.command {
            session::Command::Event {
                payload: EventPayload::Set(v) | EventPayload::Ramp { to: v, .. },
                ..
            } => Some(v.to_bits()),
            _ => None,
        })
        .collect()
}

/// auditor-1-004: every recorded value reads back from `edits.jsonl` bit for
/// bit, including values a parser that is not correctly rounded gets wrong,
/// and the binary replay is byte-identical to the live render.
#[test]
fn recorded_values_round_trip_bit_for_bit() {
    let registry = registry();
    let v0 = version(0);
    let mut recorder =
        SessionRecorder::new(&v0, &registry, v0.sample_rate, v0.seed, provenance()).unwrap();
    let plan = v0.plan(&registry).unwrap().plan;
    let config = EngineConfig::new(v0.sample_rate, v0.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();

    // Known hard cases, then a broad deterministic sample: finite values from
    // random bit patterns (all magnitudes, subnormals included) and random
    // values in [0, 1000).
    let mut values = vec![
        0.9999999999999999,
        948.2052553993453,
        0.1,
        1.0 / 3.0,
        f64::from_bits(0x000f_ffff_ffff_ffff),
        5e-324,
        f64::MIN_POSITIVE,
        f64::MAX,
        -0.0,
        9007199254740993.0,
    ];
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    while values.len() < 600 {
        let random = f64::from_bits(next());
        if random.is_finite() {
            values.push(random);
        }
        values.push((next() >> 11) as f64 / (1u64 << 53) as f64 * 1000.0);
    }

    // Sets and ramps to parameters whose effective value is clamped, one per
    // frame; the gain gets the first hard case.
    let frames = values.len() as u64 + 2;
    let live = render_with(&mut engine, frames, |engine| {
        let i = engine.next_frame().0 as usize;
        let stamp = i as u64;
        if let Some(&value) = values.get(i) {
            if i.is_multiple_of(2) {
                send(
                    &mut recorder,
                    engine,
                    stamp,
                    "/env.decay_ms",
                    EventPayload::Set(value),
                );
            } else {
                let ramp = EventPayload::Ramp {
                    to: value,
                    frames: 3,
                    curve: Curve::Linear,
                };
                send(&mut recorder, engine, stamp, "/osc.freq", ramp);
            }
        } else if i == values.len() {
            send(
                &mut recorder,
                engine,
                stamp,
                "/out.gain",
                EventPayload::Set(values[0]),
            );
        }
        Ok(())
    })
    .unwrap();
    let dir = out_dir("roundtrip/session");
    recorder.write(&dir).unwrap();

    let recorded = values_of(&recorder.edits());
    assert_eq!(recorded.len(), values.len() + 1);
    let session = Session::load(&dir, &registry).unwrap();
    assert_eq!(
        values_of(&session.edits),
        recorded,
        "edits.jsonl reads back bit for bit"
    );

    let out = out_dir("roundtrip/replay");
    let json = replayed(
        &dir,
        &[
            "--frames",
            &frames.to_string(),
            "--out",
            &out.display().to_string(),
        ],
    );
    assert_eq!(hashes(&json), hashes_of(&live));
    let reference = out_dir("roundtrip/live");
    live.write(&reference, provenance()).unwrap();
    for file in ["audio.wav", "log.jsonl"] {
        assert_eq!(
            fs::read(out.join(file)).unwrap(),
            fs::read(reference.join(file)).unwrap(),
            "{file}"
        );
    }
}

/// S3-recorder: effective frames by the engine's rule, a refused push
/// records nothing, and an invalid session is not written.
#[test]
fn the_recorder_records_what_the_engine_applied() {
    let registry = registry();
    let (v0, v1, v2) = (version(0), version(1), version(2));
    let mut recorder =
        SessionRecorder::new(&v0, &registry, v0.sample_rate, v0.seed, provenance()).unwrap();

    // max(stamp, next frame, last of the same kind), events and edits apart.
    let set = |v| EventPayload::Set(v);
    let event = recorder
        .event(Frame(5), Frame(10), port("/osc.freq"), set(1.0))
        .unwrap();
    assert_eq!(event.frame, Frame(10));
    recorder.applied_event(&event).unwrap();
    let event = recorder
        .event(Frame(3), Frame(2), port("/osc.freq"), set(2.0))
        .unwrap();
    assert_eq!(event.frame, Frame(10));
    let pending = recorder
        .edit(EditKind::Suspend, Frame(4), Frame(2), addr("/env"))
        .unwrap();
    assert_eq!(pending.frame, Frame(4));

    // Refused before the push: input the engine would discard.
    assert!(
        recorder
            .event(Frame(0), Frame(0), port("/nope.freq"), set(1.0))
            .is_err()
    );
    assert!(
        recorder
            .event(Frame(0), Frame(0), port("/out.l"), set(1.0))
            .is_err()
    );
    assert!(
        recorder
            .edit(EditKind::Resume, Frame(0), Frame(0), addr("/osc"))
            .is_err()
    );

    // A push the engine hands back is not recorded.
    let plan = v0.plan(&registry).unwrap().plan;
    let config = EngineConfig {
        queue_capacity: 1,
        ..EngineConfig::new(v0.sample_rate, v0.seed)
    };
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let mut recorder =
        SessionRecorder::new(&v0, &registry, v0.sample_rate, v0.seed, provenance()).unwrap();
    for value in [1.0, 2.0] {
        let event = recorder
            .event(Frame(0), engine.next_frame(), port("/osc.freq"), set(value))
            .unwrap();
        if engine.push_external(event.clone()).is_ok() {
            recorder.applied_event(&event).unwrap();
        }
    }
    assert_eq!(recorder.edits().len(), 1);

    // An event for a later frame whose target a swap before it removes is
    // refused at write time, with its line.
    let mut recorder =
        SessionRecorder::new(&v0, &registry, v0.sample_rate, v0.seed, provenance()).unwrap();
    recorder.applied_swap(Frame(10), &v1, &registry).unwrap();
    let late = recorder
        .event(Frame(50), Frame(11), port("/osc2.freq"), set(5.0))
        .unwrap();
    recorder.applied_event(&late).unwrap();
    recorder.applied_swap(Frame(20), &v2, &registry).unwrap();
    let dir = out_dir("recorder/invalid");
    let error = recorder.write(&dir).unwrap_err().to_string();
    assert!(error.starts_with("edits.jsonl:3:1: error: /osc2.freq is not an event input or parameter of version 2 at frame 50"), "{error}");
    assert!(!dir.exists(), "nothing is written");
}

/// What `signal replay` printed for a broken copy of a recorded session.
struct Broken {
    code: Option<i32>,
    stderr: String,
}

/// Copies the recorded session `dir` to `name`, breaks it with `change`, and
/// replays it.
fn broken(dir: &Path, name: &str, change: impl FnOnce(&Path)) -> Broken {
    let copy = out_dir(&format!("ac4/{name}"));
    copy_tree(dir, &copy);
    change(&copy);
    let output = signal(&["replay", &copy.display().to_string(), "--frames", "10"]);
    let json = signal(&[
        "replay",
        &copy.display().to_string(),
        "--frames",
        "10",
        "--json",
    ]);
    assert_eq!(json.status.code(), Some(1), "{name}");
    let kind = stdout_json(&json)["error"]["kind"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(kind == "validation" || kind == "io", "{name}: {kind}");
    Broken {
        code: output.status.code(),
        stderr: stderr(&output),
    }
}

/// Rewrites `file` in `dir` with `change`.
fn rewrite(dir: &Path, file: &str, change: impl FnOnce(String) -> String) {
    let path = dir.join(file);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, change(text)).unwrap();
}

/// Replaces line `n` (from 1) of `text`.
fn replace_line(text: String, n: usize, line: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    lines[n - 1] = line.to_owned();
    lines.join("\n") + "\n"
}

/// The line number (from 1) of the first line of `edits.jsonl` containing
/// `needle`.
fn line_of(dir: &Path, needle: &str) -> usize {
    fs::read_to_string(dir.join("edits.jsonl"))
        .unwrap()
        .lines()
        .position(|l| l.contains(needle))
        .unwrap()
        + 1
}

fn assert_fails(result: Broken, located: &str, message: &str) {
    assert_eq!(result.code, Some(1), "{}", result.stderr);
    assert!(
        result
            .stderr
            .lines()
            .any(|l| l.contains(located) && l.contains(message)),
        "no line with `{located}` and `{message}` in:\n{}",
        result.stderr
    );
}

/// AC4: malformed sessions fail with the file and line, exit code 1.
#[test]
fn malformed_sessions_fail_with_located_errors() {
    let (_, dir) = recorded("ac4/recorded");
    let swap = line_of(&dir, r#""kind":"swap","version":1"#);
    let first_set = 1;
    let suspend_env = line_of(&dir, r#""kind":"suspend","address":"/env""#);
    let resume_env = line_of(&dir, r#""kind":"resume","address":"/env""#);
    let suspend_osc2 = line_of(&dir, r#""kind":"suspend","address":"/osc2""#);
    let lines = fs::read_to_string(dir.join("edits.jsonl"))
        .unwrap()
        .lines()
        .count();
    let edits = |dir: &Path, n: usize, line: &str| {
        let line = line.to_owned();
        rewrite(dir, "edits.jsonl", |t| replace_line(t, n, &line))
    };

    // Files and syntax.
    let result = broken(&dir, "missing", |d| {
        fs::remove_file(d.join("session.toml")).unwrap()
    });
    assert_fails(result, "session.toml", "No such file");
    let result = broken(&dir, "toml", |d| {
        rewrite(d, "session.toml", |t| replace_line(t, 3, "seed = = 3"))
    });
    assert_fails(result, "session.toml:3:", "error");
    let result = broken(&dir, "format", |d| {
        rewrite(d, "session.toml", |t| {
            t.replace("format_version = 1", "format_version = 2")
        })
    });
    assert_fails(result, "session.toml:1:18:", "unsupported format_version 2");
    let result = broken(&dir, "conversion", |d| {
        rewrite(d, "session.toml", |t| {
            t.replace("conversion = \"device\"", "conversion = \"loud\"")
        })
    });
    assert_fails(result, "session.toml:", "unknown conversion \"loud\"");
    let result = broken(&dir, "json", |d| edits(d, 2, r#"{"frame":200,"seq":2,"#));
    assert_fails(result, "edits.jsonl:2:", "EOF");
    let result = broken(&dir, "kind", |d| {
        edits(
            d,
            1,
            r#"{"frame":100,"seq":1,"kind":"poke","target":"/osc.freq"}"#,
        )
    });
    assert_fails(result, "edits.jsonl:1:", "unknown variant `poke`");
    let result = broken(&dir, "field", |d| {
        edits(
            d,
            1,
            r#"{"frame":100,"seq":1,"kind":"set","target":"/osc.freq","value":1,"extra":2}"#,
        )
    });
    assert_fails(result, "edits.jsonl:1:", "unknown field `extra`");

    // Order, seq and versions.
    let result = broken(&dir, "order", |d| {
        rewrite(d, "edits.jsonl", |t| {
            let mut lines: Vec<&str> = t.lines().collect();
            lines.swap(0, 1);
            lines.join("\n") + "\n"
        })
    });
    assert_fails(result, "edits.jsonl:2:", "does not come after");
    let result = broken(&dir, "seq", |d| {
        rewrite(d, "edits.jsonl", |t| {
            t.replacen(r#""seq":2,"#, r#""seq":1,"#, 1)
        })
    });
    assert_fails(result, "edits.jsonl:2:", "seq 1 is used twice");
    let result = broken(&dir, "version", |d| {
        rewrite(d, "edits.jsonl", |t| {
            t.replace(r#""version":1"#, r#""version":9"#)
        })
    });
    assert_fails(
        result,
        &format!("edits.jsonl:{swap}:"),
        "swap to version 9; the next version is 1",
    );

    // Project copies.
    let result = broken(&dir, "hash", |d| {
        rewrite(d, "projects/v1/project.toml", |t| t + "# changed\n")
    });
    assert_fails(result, "projects/v1/project.toml:1:1:", "FNV-1a 64");
    let result = broken(&dir, "project", |d| {
        rewrite(d, "projects/v1/project.toml", |t| {
            t.replace(
                "osc2 = { module = \"std.sine_osc\"",
                "osc2 = { module = \"std.nope\"",
            )
        })
    });
    assert_fails(result, "projects/v1/project.toml:12:", "std.nope");

    // Commands the engine would discard, judged against the plan running at
    // their frame.
    let set_at = |frame: u64, seq: u64, target: &str| {
        format!(r#"{{"frame":{frame},"seq":{seq},"kind":"set","target":"{target}","value":1}}"#)
    };
    let result = broken(&dir, "unknown-target", |d| {
        edits(d, first_set, &set_at(100, 1, "/missing.freq"))
    });
    assert_fails(
        result,
        "edits.jsonl:1:",
        "/missing.freq is not an event input or parameter of version 0 at frame 100",
    );
    let result = broken(&dir, "signal-input", |d| {
        edits(d, first_set, &set_at(100, 1, "/out.l"))
    });
    assert_fails(
        result,
        "edits.jsonl:1:",
        "/out.l is not an event input or parameter",
    );
    let result = broken(&dir, "before-added", |d| {
        edits(d, first_set, &set_at(100, 1, "/osc2.freq"))
    });
    assert_fails(
        result,
        "edits.jsonl:1:",
        "/osc2.freq is not an event input or parameter of version 0",
    );
    let result = broken(&dir, "after-removed", |d| {
        rewrite(d, "edits.jsonl", |t| {
            t + &set_at(2_900, 999, "/osc2.freq") + "\n"
        })
    });
    assert_fails(
        result,
        &format!("edits.jsonl:{}:", lines + 1),
        "/osc2.freq is not an event input or parameter of version 2",
    );
    let result = broken(&dir, "missing-instance", |d| {
        rewrite(d, "edits.jsonl", |t| {
            t.replace(
                r#""kind":"suspend","address":"/osc2""#,
                r#""kind":"suspend","address":"/nope""#,
            )
        })
    });
    assert_fails(
        result,
        &format!("edits.jsonl:{suspend_osc2}:"),
        "/nope is not an instance of version 1",
    );
    let result = broken(&dir, "double-suspend", |d| {
        rewrite(d, "edits.jsonl", |t| {
            t.replace(
                r#""kind":"resume","address":"/env""#,
                r#""kind":"suspend","address":"/env""#,
            )
        })
    });
    assert_fails(
        result,
        &format!("edits.jsonl:{resume_env}:"),
        "/env is already suspended",
    );
    let result = broken(&dir, "resume-running", |d| {
        rewrite(d, "edits.jsonl", |t| {
            t.replace(
                r#""kind":"suspend","address":"/env""#,
                r#""kind":"resume","address":"/env""#,
            )
        })
    });
    assert_fails(
        result,
        &format!("edits.jsonl:{suspend_env}:"),
        "/env is not suspended",
    );
    let result = broken(&dir, "two-edits", |d| {
        rewrite(d, "edits.jsonl", |t| {
            t.replace(r#"{"frame":2600,"#, r#"{"frame":2500,"#)
        })
    });
    assert_fails(
        result,
        &format!("edits.jsonl:{resume_env}:"),
        "already suspended or resumed at frame 2500",
    );

    // The audio sink.
    let result = broken(&dir, "no-sink", |d| {
        rewrite(d, "projects/v0/project.toml", |t| {
            t.replace("out = { module = \"std.audio_out\", gain = 0.5 }\n", "")
                .replace("  { from = \"osc.out\", to = \"out.l\" },\n", "")
                .replace("  { from = \"env.out\", to = \"out.r\" },\n", "")
        })
    });
    assert_fails(
        result,
        "projects/v0/project.toml:",
        "the project has no audio sink",
    );
    let result = broken(&dir, "two-sinks", |d| {
        rewrite(d, "projects/v0/project.toml", |t| {
            t.replace(
                "[wiring]",
                "out2 = { module = \"std.audio_out\" }\n\n[wiring]",
            )
        })
    });
    assert_fails(
        result,
        "projects/v0/project.toml:",
        "the project has 2 audio sinks",
    );
    let result = broken(&dir, "sink-renamed", |d| {
        rewrite(d, "projects/v1/project.toml", |t| {
            t.replace("out = {", "main = {")
                .replace("\"out.", "\"main.")
        })
    });
    assert_fails(
        result,
        "projects/v1/project.toml:",
        "the audio sink must stay /out with 2 inputs",
    );
}

/// M2-4: a session that records its end (`frames` in session.toml) replays
/// exactly that many frames by default; one without keeps the last command
/// plus one second. The end lies after every command: `set_end` refuses an
/// earlier one, `write` refuses a session with a command at or after it, and
/// loading a session whose `frames` was edited below a command fails with
/// the command's line.
#[test]
fn a_recorded_end_is_the_default_replay_length() {
    let (live, mut recorder) = live_session();
    let last = recorder.edits().last().unwrap().frame.0;
    let rate = u64::from(version(0).sample_rate);

    let plain = out_dir("end/plain");
    recorder.write(&plain).unwrap();
    let text = fs::read_to_string(plain.join("session.toml")).unwrap();
    assert!(!text.contains("frames ="), "{text}");
    assert_eq!(replayed(&plain, &[])["frames"], last + 1 + rate);

    assert!(recorder.set_end(last).is_err(), "a command at the end");
    recorder.set_end(FRAMES).unwrap();
    let ended = out_dir("end/recorded");
    recorder.write(&ended).unwrap();
    let text = fs::read_to_string(ended.join("session.toml")).unwrap();
    assert!(text.contains(&format!("frames = {FRAMES}")), "{text}");
    let json = replayed(&ended, &[]);
    assert_eq!(json["frames"], FRAMES);
    assert_eq!(json["skipped"], 0);
    assert_eq!(
        hashes(&json),
        hashes_of(&live),
        "the recorded run, bit for bit"
    );
    let session = Session::load(&ended, &registry()).unwrap();
    assert_eq!(session.frames, Some(FRAMES));
    assert_eq!(session.default_frames(), FRAMES);

    // A command recorded after the end was set: not written.
    let v0 = version(0);
    let mut late =
        SessionRecorder::new(&v0, &registry(), v0.sample_rate, v0.seed, provenance()).unwrap();
    late.set_end(10).unwrap();
    let event = late
        .event(
            Frame(20),
            Frame(0),
            port("/osc.freq"),
            EventPayload::Set(500.0),
        )
        .unwrap();
    late.applied_event(&event).unwrap();
    let refused = out_dir("end/late");
    assert!(matches!(
        late.write(&refused),
        Err(signal_project::session::SessionError::Invalid(_))
    ));
    assert!(!refused.exists());

    let result = broken(&ended, "end-before-a-command", |dir| {
        rewrite(dir, "session.toml", |text| {
            text.replace(&format!("frames = {FRAMES}"), &format!("frames = {last}"))
        });
    });
    assert_fails(result, "edits.jsonl:", "at or after the session's end");
}

/// M2.7-4 (C5, AC2): `signal replay --ideal` labels its manifest `ideal`;
/// a plain replay of a session without latent modules has no `ideal` and,
/// as the session records the device conversion, says `device_conversion`;
/// nothing clipped, so the two render the same audio and log.
#[test]
fn ideal_replay_is_labelled_and_changes_nothing_without_latent_modules() {
    let (_, dir) = recorded("ideal/session");
    let exact = replayed(&dir, &[]);
    assert_eq!(exact["ideal"], false);
    assert_eq!(exact["boundaries"], "not-latent");
    assert!(exact["manifest"].get("ideal").is_none(), "{exact}");
    assert_eq!(exact["manifest"]["device_conversion"], true, "{exact}");
    assert_eq!(exact["clipped"], 0);
    let ideal = replayed(&dir, &["--ideal"]);
    assert_eq!(ideal["ideal"], true);
    assert_eq!(ideal["boundaries"], "ideal");
    assert_eq!(ideal["manifest"]["ideal"], true);
    assert_eq!(hashes(&ideal), hashes(&exact));
}
