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

/// The review of 4d37bf8: a replay writes a discrete main side's
/// `audio.wav` as the render does, without speaker positions.
#[test]
fn a_replayed_discrete_main_side_has_no_speaker_positions() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("discrete-replay");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n[project]\nname = \"stems\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\none = { module = \"std.const\", value = 0.25 }\n\
         stems = { device = { play = [\"a\", \"b\", \"c\", \"d\"], role = \"main\" } }\n\
         [wiring]\nconnections = [ { from = \"one.out\", to = \"stems.c\" } ]\n",
    )
    .unwrap();
    let signal = |args: &[&str]| {
        let run = Command::new(env!("CARGO_BIN_EXE_signal"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    };
    let session = dir.join("session");
    signal(&[
        "play",
        dir.to_str().unwrap(),
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "0.1",
        "--record-session",
        session.to_str().unwrap(),
    ]);
    let out = dir.join("replayed");
    signal(&[
        "replay",
        session.to_str().unwrap(),
        "--frames",
        "2400",
        "--out",
        out.to_str().unwrap(),
    ]);
    let bytes = fs::read(out.join("audio.wav")).unwrap();
    assert_eq!(
        u16::from_le_bytes([bytes[22], bytes[23]]),
        4,
        "four channels"
    );
    assert_eq!(&bytes[40..44], &[0, 0, 0, 0], "speaker mask 0");
}

/// The review of 4d37bf8: a render over an earlier one in the same folder
/// retires the device files it does not write.
#[test]
fn a_render_over_another_retires_its_device_files() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("discrete-retire");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("stems")).unwrap();
    fs::create_dir_all(dir.join("plain")).unwrap();
    fs::write(
        dir.join("stems/project.toml"),
        "schema_version = 1\n[project]\nname = \"stems\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\nout = { module = \"std.audio_out\" }\n\
         stems = { device = { play = [\"a\", \"b\"], role = \"stems\" } }\n",
    )
    .unwrap();
    fs::write(
        dir.join("plain/project.toml"),
        "schema_version = 1\n[project]\nname = \"plain\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\nout = { module = \"std.audio_out\" }\n",
    )
    .unwrap();
    let out = dir.join("out");
    let render = |project: &str| {
        let run = Command::new(env!("CARGO_BIN_EXE_signal"))
            .args([
                "render",
                dir.join(project).to_str().unwrap(),
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
    };
    render("stems");
    assert!(out.join("devices/stems.wav").is_file());
    render("plain");
    assert!(!out.join("devices").exists(), "the stems' file retired");
    assert!(out.join("audio.wav").is_file());

    // The review of 99b764b: files placed beside a render's own are never
    // retired, only the ones a run recorded as its outputs.
    render("stems");
    fs::write(out.join("devices/reference.wav"), b"mine").unwrap();
    fs::write(out.join("devices/fixture.dmx"), b"mine").unwrap();
    fs::write(out.join("devices/notes.txt"), b"mine").unwrap();
    render("plain");
    assert!(!out.join("devices/stems.wav").exists());
    for kept in ["reference.wav", "fixture.dmx", "notes.txt"] {
        assert!(out.join("devices").join(kept).is_file(), "{kept} deleted");
    }

    // A replay into the folder retires the earlier render's device files.
    let session = dir.join("session");
    let play = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "play",
            dir.join("plain").to_str().unwrap(),
            "--driver",
            "null",
            "--fast",
            "--seconds",
            "0.01",
            "--record-session",
            session.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        play.status.success(),
        "{}",
        String::from_utf8_lossy(&play.stderr)
    );
    render("stems");
    assert!(out.join("devices/stems.wav").is_file());
    let replay = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "replay",
            session.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert!(
        !out.join("devices/stems.wav").exists(),
        "the replay left it"
    );
    assert!(out.join("devices/reference.wav").is_file());
}
