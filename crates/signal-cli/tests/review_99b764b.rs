//! The outside review of 99b764b: recording destinations, replay's
//! recorded end and inputs, routes on the null driver's recording, and
//! `signal ui`'s per-run device flags.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("review-99b764b-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A stereo project playing 0.125 left and 0.5 right.
fn stereo(dir: &Path) -> String {
    let project = dir.join("stereo");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("project.toml"),
        "schema_version = 1\n[project]\nname = \"stereo\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\nl = { module = \"std.const\", value = 0.125 }\n\
         r = { module = \"std.const\", value = 0.5 }\nout = { module = \"std.audio_out\" }\n\
         [wiring]\nconnections = [\n  { from = \"l.out\", to = \"out.l\" },\n  { from = \"r.out\", to = \"out.r\" },\n]\n",
    )
    .unwrap();
    project.to_str().unwrap().to_owned()
}

/// The first frame and channel count of a float WAV.
fn first_frame(path: &Path) -> (usize, Vec<f32>) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let channels = usize::from(reader.spec().channels);
    let frame = reader
        .samples::<f32>()
        .take(channels)
        .map(Result::unwrap)
        .collect();
    (channels, frame)
}

/// A session destination that is a regular file is refused before the run;
/// one that fails only at the end still leaves the audio written.
#[test]
fn a_bad_session_destination_never_loses_the_audio() {
    let dir = scratch("destinations");
    let project = stereo(&dir);
    let file = dir.join("taken");
    fs::write(&file, b"x").unwrap();
    let wav = dir.join("take.wav");
    let refused = signal(&[
        "play",
        &project,
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "0.05",
        "--record-session",
        file.to_str().unwrap(),
        "--record-audio",
        wav.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("is not a directory"),
        "{}",
        stderr(&refused)
    );
    assert!(!wav.exists(), "refused before the run");

    let late = signal(&[
        "play",
        &project,
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "0.05",
        "--record-session",
        file.join("session").to_str().unwrap(),
        "--record-audio",
        wav.to_str().unwrap(),
    ]);
    assert!(!late.status.success(), "the session could not be written");
    assert!(wav.is_file(), "the audio was lost: {}", stderr(&late));
}

/// Without --ideal, a replay never renders past the recorded end, also for
/// a session without boundary records or inputs.
#[test]
fn a_replay_stops_at_the_recorded_end() {
    let dir = scratch("end");
    let project = stereo(&dir);
    let session = dir.join("session");
    let play = signal(&[
        "play",
        &project,
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "0.01",
        "--buffer",
        "480",
        "--record-session",
        session.to_str().unwrap(),
    ]);
    assert!(play.status.success(), "{}", stderr(&play));
    let text = fs::read_to_string(session.join("session.toml")).unwrap();
    assert!(text.contains("frames = 480"), "{text}");
    let past = signal(&["replay", session.to_str().unwrap(), "--frames", "960"]);
    assert!(!past.status.success());
    assert!(stderr(&past).contains("960"), "{}", stderr(&past));
    let ideal = signal(&[
        "replay",
        session.to_str().unwrap(),
        "--frames",
        "960",
        "--ideal",
    ]);
    assert!(ideal.status.success(), "{}", stderr(&ideal));
}

/// A complete input shorter than the session is refused when it loads.
#[test]
fn a_short_complete_input_is_refused() {
    let dir = scratch("short-input");
    let project = dir.join("listen");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("project.toml"),
        "schema_version = 1\n[project]\nname = \"listen\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\nmic = { device = { capture = \"mono\", role = \"voice\" } }\n\
         out = { module = \"std.audio_out\" }\n\
         [wiring]\nconnections = [\n  { from = \"mic.m\", to = \"out.l\" },\n]\n",
    )
    .unwrap();
    let session = dir.join("session");
    let play = signal(&[
        "play",
        project.to_str().unwrap(),
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "0.02",
        "--buffer",
        "480",
        "--record-session",
        session.to_str().unwrap(),
    ]);
    assert!(play.status.success(), "{}", stderr(&play));
    let ok = signal(&["replay", session.to_str().unwrap()]);
    assert!(ok.status.success(), "{}", stderr(&ok));
    let path = session.join("session.toml");
    let text = fs::read_to_string(&path).unwrap();
    let at = text.find("[[inputs]]").expect("an input");
    let (head, tail) = text.split_at(at);
    assert!(tail.contains("frames = 960"), "{tail}");
    fs::write(
        &path,
        format!("{head}{}", tail.replacen("frames = 960", "frames = 480", 1)),
    )
    .unwrap();
    let short = signal(&["replay", session.to_str().unwrap()]);
    assert!(!short.status.success());
    assert!(
        stderr(&short).contains("is complete but holds 480 frames of the session's 960"),
        "{}",
        stderr(&short)
    );
}

/// The null driver's recording takes the routes asked for.
#[test]
fn the_null_recording_is_routed() {
    let dir = scratch("routes");
    let project = stereo(&dir);
    let record = |name: &str, route: Option<&str>| {
        let wav = dir.join(name);
        let mut args = vec![
            "play",
            &project,
            "--driver",
            "null",
            "--fast",
            "--seconds",
            "0.01",
            "--record-audio",
            wav.to_str().unwrap(),
        ];
        if let Some(route) = route {
            args.extend(["--route", route]);
        }
        let run = signal(&args);
        assert!(run.status.success(), "{}", stderr(&run));
        first_frame(&wav)
    };
    assert_eq!(record("plain.wav", None), (2, vec![0.125, 0.5]));
    assert_eq!(
        record("swapped.wav", Some("main=2,1")),
        (2, vec![0.5, 0.125])
    );
    assert_eq!(
        record("moved.wav", Some("main=3,4")),
        (4, vec![0.0, 0.0, 0.125, 0.5])
    );
}

/// `signal ui` refuses the per-run device flags it would ignore.
#[test]
fn signal_ui_refuses_per_run_device_flags() {
    let dir = scratch("ui-flags");
    let project = stereo(&dir);
    for flag in [
        ["--artnet", "lights=127.0.0.1/0"],
        ["--osc", "surface=127.0.0.1:9000"],
        ["--midi", "keys=*"],
    ] {
        let run = signal(&["ui", &project, "--driver", "null", flag[0], flag[1]]);
        assert!(!run.status.success(), "{flag:?}");
        assert!(stderr(&run).contains("Setup"), "{flag:?}: {}", stderr(&run));
    }
}
