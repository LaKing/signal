//! ADR 0056 §1, §2: a sample pattern (`kind = "sample"`, a WAV in the
//! project folder) played by `std.sampler`. A render plays it word for word
//! from the trigger and hashes the WAV as a project file; `signal pack`
//! packs it and the archive renders the same; a session copies it into its
//! version, so `signal replay` needs nothing outside the session; a WAV of
//! another rate is refused with its reason.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SAMPLE: usize = 1000;

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn wav(path: &Path, rate: u32, samples: &[f32]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();
}

fn take() -> Vec<f32> {
    (0..SAMPLE)
        .map(|n| ((n as f32) * 0.05).sin() * 0.4)
        .collect()
}

/// A project whose sampler plays `samples/hit.wav` on each beat at 120 bpm.
fn project(name: &str, rate: u32) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sampler")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("patterns")).unwrap();
    wav(&dir.join("samples/hit.wav"), rate, &take());
    fs::write(
        dir.join("patterns/hit.toml"),
        "schema_version = 1\nkind = \"sample\"\nfile = \"samples/hit.wav\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n[project]\nname = \"sampler\"\nsample_rate = 48000\nseed = 1\n\
         [instances]\nclock = { module = \"std.transport\", bpm = 120 }\n\
         speed = { module = \"std.const\", value = 1 }\n\
         hit = { module = \"std.sampler\", pattern = \"hit\" }\n\
         out = { module = \"std.audio_out\" }\n\
         [wiring]\nconnections = [\n  { from = \"clock.beat\", to = \"hit.trigger\" },\n  { from = \"speed.out\", to = \"hit.rate\" },\n  { from = \"hit.l\", to = \"out.l\" },\n  { from = \"hit.r\", to = \"out.r\" },\n]\n",
    )
    .unwrap();
    dir
}

fn left(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .into_samples::<f32>()
        .map(Result::unwrap)
        .step_by(2)
        .collect()
}

#[test]
fn a_sample_plays_word_for_word_and_travels_with_the_project() {
    let dir = project("plays", 48_000);
    let out = dir.join("out");
    let run = ok(signal(&[
        "--json",
        "render",
        dir.to_str().unwrap(),
        "--frames",
        "48000",
        "--out",
        out.to_str().unwrap(),
    ]));
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    let audio = left(&out.join("audio.wav"));
    // The first beat is frame 0; the next at 24 000.
    assert_eq!(
        &audio[..SAMPLE],
        &take()[..],
        "word for word from the trigger"
    );
    assert!(
        audio[SAMPLE..24_000].iter().all(|&s| s == 0.0),
        "it stops at its end"
    );
    assert_eq!(&audio[24_000..24_000 + SAMPLE], &take()[..]);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert!(
        manifest["provenance"]["project_file_hashes"]["samples/hit.wav"].is_string(),
        "a project file: {manifest}"
    );

    // Packed, it renders the same.
    let packed = dir.with_extension("signal");
    ok(signal(&[
        "pack",
        dir.to_str().unwrap(),
        packed.to_str().unwrap(),
    ]));
    let again = ok(signal(&[
        "--json",
        "render",
        packed.to_str().unwrap(),
        "--frames",
        "48000",
        "--out",
        dir.join("out2").to_str().unwrap(),
    ]));
    let report2: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(
        report["manifest"]["audio_fnv1a64"],
        report2["manifest"]["audio_fnv1a64"]
    );

    // A session copies it: the replay needs nothing outside the session.
    let session = dir.join("session");
    ok(signal(&[
        "play",
        dir.to_str().unwrap(),
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "1",
        "--record-session",
        session.to_str().unwrap(),
    ]));
    assert!(session.join("projects/v0/samples/hit.wav").is_file());
    fs::remove_file(dir.join("samples/hit.wav")).unwrap();
    let replayed = ok(signal(&[
        "replay",
        session.to_str().unwrap(),
        "--frames",
        "48000",
        "--out",
        dir.join("replayed").to_str().unwrap(),
    ]));
    assert!(replayed.status.success());
    assert_eq!(left(&dir.join("replayed/audio.wav")), audio);
}

#[test]
fn a_sample_of_another_rate_is_refused() {
    let dir = project("rate", 44_100);
    let run = signal(&[
        "render",
        dir.to_str().unwrap(),
        "--frames",
        "100",
        "--out",
        dir.join("out").to_str().unwrap(),
    ]);
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("44100 Hz; the project plays at 48000 Hz"),
        "{stderr}"
    );
}
