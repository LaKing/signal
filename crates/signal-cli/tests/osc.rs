//! ADR 0057 §1 (M6 phase 2): an `osc` device slot's values come from OSC
//! messages, `/<role>/<name>` with a number. A real-time run takes them on
//! the bound address; its session records what the source read, and
//! `signal replay` renders bit for bit what the run played.

use std::fs;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use signal_output::osc::osc_message;

fn samples(path: &std::path::Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .into_samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn osc_values_are_recorded_and_replayed_exactly() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("osc");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n[project]\nname = \"osc\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\nsurface = { device = { kind = \"osc\", capture = [\"level\", \"pan\"], role = \"surface\" } }\n\
         out = { module = \"std.audio_out\", gain = 0.5 }\n\
         [wiring]\nconnections = [\n  { from = \"surface.level\", to = \"out.l\" },\n  { from = \"surface.pan\", to = \"out.r\" },\n]\n",
    )
    .unwrap();
    let port = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let address = format!("127.0.0.1:{port}");
    let session = dir.join("session");
    let live = dir.join("live.wav");
    let run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "play",
            dir.to_str().unwrap(),
            "--driver",
            "null",
            "--seconds",
            "2",
        ])
        .args(["--osc", &format!("surface={address}")])
        .args(["--record-session", session.to_str().unwrap()])
        .args(["--record-audio", live.to_str().unwrap()])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Messages go once the run plays (a datagram before the listener is lost).
    let mut run = run;
    let stderr = run.stderr.take().unwrap();
    let (lines_tx, lines_rx) = std::sync::mpsc::channel::<String>();
    let reader = thread::spawn(move || {
        use std::io::BufRead;
        let mut all = String::new();
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            let _ = lines_tx.send(line.clone());
            all.push_str(&line);
            all.push('\n');
        }
        all
    });
    while let Ok(line) = lines_rx.recv_timeout(Duration::from_secs(30)) {
        if line.contains("playing") {
            break;
        }
    }
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    for (wait, name, value) in [
        (200, "level", 0.5f32),
        (300, "pan", -0.25),
        (400, "level", 0.125),
    ] {
        thread::sleep(Duration::from_millis(wait));
        client
            .send_to(&osc_message(&format!("/surface/{name}"), value), &address)
            .unwrap();
    }
    let status = run.wait().unwrap();
    let stderr = reader.join().unwrap();
    assert!(status.success(), "{stderr}");
    assert!(stderr.contains("OSC on"), "{stderr}");

    let played = samples(&live);
    let left: Vec<f32> = played.iter().step_by(2).copied().collect();
    let right: Vec<f32> = played.iter().skip(1).step_by(2).copied().collect();
    let mut seen: Vec<f32> = left.clone();
    seen.dedup();
    assert!(
        left.contains(&0.25) && left.contains(&0.0625),
        "the levels arrived, times the gain: {seen:?}\n{stderr}"
    );
    assert!(right.contains(&-0.125), "the pan arrived");
    assert_eq!(left[0], 0.0, "0 before the first message");

    let replayed = dir.join("replayed");
    let frames = (played.len() / 2).to_string();
    let replay = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "replay",
            session.to_str().unwrap(),
            "--frames",
            &frames,
            "--out",
            replayed.to_str().unwrap(),
        ])
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
