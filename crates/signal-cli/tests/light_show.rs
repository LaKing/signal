//! ADR 0057 §5 (M6 phase 4): `projects/light-show`, blue on bass. Offline,
//! its DMX frames follow the sound: the blue breathes with the bass (high
//! under a note, near 0 in a rest), the dimmer flashes on each kick. Live,
//! a run sends them as Art-Net to a node and an OSC surface dims them: after
//! `/surface/dim 1` every packet's blue and dimmer are 0.

use std::io::BufRead;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use signal_output::osc::osc_message;

fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/light-show")
}

#[test]
fn the_light_follows_the_sound() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("light-show");
    let _ = std::fs::remove_dir_all(&out);
    let run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "render",
            project().to_str().unwrap(),
            "--seconds",
            "4",
            "--out",
        ])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = std::fs::read_to_string(out.join("devices/lights.dmx")).unwrap();
    let frames: Vec<(u64, [u8; 4])> = text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let (f, b) = l.split_once(' ').unwrap();
            let byte = |i: usize| u8::from_str_radix(&b[2 * i..2 * i + 2], 16).unwrap();
            (f.parse().unwrap(), [byte(0), byte(1), byte(2), byte(3)])
        })
        .collect();
    let blue: Vec<u8> = frames.iter().map(|(_, c)| c[2]).collect();
    assert!(blue.iter().any(|&b| b > 100), "under a note: {blue:?}");
    assert!(blue[44..].iter().any(|&b| b < 20), "in a rest: {blue:?}");
    // A kick on every beat (24 000 frames at 120 bpm): bright just after
    // it, dark just before the next.
    let at = |frame: u64| frames.iter().find(|(f, _)| *f >= frame).unwrap().1[3];
    for beat in 1..7u64 {
        let hit = beat * 24_000;
        assert!(at(hit + 1100) > 200, "beat {beat}: flash");
        assert!(at(hit - 2200) < 80, "beat {beat}: dark before the next");
    }
    assert!(
        frames.iter().all(|(_, c)| c[0] == 0 && c[1] == 0),
        "no tint before the surface"
    );
}

#[test]
fn a_surface_dims_the_light_a_run_sends() {
    let node = UdpSocket::bind("127.0.0.1:0").unwrap();
    let osc_port = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let osc = format!("127.0.0.1:{osc_port}");
    let mut run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "play",
            project().to_str().unwrap(),
            "--driver",
            "null",
            "--seconds",
            "3",
        ])
        .args([
            "--artnet",
            &format!("lights={}", node.local_addr().unwrap()),
        ])
        .args(["--osc", &format!("surface={osc}")])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = run.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            let _ = tx.send(line);
        }
    });
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(30)) {
        if line.contains("playing") {
            break;
        }
    }
    node.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut buffer = [0u8; 600];
    let mut before = Vec::new();
    while before.len() < 40 {
        let n = node.recv(&mut buffer).unwrap();
        before.push(buffer[18..n].to_vec());
    }
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .send_to(&osc_message("/surface/dim", 1.0), &osc)
        .unwrap();
    // A frame or two may still be on the way; then dark.
    thread::sleep(Duration::from_millis(200));
    let mut after = Vec::new();
    while let Ok(n) = node.recv(&mut buffer) {
        after.push(buffer[18..n].to_vec());
    }
    assert!(run.wait().unwrap().success());
    assert!(
        before.iter().any(|d| d[2] > 50 || d[3] > 50),
        "lit before: {before:?}"
    );
    let tail = &after[after.len().saturating_sub(20)..];
    assert!(
        tail.len() >= 10,
        "packets after the surface: {}",
        after.len()
    );
    assert!(
        tail.iter().all(|d| d[2] == 0 && d[3] == 0),
        "dimmed after: {tail:?}"
    );
}

/// ADR 0057 phase 4: the Setup binds the roles, as Signal.app's runs read
/// them (`signal ui` plays as the app does): `[play.lights] artnet` sends
/// the light to its node and universe, with no flag.
#[test]
fn the_setup_binds_the_light_for_a_host_run() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("light-show-setup");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copy = Command::new("cp")
        .args([
            "-R",
            project().to_str().unwrap(),
            dir.join("project").to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(copy.success());
    let node = UdpSocket::bind("127.0.0.1:0").unwrap();
    let setup = dir.join("setup.toml");
    std::fs::write(
        &setup,
        format!(
            "[play.lights]\nartnet = \"{}/3\"\n",
            node.local_addr().unwrap()
        ),
    )
    .unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "ui",
            dir.join("project").to_str().unwrap(),
            "--driver",
            "null",
            "--seconds",
            "2",
        ])
        .args(["--setup", setup.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    node.set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut buffer = [0u8; 600];
    let n = node.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..8], b"Art-Net\0");
    assert_eq!(&buffer[14..16], &[3, 0], "universe 3");
    assert_eq!(n, 18 + 4);
    let output = run.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
