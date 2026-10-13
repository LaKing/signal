//! `signal play` with the null driver (M2-4): a recorded session replays to
//! the recorded audio (AC1), `--watch` applies a parameter edit and a wiring
//! edit while it plays and keeps playing through a load error (AC2, AC3), and
//! the command line (AC6, AC7). No test here reaches an audio device.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

fn kick() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick")
}

/// A fresh folder for one test, inside the target directory, with a copy of
/// the kick project in `kick/`.
fn workspace(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("play")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    let mut pending = vec![kick()];
    while let Some(from) = pending.pop() {
        for entry in fs::read_dir(&from).unwrap() {
            let path = entry.unwrap().path();
            let to = dir.join("kick").join(path.strip_prefix(kick()).unwrap());
            if path.is_dir() {
                pending.push(path);
            } else {
                fs::create_dir_all(to.parent().unwrap()).unwrap();
                fs::copy(&path, &to).unwrap();
            }
        }
    }
    dir
}

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("the signal binary runs")
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

/// The samples of a 32-bit float WAV file.
fn samples(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    reader.samples::<f32>().map(Result::unwrap).collect()
}

/// Replays `session` with a bare `signal replay` (no `--frames`) into
/// `out`, and checks it gives `audio`'s samples bit for bit.
fn assert_replays_to(session: &Path, out: &Path, audio: &Path, frames: u64) {
    let output = signal(&["replay", &text(session), "--out", &text(out), "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["frames"], frames, "replay renders the recorded length");
    assert_eq!(json["skipped"], 0);
    let (replayed, recorded) = (samples(&out.join("audio.wav")), samples(audio));
    assert_eq!(replayed.len(), recorded.len());
    let first = replayed
        .iter()
        .zip(&recorded)
        .position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first, None, "replay equals the recorded audio bit for bit");
}

/// AC1: five seconds of the kick, null driver, as fast as possible, session
/// and audio recorded; `signal replay s` gives the same audio as `a.wav`.
#[test]
fn a_recorded_session_replays_to_the_recorded_audio() {
    let dir = workspace("ac1");
    let (session, audio) = (dir.join("s"), dir.join("a.wav"));
    let output = signal(&[
        "play",
        &text(&dir.join("kick")),
        "--driver",
        "null",
        "--fast",
        "--record-session",
        &text(&session),
        "--record-audio",
        &text(&audio),
        "--seconds",
        "5",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["frames"], 5 * 96_000);
    assert_eq!(json["sink_channels"], 2);
    assert_eq!(
        json["device_channels"], 2,
        "the null driver plays the sink width"
    );
    assert_eq!(json["driver"], "null");
    // AC7: every statistic is reported.
    for key in [
        "xruns",
        "worst_callback_ratio",
        "clipped",
        "queue_overflows",
    ] {
        assert!(!json[key].is_null(), "{key} is reported");
    }
    assert_eq!(
        json["xruns"]["total"],
        json["xruns"]["stream_errors"].as_u64().unwrap()
            + json["xruns"]["overruns"].as_u64().unwrap()
    );
    assert!(json["worst_callback_ratio"].as_f64().unwrap() > 0.0);
    // M2.6-3: the worker count and the scheduling path (the null driver
    // leaves its workers ordinary).
    let threads = json["threads"].as_u64().unwrap();
    assert!(threads >= 1);
    assert_eq!(json["workers"].as_u64(), Some(threads - 1));
    assert_eq!(json["scheduling"]["path"], "none");
    let workers = json["scheduling"]["workers"].as_array().unwrap();
    assert_eq!(workers.len() as u64, threads - 1);
    assert!(workers.iter().all(|w| w == "none (null driver)"));
    assert_eq!(json["clipped"], 0, "the kick never clips");
    assert_eq!(json["queue_overflows"]["total"], 0);
    let toml = fs::read_to_string(session.join("session.toml")).unwrap();
    assert!(toml.contains("frames = 480000"), "{toml}");

    assert_replays_to(&session, &dir.join("r"), &audio, 5 * 96_000);
}

/// M2.6-2 AC9: `--threads` on `play` and `replay` changes nothing: one
/// second of the kick through the null driver on 1 and 3 threads gives the
/// same audio, and `signal replay --threads 2` of the session gives it too.
#[test]
fn threads_on_play_and_replay_change_nothing() {
    let dir = workspace("threads");
    let play = |threads: &str| {
        let (session, audio) = (
            dir.join(format!("s{threads}")),
            dir.join(format!("a{threads}.wav")),
        );
        let output = signal(&[
            "play",
            &text(&dir.join("kick")),
            "--driver",
            "null",
            "--fast",
            "--threads",
            threads,
            "--record-session",
            &text(&session),
            "--record-audio",
            &text(&audio),
            "--seconds",
            "1",
            "--json",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        (session, audio)
    };
    let (session, one) = play("1");
    let (_, three) = play("3");
    assert!(
        samples(&one) == samples(&three),
        "--threads 3 plays the same"
    );

    let out = dir.join("r2");
    let output = signal(&[
        "replay",
        &text(&session),
        "--threads",
        "2",
        "--out",
        &text(&out),
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        samples(&out.join("audio.wav")) == samples(&one),
        "replay --threads 2 gives the played audio"
    );
}

/// Kills the child if the test fails before it ends.
struct Playing(Child);

impl Drop for Playing {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits for a stderr line containing every `needle`; returns it.
fn wait_for(lines: &Receiver<String>, seen: &mut Vec<String>, needles: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(line) => {
                seen.push(line.clone());
                if needles.iter().all(|n| line.contains(n)) {
                    return line;
                }
            }
            Err(_) => panic!(
                "no stderr line with {needles:?}; stderr was:\n{}",
                seen.join("\n")
            ),
        }
    }
}

/// Replaces `project.toml` atomically (written beside the folder, then
/// renamed), so the watch never reads half a file.
fn save(dir: &Path, text: &str) {
    let temp = dir.join("project.toml.next");
    fs::write(&temp, text).unwrap();
    fs::rename(&temp, dir.join("kick/project.toml")).unwrap();
}

/// AC2 and AC3: `--watch` on the null driver at real-time pace. The test sets
/// the output gain (a `set`), then saves a file that does not load (the
/// diagnostic is printed with file:line:col and the run keeps playing the
/// last good version), then removes a connection (a `swap`), then presses
/// Enter. The session holds the set and the swap and replays to the recorded
/// audio.
#[test]
fn watch_applies_a_set_and_a_swap_and_keeps_playing_through_a_load_error() {
    let dir = workspace("watch");
    let (session, audio) = (dir.join("s"), dir.join("a.wav"));
    let child = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "play",
            &text(&dir.join("kick")),
            "--driver",
            "null",
            "--watch",
            "--record-session",
            &text(&session),
            "--record-audio",
            &text(&audio),
            "--seconds",
            "600",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut playing = Playing(child);
    let stderr = playing.0.stderr.take().unwrap();
    let (tx, lines) = channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut seen = Vec::new();
    wait_for(&lines, &mut seen, &["playing"]);

    let original = fs::read_to_string(dir.join("kick/project.toml")).unwrap();
    let quieter = original.replace("gain = 0.75", "gain = 0.5");
    assert_ne!(quieter, original);
    save(&dir, &quieter);
    wait_for(&lines, &mut seen, &["watch: applied 1 sets"]);

    save(&dir, &quieter.replace("bpm = 132", "bpm = = 132"));
    let diagnostic = wait_for(&lines, &mut seen, &["project.toml:", ": error:"]);
    let located = diagnostic.split("project.toml:").nth(1).unwrap();
    let mut parts = located.split(':');
    let (line, column) = (parts.next().unwrap(), parts.next().unwrap());
    assert!(
        line.parse::<usize>().is_ok() && column.parse::<usize>().is_ok(),
        "file:line:col in {diagnostic}"
    );
    wait_for(&lines, &mut seen, &["keeping the last good version"]);

    let rewired = quieter.replace("  { from = \"drums.out\", to = \"out.r\" },\n", "");
    assert_ne!(rewired, quieter);
    save(&dir, &rewired);
    wait_for(&lines, &mut seen, &["watch: applied a swap to version 1"]);

    playing.0.stdin.take().unwrap().write_all(b"\n").unwrap();
    let mut stdout = String::new();
    std::io::Read::read_to_string(&mut playing.0.stdout.take().unwrap(), &mut stdout).unwrap();
    let status = playing.0.wait().unwrap();
    assert!(status.success(), "stderr:\n{}", seen.join("\n"));
    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["edits"]["sets"], 1);
    assert_eq!(json["edits"]["swaps"], 1);
    assert_eq!(json["edits"]["errors"], 1);
    assert_eq!(json["refused"], serde_json::json!([]));
    let frames = json["frames"].as_u64().unwrap();
    assert!(frames > 0);

    let edits = fs::read_to_string(session.join("edits.jsonl")).unwrap();
    let kinds: Vec<&str> = edits
        .lines()
        .map(|l| {
            if l.contains("\"kind\":\"set\"") {
                "set"
            } else if l.contains("\"kind\":\"swap\"") {
                "swap"
            } else {
                "other"
            }
        })
        .collect();
    assert_eq!(kinds, ["set", "swap"], "{edits}");
    assert!(
        edits.contains("\"target\":\"/out.gain\",\"value\":0.5"),
        "{edits}"
    );

    assert_replays_to(&session, &dir.join("r"), &audio, frames);
}

/// AC6: the flags, and the null driver's own flags refused with the device.
#[test]
fn play_and_devices_take_their_flags() {
    let help = signal(&["play", "--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    for flag in [
        "--device",
        "--buffer",
        "[default: 128]",
        "--seconds",
        "--watch",
        "--record-session",
        "--driver",
        "coreaudio",
        "null",
        "--fast",
        "--record-audio",
        "--json",
        "--rt-policy",
        "[default: park50]",
        "--rt-computation",
        "[default: 0.25]",
        "--rt-trace",
    ] {
        assert!(help.contains(flag), "{flag} in:\n{help}");
    }
    let help = signal(&["devices", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--json"));

    // Refused before any device is opened.
    let project = text(&kick());
    for flags in [&["--fast"][..], &["--record-audio", "x.wav"][..]] {
        let mut args = vec!["play", project.as_str(), "--seconds", "1", "--json"];
        args.extend_from_slice(flags);
        let output = signal(&args);
        assert_eq!(output.status.code(), Some(1), "{flags:?}");
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["error"]["kind"], "usage");
    }
}

/// M2.7-1 (C1, C2, C4, C6): every wake policy plays with the null driver,
/// with `--rt-computation` and the wake-up trace, reports what it ran and
/// the trace, and plays the same samples as every other; the default is
/// `park50` at 0.25; unknown policies and computations out of (0, 1] are
/// usage errors.
#[test]
fn every_wake_policy_plays_and_reports_its_trace() {
    let dir = workspace("policies");
    let project = text(&dir.join("kick"));
    let mut first: Option<Vec<f32>> = None;
    for policy in ["park50", "budget-spin", "semaphore", "fanout"] {
        let wav = dir.join(format!("{policy}.wav"));
        let output = signal(&[
            "--json",
            "play",
            &project,
            "--driver",
            "null",
            "--fast",
            "--seconds",
            "0.5",
            "--threads",
            "4",
            "--rt-policy",
            policy,
            "--rt-computation",
            "0.5",
            "--rt-trace",
            "--record-audio",
            &text(&wav),
        ]);
        assert!(
            output.status.success(),
            "{policy}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["rt_policy"], policy);
        assert_eq!(json["rt_computation"], 0.5);
        let waiter = if policy == "semaphore" {
            "semaphore"
        } else {
            "park"
        };
        assert_eq!(json["waiter"], waiter);
        for key in ["p99_callback_ratio", "median_callback_ratio"] {
            assert!(json[key].as_f64().is_some(), "{policy}: {key}");
        }
        let wake = &json["wake"];
        assert!(wake["callbacks"].as_u64().unwrap() > 0, "{policy}: {wake}");
        for key in ["wake_median_us", "first_join_p99_us", "last_join_max_us"] {
            assert!(wake[key].as_f64().is_some(), "{policy}: {key}");
        }
        assert_eq!(wake["per_worker"].as_array().unwrap().len(), 3);
        assert_eq!(json["rt_demotions"]["exposed"], false);
        assert_eq!(
            json["rt_demotions"]["samples"], 0,
            "the null driver samples none"
        );
        let audio = samples(&wav);
        match &first {
            None => first = Some(audio),
            Some(first) => assert!(
                audio
                    .iter()
                    .map(|x| x.to_bits())
                    .eq(first.iter().map(|x| x.to_bits())),
                "{policy} plays other samples"
            ),
        }
    }

    // The defaults, and no trace without --rt-trace.
    let output = signal(&[
        "--json",
        "play",
        &project,
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "0.1",
    ]);
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["rt_policy"], "park50");
    assert_eq!(json["rt_computation"], 0.25);
    assert!(json["wake"].is_null());

    for flags in [
        ["--rt-policy", "spin"],
        ["--rt-computation", "0"],
        ["--rt-computation", "1.5"],
    ] {
        let mut args = vec!["--json", "play", project.as_str(), "--driver", "null"];
        args.extend_from_slice(&flags);
        let output = signal(&args);
        assert_eq!(output.status.code(), Some(1), "{flags:?}");
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["error"]["kind"], "usage", "{flags:?}");
    }
}

/// AC6: without `--seconds`, a closed stdin ends the run like Enter: the
/// statistics are printed and the session is written.
#[test]
fn a_closed_stdin_ends_a_run_without_seconds() {
    let dir = workspace("eof");
    let session = dir.join("s");
    let output = signal(&[
        "play",
        &text(&dir.join("kick")),
        "--driver",
        "null",
        "--record-session",
        &text(&session),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in [
        "played ",
        "xruns ",
        "worst callback ",
        "clipped samples ",
        "queue overflows ",
    ] {
        assert!(stdout.contains(line), "{line} in:\n{stdout}");
    }
    assert!(session.join("session.toml").exists());
}
