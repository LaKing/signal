//! The M2.10-3 library demo `projects/library-demo` (AC11): it uses every
//! library composite, and its golden render (8 s, with no fault, no mute and
//! the output stage's safe peak) lists each with its content hash. How to
//! update the golden: `docs/testing-and-evaluation.md`. The committed
//! project is only read.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use signal_project::{Library, Project};

/// 8 s at 48 kHz (4 bars at 120 bpm): the golden's length.
const GOLDEN_FRAMES: u64 = 8 * 48_000;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn demo() -> PathBuf {
    repository().join("projects/library-demo")
}

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .expect("the signal binary runs")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// AC11: every library composite is in the plan, and no project composite.
#[test]
fn the_demo_uses_every_library_composite() {
    let project = Project::load(&demo()).unwrap();
    assert!(project.composites.is_empty());
    let used: Vec<String> = project.library_hashes().into_keys().collect();
    let all: Vec<String> = Library::builtin()
        .names()
        .map(|n| format!("lib.{n}"))
        .collect();
    assert_eq!(used, all);
}

/// AC11: the golden render, seed 13, 8 s at 48 kHz. Its manifest, without
/// the fields that change with the build rather than the sound, equals
/// `tests/golden/library-demo/manifest.json`, which lists every library
/// composite with its hash; the log has no fault or mute, every sample is
/// finite and the peak is at most 0.25 (the tanh limiters and the output
/// gain). One thread: the output is the same on any number.
#[test]
fn golden_library_demo_8_seconds() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("library-demo-golden");
    let _ = fs::remove_dir_all(&out);
    let (project, out_text, frames) = (
        demo().display().to_string(),
        out.display().to_string(),
        GOLDEN_FRAMES.to_string(),
    );
    let output = signal(&[
        "render",
        &project,
        "--frames",
        &frames,
        "--threads",
        "1",
        "--out",
        &out_text,
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let log = fs::read_to_string(out.join("log.jsonl")).unwrap();
    let faults: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\"") || l.contains("\"type\":\"mute\""))
        .collect();
    assert_eq!(faults, Vec::<&str>::new());
    let mut reader = hound::WavReader::open(out.join("audio.wav")).unwrap();
    let samples: Vec<f64> = reader
        .samples::<f32>()
        .map(|s| f64::from(s.unwrap()))
        .collect();
    assert_eq!(samples.len() as u64, 2 * GOLDEN_FRAMES);
    assert!(samples.iter().all(|s| s.is_finite()));
    let peak = samples.iter().fold(0.0f64, |m, s| m.max(s.abs()));
    println!("library-demo: peak {peak:.4}");
    assert!(peak <= 0.25, "peak {peak}");
    assert!(peak > 0.05, "the project plays: peak {peak}");

    let mut manifest = read_json(&out.join("manifest.json"));
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
        names(&["project_file_hashes", "library_composite_hashes"])
    );
    assert_eq!(
        manifest["provenance"]["library_composite_hashes"]
            .as_object()
            .unwrap()
            .len(),
        Library::builtin().names().count()
    );

    let golden = repository().join("tests/golden/library-demo/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}
