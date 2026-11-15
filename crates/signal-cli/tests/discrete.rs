//! ADR 0038 §2, §9: a discrete slot renders in its own WAV,
//! `devices/<slot>.wav`, its channels the ports in order and its speaker
//! mask 0 (no positions), whatever the machine.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn a_discrete_slot_renders_its_ports_in_order_without_speaker_positions() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("discrete-render");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n[project]\nname = \"stems\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\none = { module = \"std.const\", value = 0.25 }\n\
         out = { module = \"std.audio_out\" }\n\
         stems = { device = { play = [\"kick\", \"snare\", \"hat\", \"pad\"], role = \"stems\" } }\n\
         [wiring]\nconnections = [ { from = \"one.out\", to = \"stems.hat\" } ]\n",
    )
    .unwrap();
    let out = dir.join("out");
    let run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "render",
            dir.to_str().unwrap(),
            "--frames",
            "64",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let file = out.join("devices/stems.wav");
    let reader = hound::WavReader::open(&file).unwrap();
    assert_eq!(reader.spec().channels, 4);
    let samples: Vec<f32> = reader.into_samples::<f32>().map(Result::unwrap).collect();
    for frame in samples.chunks(4) {
        assert_eq!(frame, [0.0, 0.0, 0.25, 0.0]);
    }
    let bytes = fs::read(&file).unwrap();
    assert_eq!(
        u16::from_le_bytes([bytes[20], bytes[21]]),
        0xFFFE,
        "extensible"
    );
    assert_eq!(&bytes[40..44], &[0, 0, 0, 0], "speaker mask 0");
}
