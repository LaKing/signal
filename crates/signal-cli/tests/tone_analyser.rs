//! ADR 0055 verification 4: `projects/tone-analyser` measures a known loop
//! offline. Its output, delayed by 480 frames and halved, is fed back as
//! the capture role `loop` (`signal render --input loop=<wav>`); the report
//! slot's WAV (`devices/report.wav`, the discrete layout `latency`, `found`,
//! `peak_hz`, `peak`) then reads a round trip of 480 frames, the click
//! found, the tone at 1 kHz and at half its level, on 1 and 4 threads alike.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DELAY: usize = 480;
const FRAMES: usize = 96_000;

fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/tone-analyser")
}

fn render(out: &Path, extra: &[&str]) {
    let run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["render", project().to_str().unwrap(), "--frames"])
        .arg(FRAMES.to_string())
        .args(["--out", out.to_str().unwrap()])
        .args(extra)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

fn samples(path: &Path) -> (u16, Vec<f32>) {
    let reader = hound::WavReader::open(path).unwrap();
    let channels = reader.spec().channels;
    (
        channels,
        reader.into_samples::<f32>().map(Result::unwrap).collect(),
    )
}

#[test]
fn the_tone_analyser_measures_a_known_loop() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tone-analyser");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // What goes out (the loop reads 0 without an input file).
    render(&dir.join("dry"), &[]);
    let (channels, out) = samples(&dir.join("dry/audio.wav"));
    assert_eq!(channels, 2);
    let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    assert!(left.iter().all(|s| s.abs() <= 0.2 + 1e-6), "safe gain");

    // The loop: delayed and halved, mono.
    let looped = dir.join("loop.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&looped, spec).unwrap();
    for n in 0..FRAMES {
        let s = if n >= DELAY {
            left[n - DELAY] * 0.5
        } else {
            0.0
        };
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();

    let input = format!("loop={}", looped.display());
    let mut reports = Vec::new();
    for threads in ["1", "4"] {
        let out = dir.join(format!("wet-{threads}"));
        render(&out, &["--input", &input, "--threads", threads]);
        let (channels, report) = samples(&out.join("devices/report.wav"));
        assert_eq!(channels, 4, "latency, found, peak_hz, peak");
        reports.push(report);
    }
    assert_eq!(reports[0], reports[1], "the thread count never matters");
    let frame = |n: usize| &reports[0][n * 4..n * 4 + 4];
    // The first click goes out at half a period (frame 24 000).
    assert_eq!(frame(24_000 + DELAY - 1)[0], -1.0, "not back yet");
    let [latency, found, peak_hz, peak] = frame(FRAMES - 1).try_into().unwrap();
    assert_eq!(latency, DELAY as f32);
    assert_eq!(found, 1.0);
    assert!((peak_hz - 1000.0).abs() < 5.0, "peak_hz {peak_hz}");
    assert!((peak - 0.025).abs() < 0.025 * 0.01, "peak {peak}");
    assert_eq!(
        frame(24_000 + DELAY)[0],
        DELAY as f32,
        "measured as it returns"
    );
}
