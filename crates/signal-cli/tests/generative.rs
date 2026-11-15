//! ADR 0056 verification 6 (the test suite's part): `projects/generative`
//! renders the same on 1 and 4 threads and in blocks of 64 and 256; its
//! first section is the intro, and its sections follow the form's chain.
//! The 20-minute render, its hash and every section occurring, is the
//! release-build run recorded in `docs/benchmarks.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One section of the piece: 8 bars of 4 beats at 100 bpm.
const SECTION: usize = 48_000 * 60 / 100 * 32;

fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/generative")
}

/// A copy of the project with the form's state on a report slot.
fn reported(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    let copy = Command::new("cp")
        .args(["-R", project().to_str().unwrap(), dir.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(copy.success());
    let path = dir.join("project.toml");
    let text = fs::read_to_string(&path).unwrap();
    let text = text
        .replace(
            "out = { module = \"std.audio_out\", gain = 0.25 }",
            "out = { module = \"std.audio_out\", gain = 0.25 }\nreport = { device = { play = [\"form\"], role = \"report\" } }",
        )
        .replacen(
            "connections = [\n",
            "connections = [\n  { from = \"form.state\", to = \"report.form\" },\n",
            1,
        );
    fs::write(path, text).unwrap();
}

fn render(dir: &Path, out: &Path, frames: usize, extra: &[&str]) -> serde_json::Value {
    let run = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["--json", "render", dir.to_str().unwrap(), "--frames"])
        .arg(frames.to_string())
        .args(["--out", out.to_str().unwrap()])
        .args(extra)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    serde_json::from_slice(&run.stdout).unwrap()
}

#[test]
fn the_piece_renders_the_same_however_it_is_split() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("generative");
    let frames = 48_000 * 20;
    let hashes: Vec<serde_json::Value> = [["--threads", "1", "--block", "64"], ["--threads", "4", "--block", "256"]]
        .iter()
        .enumerate()
        .map(|(i, extra)| {
            render(&project(), &base.join(format!("split-{i}")), frames, extra)["manifest"]["audio_fnv1a64"].clone()
        })
        .collect();
    assert_eq!(hashes[0], hashes[1]);
}

#[test]
fn the_piece_starts_with_its_intro_and_moves_on() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("generative-form");
    let copy = base.join("copy");
    fs::create_dir_all(&base).unwrap();
    reported(&copy);
    render(&copy, &base.join("out"), SECTION * 3, &["--threads", "4"]);
    let form: Vec<f32> = hound::WavReader::open(base.join("out/devices/report.wav"))
        .unwrap()
        .into_samples::<f32>()
        .map(Result::unwrap)
        .collect();
    // States: start, intro, groove, build, peak, breakdown.
    assert_eq!(form[0], 1.0, "the first section is the intro");
    assert_eq!(form[SECTION - 1], 1.0);
    let second = form[SECTION + 1];
    assert!(
        second == 2.0 || second == 3.0,
        "from the intro: a groove or a build, {second}"
    );
}
