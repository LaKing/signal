//! ADR 0057 §3 (M6): `projects/clock-follow` follows an external MIDI clock
//! end to end. A virtual MIDI source (the test's "drum machine") sends a
//! Start and two beats of clock at 120 bpm, a Stop, then a second Start and
//! a beat; a real-time run of the project reads it by name. Nothing plays
//! before the first Start, the kicks fall on the clock's beats, nothing
//! plays from the Stop to the next Start, which plays the downbeat again
//! (a first build lost it: the Start and its first tick read in one block
//! looked like a jump back, a seek), and
//! `signal replay` of the run's session renders bit for bit what the run
//! played. Skipped where there is no MIDI server.

use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use signal_audio::midi_in::MidiInput;

fn samples(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .into_samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

/// `ticks` clock ticks at 120 bpm (24 a beat, 20.83 ms apart), paced
/// against a start time so the tempo does not drift.
fn clock(source: &mut MidiInput, ticks: u32) {
    let start = Instant::now();
    for i in 0..ticks {
        let due = start + Duration::from_micros(u64::from(i) * 500_000 / 24);
        thread::sleep(due.saturating_duration_since(Instant::now()));
        source.send_virtual(&[0xF8]).unwrap();
    }
}

/// The frames where `left` rises above `level` after staying under it for
/// at least `quiet` frames: the kicks (the snare and hats stay under).
fn onsets(left: &[f32], level: f32, quiet: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut last_loud: Option<usize> = None;
    for (i, v) in left.iter().enumerate() {
        if v.abs() > level {
            if last_loud.is_none_or(|l| i - l >= quiet) {
                out.push(i);
            }
            last_loud = Some(i);
        }
    }
    out
}

#[test]
fn the_drums_follow_a_midi_clock_and_replay_exactly() {
    let mut machine = match MidiInput::open("no such source") {
        Ok(input) => input,
        Err(why) => return eprintln!("skipped: {why}"),
    };
    let name = format!("Signal test clock {}", std::process::id());
    machine.connect_virtual(&name).unwrap();

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("clock-follow");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/clock-follow");
    let session = dir.join("session");
    let live = dir.join("live.wav");
    let mut run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .arg("play")
        .arg(&project)
        .args(["--driver", "null", "--seconds", "4"])
        .args(["--midi", &format!("clock={name}")])
        .args(["--record-session", session.to_str().unwrap()])
        .args(["--record-audio", live.to_str().unwrap()])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = run.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let reader = thread::spawn(move || {
        let mut all = String::new();
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            let _ = tx.send(line.clone());
            all.push_str(&line);
            all.push('\n');
        }
        all
    });
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(30)) {
        if line.contains("playing") {
            break;
        }
    }
    thread::sleep(Duration::from_millis(400));
    machine.send_virtual(&[0xFA]).unwrap();
    clock(&mut machine, 49);
    machine.send_virtual(&[0xFC]).unwrap();
    thread::sleep(Duration::from_millis(900));
    machine.send_virtual(&[0xFA]).unwrap();
    clock(&mut machine, 25);
    machine.send_virtual(&[0xFC]).unwrap();
    let status = run.wait().unwrap();
    let stderr = reader.join().unwrap();
    assert!(status.success(), "{stderr}");
    assert!(stderr.contains(&format!("MIDI from {name}")), "{stderr}");

    let played = samples(&live);
    let left: Vec<f32> = played.iter().step_by(2).copied().collect();
    let first = left.iter().position(|v| *v != 0.0);
    let first = first.unwrap_or_else(|| panic!("the drums played\n{stderr}"));
    assert!(
        first > 48_000 / 4,
        "nothing before the first Start (sound from frame {first})"
    );
    // The first run of the clock: kicks on beats 0, 1 and 2, half a second
    // apart (within a block and the feed's poll); after the Stop, the next
    // Start plays the downbeat again, then beat 1.
    let kicks = onsets(&left, 0.3, 9_600);
    assert_eq!(kicks.len(), 5, "five kicks: {kicks:?}\n{stderr}");
    for pair in [[0, 1], [1, 2], [3, 4]] {
        let apart = kicks[pair[1]] - kicks[pair[0]];
        assert!(
            (22_000..26_000).contains(&apart),
            "a beat at 120 bpm is 24000 frames: {kicks:?}"
        );
    }
    assert!(
        kicks[3] - kicks[2] > 40_000,
        "quiet from the Stop to the next Start: {kicks:?}"
    );

    let replayed = dir.join("replayed");
    let frames = left.len().to_string();
    let replay = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "replay",
            session.to_str().unwrap(),
            "--frames",
            &frames,
            "--out",
        ])
        .arg(&replayed)
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert_eq!(
        samples(&replayed.join("audio.wav")),
        played,
        "the replay is what was played"
    );
}
