//! ADR 0057 §1, §3 (M6 phase 3): a `midi` device slot's values come from
//! CoreMIDI. This test makes a virtual MIDI source (visible to every
//! process), a real-time run connects to it by name, the test plays a note
//! and a controller through it, and `signal replay` of the run's session
//! renders bit for bit what the run played. Skipped where there is no MIDI
//! server.

use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use signal_audio::midi_in::MidiInput;

fn samples(path: &std::path::Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .into_samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn midi_values_are_recorded_and_replayed_exactly() {
    let mut keyboard = match MidiInput::open("no such source") {
        Ok(input) => input,
        Err(why) => return eprintln!("skipped: {why}"),
    };
    let name = format!("Signal test keys {}", std::process::id());
    keyboard.connect_virtual(&name).unwrap();

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("midi");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n[project]\nname = \"midi\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\nkeys = { device = { kind = \"midi\", capture = [\"note_velocity\", \"cc74\"], role = \"keys\" } }\n\
         out = { module = \"std.audio_out\", gain = 0.5 }\n\
         [wiring]\nconnections = [\n  { from = \"keys.note_velocity\", to = \"out.l\" },\n  { from = \"keys.cc74\", to = \"out.r\" },\n]\n",
    )
    .unwrap();
    let session = dir.join("session");
    let live = dir.join("live.wav");
    let mut run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "play",
            dir.to_str().unwrap(),
            "--driver",
            "null",
            "--seconds",
            "2",
        ])
        .args(["--midi", &format!("keys={name}")])
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
    thread::sleep(Duration::from_millis(200));
    keyboard.send_virtual(&[0x90, 60, 127]).unwrap();
    thread::sleep(Duration::from_millis(300));
    keyboard.send_virtual(&[0xB0, 74, 64]).unwrap();
    thread::sleep(Duration::from_millis(300));
    keyboard.send_virtual(&[0x80, 60, 0]).unwrap();
    let status = run.wait().unwrap();
    let stderr = reader.join().unwrap();
    assert!(status.success(), "{stderr}");
    assert!(stderr.contains(&format!("MIDI from {name}")), "{stderr}");

    let played = samples(&live);
    let left: Vec<f32> = played.iter().step_by(2).copied().collect();
    let right: Vec<f32> = played.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(left[0], 0.0, "0 before the first note");
    assert!(
        left.contains(&0.5),
        "the note's velocity, times the gain\n{stderr}"
    );
    let cc = 64.0f32 / 127.0 * 0.5;
    assert!(right.contains(&cc), "the controller arrived\n{stderr}");
    assert_eq!(
        *left.last().unwrap(),
        0.5,
        "released, the velocity holds (the gate drops)"
    );

    let replayed = dir.join("replayed");
    let frames = (played.len() / 2).to_string();
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
