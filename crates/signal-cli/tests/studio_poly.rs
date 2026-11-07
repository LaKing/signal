//! `projects/studio-poly` (ADR 0044 phase 1, decision 3): the studio with a
//! pad of eight `moog_voice` voices in a `lib.poly_voice` slot, played by
//! `std.note_seq`'s `notes`. Its golden render (16 s, with no fault, no mute
//! and a safe peak), added with the project (not a regeneration); the same
//! audio in blocks of 1, 64 and 256 frames on one and four threads; and the
//! pad's chords on several voices (eight voices louder than one). How to
//! update the golden: `docs/testing-and-evaluation.md`. The committed
//! project is only read.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// 16 s at 48 kHz (8 bars at 120 bpm): the golden's length.
const GOLDEN_FRAMES: u64 = 16 * 48_000;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn project() -> PathBuf {
    repository().join("projects/studio-poly")
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("studio-poly-{name}"));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Renders `project` for `frames` in blocks of `block` on `threads`
/// threads into `out`, and gives its manifest.
fn render(project: &Path, frames: u64, block: usize, threads: usize, out: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "render",
            project.to_str().unwrap(),
            "--frames",
            &frames.to_string(),
            "--block",
            &block.to_string(),
            "--threads",
            &threads.to_string(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap()
}

fn faults(log: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(log)
        .lines()
        .filter(|line| line.contains("\"fault\"") || line.contains("\"mute\""))
        .map(str::to_owned)
        .collect()
}

fn samples(out: &Path) -> Vec<f64> {
    let mut reader = hound::WavReader::open(out.join("audio.wav")).unwrap();
    reader
        .samples::<f32>()
        .map(|s| f64::from(s.unwrap()))
        .collect()
}

fn rms(samples: &[f64]) -> f64 {
    (samples.iter().map(|x| x * x).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn golden_studio_poly_16_seconds() {
    let out = scratch("golden");
    let mut manifest = render(&project(), GOLDEN_FRAMES, 256, 1, &out);
    let log = fs::read(out.join("log.jsonl")).unwrap();
    assert_eq!(faults(&log), Vec::<String>::new());
    let samples = samples(&out);
    assert_eq!(samples.len() as u64, 2 * GOLDEN_FRAMES);
    assert!(samples.iter().all(|x| x.is_finite()));
    let peak = samples.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    assert!(peak <= 0.25, "peak {peak}");
    assert!(peak > 0.05, "the project plays: peak {peak}");

    let fields = manifest.as_object_mut().unwrap();
    fields.remove("debug_assertions");
    let provenance = fields["provenance"].as_object_mut().unwrap();
    for volatile in [
        "signal_revision",
        "signal_dirty",
        "rustc_version",
        "cargo_lock_hash",
    ] {
        provenance.remove(volatile);
    }
    let keys = |v: &Value| -> BTreeSet<String> { v.as_object().unwrap().keys().cloned().collect() };
    let names =
        |list: &[&str]| -> BTreeSet<String> { list.iter().map(|s| s.to_string()).collect() };
    assert_eq!(
        keys(&manifest),
        names(&[
            "provenance",
            "seed",
            "sample_rate",
            "frames",
            "log_fnv1a64",
            "audio_fnv1a64"
        ]),
        "a new manifest field must be added to the golden or listed as volatile"
    );
    assert_eq!(
        keys(&manifest["provenance"]),
        names(&["project_file_hashes", "library_interface_hashes"])
    );
    let golden = repository().join("tests/golden/studio-poly/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}

/// 4 s in blocks of 1, 64 and 256 on one and four threads: one audio and
/// one log.
#[test]
fn renders_identically_in_blocks_and_on_threads() {
    let frames = 4 * 48_000;
    let reference = render(&project(), frames, 256, 1, &scratch("ref"));
    for (block, threads) in [(1, 1), (64, 1), (1, 4), (64, 4), (256, 4)] {
        let found = render(
            &project(),
            frames,
            block,
            threads,
            &scratch(&format!("b{block}-t{threads}")),
        );
        for hash in ["audio_fnv1a64", "log_fnv1a64"] {
            assert_eq!(
                found[hash], reference[hash],
                "{hash} at block {block} on {threads} threads"
            );
        }
    }
}

/// The pad alone (the other buses at 0): eight voices play the chords,
/// one voice only the latest note of each, so eight are louder.
#[test]
fn the_pad_plays_its_chords_on_several_voices() {
    let text = fs::read_to_string(project().join("project.toml")).unwrap();
    let alone = text.replace(
        "gain_a = 0.45, gain_b = 0.45, gain_c = 0.55",
        "gain_a = 0, gain_b = 0, gain_c = 0",
    );
    assert_ne!(alone, text, "the bus line the test edits");
    let mut levels = Vec::new();
    for voices in [8, 1] {
        let dir = scratch(&format!("pad-{voices}"));
        copy_dir(&project(), &dir);
        let edited = alone.replace("voices = 8", &format!("voices = {voices}"));
        fs::write(dir.join("project.toml"), edited).unwrap();
        let out = dir.join("out");
        render(&dir, 8 * 48_000, 256, 1, &out);
        levels.push(rms(&samples(&out)));
    }
    assert!(levels[1] > 0.0, "{levels:?}");
    assert!(levels[0] > 1.5 * levels[1], "{levels:?}");
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()));
        } else {
            fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}
