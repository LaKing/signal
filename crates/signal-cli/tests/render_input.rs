//! ADR 0038 §9: `signal render --input SLOT=WAV` feeds a capture slot from
//! a file offline. The render reads exactly the file's words (a mono mic
//! wired at gain 1 to both channels comes out as the file, bit for bit, and
//! 0 past its end); the manifest records the file by hash; a slot not fed
//! reads 0; a wrong rate, channel count or slot is refused.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

const RATE: u32 = 48_000;

fn signal(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .unwrap()
}

/// A project whose mono capture slot `mic` plays at gain 1 on both
/// channels of `main`, in a fresh folder named `name`.
fn project(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("render-input/{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("project.toml"),
        format!(
            "schema_version = 1\n[project]\nname = \"{name}\"\nsample_rate = {RATE}\nseed = 1\n\
             [instances]\n\
             mic = {{ device = {{ capture = \"mono\", role = \"voice\" }} }}\n\
             out = {{ module = \"std.audio_out\", gain = 1 }}\n\
             [wiring]\nconnections = [\n\
               {{ from = \"mic.m\", to = \"out.l\" }},\n\
               {{ from = \"mic.m\", to = \"out.r\" }},\n]\n"
        ),
    )
    .unwrap();
    dir
}

/// A float WAV of `samples` at `rate` with `channels`.
fn wav(path: &PathBuf, rate: u32, channels: u16, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels,
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

fn samples(path: &PathBuf) -> Vec<f32> {
    hound::WavReader::open(path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn an_input_file_is_what_the_render_reads() {
    let dir = project("fed");
    let take: Vec<f32> = (0..30_000)
        .map(|n| (n as f32 * 0.013).sin() * 0.4)
        .collect();
    let file = dir.join("take.wav");
    wav(&file, RATE, 1, &take);
    let out = dir.join("out");
    let run = signal(&[
        "render",
        dir.to_str().unwrap(),
        "--frames",
        "48000",
        "--out",
        out.to_str().unwrap(),
        "--input",
        &format!("mic={}", file.display()),
        "--json",
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let audio = samples(&out.join("audio.wav"));
    assert_eq!(audio.len(), 48_000 * 2);
    for (n, frame) in audio.chunks(2).enumerate() {
        let want = take.get(n).copied().unwrap_or(0.0);
        assert_eq!(frame, [want, want], "frame {n}");
    }
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    let hash = &manifest["provenance"]["render_input_hashes"]["mic"];
    assert_eq!(
        hash.as_str().unwrap(),
        format!("{:016x}", signal_core::fnv1a64_words(&take))
    );
    // On one thread and in blocks of 7: the same.
    let again = dir.join("again");
    let run = signal(&[
        "render",
        dir.to_str().unwrap(),
        "--frames",
        "48000",
        "--out",
        again.to_str().unwrap(),
        "--input",
        &format!("mic={}", file.display()),
        "--threads",
        "1",
        "--block",
        "7",
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(samples(&again.join("audio.wav")), audio);
}

#[test]
fn without_an_input_the_slot_reads_0_and_the_manifest_names_none() {
    let dir = project("unfed");
    let out = dir.join("out");
    let run = signal(&[
        "render",
        dir.to_str().unwrap(),
        "--frames",
        "4800",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(samples(&out.join("audio.wav")).iter().all(|&x| x == 0.0));
    let manifest = fs::read_to_string(out.join("manifest.json")).unwrap();
    assert!(!manifest.contains("render_input_hashes"), "{manifest}");
}

#[test]
fn a_wrong_rate_channel_count_or_slot_is_refused() {
    let dir = project("refused");
    let out = dir.join("out");
    let render = |input: String| {
        signal(&[
            "render",
            dir.to_str().unwrap(),
            "--frames",
            "480",
            "--out",
            out.to_str().unwrap(),
            "--input",
            &input,
        ])
    };
    let slow = dir.join("slow.wav");
    wav(&slow, 44_100, 1, &[0.0; 100]);
    let stereo = dir.join("stereo.wav");
    wav(&stereo, RATE, 2, &[0.0; 200]);
    for (input, said) in [
        (format!("mic={}", slow.display()), "44100 Hz"),
        (format!("mic={}", stereo.display()), "2 channels"),
        (format!("voice={}", stereo.display()), "no capture slot"),
        ("mic".to_owned(), "SLOT=WAV"),
    ] {
        let run = render(input.clone());
        assert!(!run.status.success(), "{input}");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(stderr.contains(said), "{input}: {stderr}");
    }
}

/// [`project`] with `mic`'s own render file `fixtures/voice.wav` of `take`.
fn with_fixture(name: &str, take: &[f32]) -> PathBuf {
    let dir = project(name);
    let text = fs::read_to_string(dir.join("project.toml")).unwrap().replace(
        "mic = { device = { capture = \"mono\", role = \"voice\" } }",
        "mic = { device = { capture = \"mono\", role = \"voice\" }, render = \"fixtures/voice.wav\" }",
    );
    fs::write(dir.join("project.toml"), text).unwrap();
    fs::create_dir_all(dir.join("fixtures")).unwrap();
    wav(&dir.join("fixtures/voice.wav"), RATE, 1, take);
    dir
}

#[test]
fn a_slots_render_file_feeds_it_and_is_a_project_file() {
    let take: Vec<f32> = (0..9_000).map(|n| (n as f32 * 0.021).cos() * 0.3).collect();
    let dir = with_fixture("fixture", &take);
    let out = dir.join("out");
    let run = signal(&[
        "render",
        dir.to_str().unwrap(),
        "--frames",
        "12000",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let audio = samples(&out.join("audio.wav"));
    for (n, frame) in audio.chunks(2).enumerate() {
        let want = take.get(n).copied().unwrap_or(0.0);
        assert_eq!(frame, [want, want], "frame {n}");
    }
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    let bytes = fs::read(dir.join("fixtures/voice.wav")).unwrap();
    assert_eq!(
        manifest["provenance"]["project_file_hashes"]["fixtures/voice.wav"]
            .as_str()
            .unwrap(),
        format!("{:016x}", signal_core::fnv1a64(&bytes))
    );
    // `--input` wins over it.
    let other = dir.join("other.wav");
    wav(&other, RATE, 1, &[0.25; 100]);
    let fed = dir.join("fed");
    let run = signal(&[
        "render",
        dir.to_str().unwrap(),
        "--frames",
        "200",
        "--out",
        fed.to_str().unwrap(),
        "--input",
        &format!("mic={}", other.display()),
    ]);
    assert!(run.status.success());
    let audio = samples(&fed.join("audio.wav"));
    assert_eq!(&audio[..4], [0.25; 4]);
    assert_eq!(&audio[200..], [0.0; 200]);
    // Packed, the file goes with the project: the .signal renders the same.
    let packed = dir.with_extension("signal");
    let _ = fs::remove_file(&packed);
    let run = signal(&["pack", dir.to_str().unwrap(), packed.to_str().unwrap()]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let from_file = dir.join("from-file");
    let run = signal(&[
        "render",
        packed.to_str().unwrap(),
        "--frames",
        "12000",
        "--out",
        from_file.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        samples(&from_file.join("audio.wav")),
        samples(&out.join("audio.wav"))
    );
}

#[test]
fn a_render_file_missing_misplaced_or_on_a_play_side_is_refused() {
    let dir = with_fixture("bad-fixture", &[0.0; 10]);
    fs::remove_file(dir.join("fixtures/voice.wav")).unwrap();
    let out = dir.join("out");
    let render = || {
        signal(&[
            "render",
            dir.to_str().unwrap(),
            "--frames",
            "100",
            "--out",
            out.to_str().unwrap(),
        ])
    };
    let run = render();
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("fixtures/voice.wav"),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = fs::read_to_string(dir.join("project.toml")).unwrap();
    for (bad, said) in [
        (
            text.replace("fixtures/voice.wav", "../voice.wav"),
            "inside the project folder",
        ),
        (
            text.replace("fixtures/voice.wav", "fixtures/voice.flac"),
            "WAV",
        ),
        (
            text.replace(
                "out = { module = \"std.audio_out\", gain = 1 }",
                "out = { module = \"std.audio_out\", gain = 1 }\ncue = { device = { play = \"stereo\", role = \"cue\" }, render = \"x.wav\" }",
            ),
            "only plays",
        ),
        (
            text.replace(
                "out = { module = \"std.audio_out\", gain = 1 }",
                "out = { module = \"std.audio_out\", gain = 1, render = \"x.wav\" }",
            ),
            "capture slot's key",
        ),
    ] {
        fs::write(dir.join("project.toml"), &bad).unwrap();
        let run = render();
        assert!(!run.status.success(), "{said}");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(stderr.contains(said), "{said}: {stderr}");
    }
}
