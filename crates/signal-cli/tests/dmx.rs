//! ADR 0057 §2 (M6 phase 1): a `dmx` device slot's channels are DMX levels.
//! `signal render` writes the frames a run would send, every 1091 frames
//! at 48 kHz (under 44 Hz), to `devices/<slot>.dmx`, each level a byte;
//! an audio play slot is unaffected and an unknown kind is refused.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn project(name: &str, instances: &str, wiring: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("dmx")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("project.toml"),
        format!(
            "schema_version = 1\n[project]\nname = \"{name}\"\nsample_rate = 48000\nseed = 1\n\
             [instances]\nout = {{ module = \"std.audio_out\" }}\n{instances}\n\
             [wiring]\nconnections = [\n{wiring}]\n"
        ),
    )
    .unwrap();
    dir
}

fn render(dir: &std::path::Path, frames: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "--json",
            "render",
            dir.to_str().unwrap(),
            "--frames",
            frames,
            "--out",
        ])
        .arg(dir.join("out"))
        .output()
        .unwrap()
}

#[test]
fn a_dmx_slot_renders_the_frames_a_run_sends() {
    let dir = project(
        "lights",
        "half = { module = \"std.const\", value = 0.5 }\n\
         full = { module = \"std.const\", value = 3 }\n\
         ramp = { module = \"std.frames\" }\n\
         scale = { module = \"std.mul\" }\n\
         per = { module = \"std.const\", value = 0.0000001 }\n\
         lights = { device = { kind = \"dmx\", play = [\"red\", \"green\", \"blue\"], role = \"lights\" } }",
        "  { from = \"half.out\", to = \"lights.red\" },\n  { from = \"full.out\", to = \"lights.green\" },\n\
           { from = \"ramp.lo\", to = \"scale.a\" },\n  { from = \"per.out\", to = \"scale.b\" },\n\
           { from = \"scale.out\", to = \"lights.blue\" },\n",
    );
    let run = render(&dir, "48000");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(report["devices"]["lights"]["file"], "devices/lights.dmx");
    let text = fs::read_to_string(dir.join("out/devices/lights.dmx")).unwrap();
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("# signal dmx: 3 channels, a frame every 1091 frames")
    );
    let frames: Vec<(u64, String)> = lines
        .map(|l| {
            let (f, b) = l.split_once(' ').unwrap();
            (f.parse().unwrap(), b.to_owned())
        })
        .collect();
    // 0, 1091, … below 48 000: 44 frames, at most 44 a second.
    assert_eq!(frames.len(), 44);
    assert!(
        frames
            .iter()
            .enumerate()
            .all(|(i, (f, _))| *f == i as u64 * 1091)
    );
    // red 0.5 → 0x80, green clamped to 0xff, blue the frame number × 1e-7.
    assert!(frames.iter().all(|(_, b)| &b[..4] == "80ff"));
    let (f, b) = &frames[40];
    let blue = u8::from_str_radix(&b[4..6], 16).unwrap();
    assert_eq!(blue, ((*f as f64 * 1e-7).min(1.0) * 255.0).round() as u8);
}

#[test]
fn a_dmx_slot_names_its_channels() {
    let dir = project(
        "bad",
        "lights = { device = { kind = \"dmx\", play = \"stereo\" } }",
        "",
    );
    let said = |run: std::process::Output| {
        assert!(!run.status.success());
        format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        )
    };
    assert!(said(render(&dir, "10")).contains("plays its channels by name"));
    let dir = project(
        "midi",
        "keys = { device = { kind = \"laser\", play = [\"a\"] } }",
        "",
    );
    assert!(said(render(&dir, "10")).contains("supplies `audio` and `dmx`"));
}

/// ADR 0057 §2, phase 1's live exit: a run sends each DMX frame as an
/// ArtDmx packet to the bound node: the header as the specification lays
/// it out, the role's universe, sequence numbers in order from 1, the
/// levels as bytes, and in a second of real time at most 44 packets.
#[test]
fn a_run_sends_art_dmx_to_its_node() {
    let dir = project(
        "live",
        "half = { module = \"std.const\", value = 0.5 }\n\
         lights = { device = { kind = \"dmx\", play = [\"red\", \"blue\"], role = \"lights\" } }",
        "  { from = \"half.out\", to = \"lights.red\" },\n",
    );
    let node = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    node.set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let target = format!("lights={}/2", node.local_addr().unwrap());
    let run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "play",
            dir.to_str().unwrap(),
            "--driver",
            "null",
            "--seconds",
            "1",
            "--artnet",
            &target,
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let mut packets = Vec::new();
    let mut buffer = [0u8; 1024];
    node.set_read_timeout(Some(std::time::Duration::from_millis(300)))
        .unwrap();
    while let Ok(n) = node.recv(&mut buffer) {
        packets.push(buffer[..n].to_vec());
    }
    // 0, 1091, … below 48 000: 44 frames in the second.
    assert_eq!(packets.len(), 44, "at most 44 a second");
    for (i, p) in packets.iter().enumerate() {
        assert_eq!(&p[..8], b"Art-Net\0");
        assert_eq!(&p[8..12], &[0x00, 0x50, 0, 14]);
        assert_eq!(p[12], (i + 1) as u8, "sequence in order from 1");
        assert_eq!(&p[14..16], &[2, 0], "universe 2");
        assert_eq!(&p[16..18], &[0, 2]);
        assert_eq!(&p[18..], &[0x80, 0]);
    }
}
