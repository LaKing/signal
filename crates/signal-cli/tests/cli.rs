//! The built `signal` binary: rendering a fixture project, its artifacts, JSON
//! output, determinism, `modules`, `schema`, and exit codes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .expect("the signal binary runs")
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .display()
        .to_string()
}

/// A fresh output folder for one test, inside the target directory.
fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

/// Renders the sine fixture for 0.1 s into `out` with `--json`.
fn render_sine(out: &Path) -> Output {
    let out = out.display().to_string();
    signal(&[
        "render",
        &fixture("sine"),
        "--seconds",
        "0.1",
        "--out",
        &out,
        "--json",
    ])
}

#[test]
fn render_writes_the_four_artifacts() {
    let out = out_dir("cli-render");
    let output = render_sine(&out);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let summary = stdout_json(&output);
    assert_eq!(summary["frames"], 4800);
    assert_eq!(summary["sample_rate"], 48_000);
    assert_eq!(summary["channels"], 2);
    assert_eq!(summary["warnings"], Value::Array(Vec::new()));
    for file in ["audio.wav", "manifest.json", "log.jsonl", "metrics.json"] {
        assert!(out.join(file).is_file(), "{file} is written");
    }

    let wav = hound::WavReader::open(out.join("audio.wav")).unwrap();
    let spec = wav.spec();
    assert_eq!(spec.sample_rate, 48_000);
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.bits_per_sample, 32);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    assert_eq!(wav.duration(), 4800);

    let text = std::fs::read_to_string(out.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        keys(&manifest),
        BTreeSet::from([
            "provenance",
            "seed",
            "sample_rate",
            "frames",
            "debug_assertions",
            "log_fnv1a64",
            "audio_fnv1a64",
        ])
    );
    let provenance = &manifest["provenance"];
    assert_eq!(
        keys(provenance),
        BTreeSet::from([
            "signal_revision",
            "signal_dirty",
            "rustc_version",
            "project_file_hashes",
            "cargo_lock_hash",
        ])
    );
    assert_eq!(
        keys(&provenance["project_file_hashes"]),
        BTreeSet::from(["project.toml"])
    );
    assert_eq!(manifest["seed"], 1);
    assert_eq!(manifest["sample_rate"], 48_000);
    assert_eq!(manifest["frames"], 4800);
    assert_eq!(summary["manifest"], manifest);

    let metrics: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("metrics.json")).unwrap()).unwrap();
    assert_eq!(metrics["frames"], 4800);
    let channels = metrics["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 2);
    for channel in channels {
        assert_eq!(
            keys(channel),
            BTreeSet::from(["peak", "rms", "mean", "non_finite", "subnormal"])
        );
        // A 440 Hz sine at gain 0.5: peak about 0.5, nothing non-finite.
        let peak = channel["peak"].as_f64().unwrap();
        assert!((0.49..=0.5).contains(&peak), "peak {peak}");
        assert_eq!(channel["non_finite"], 0);
    }
    assert_eq!(summary["metrics"], metrics);
}

#[test]
fn rendering_twice_gives_identical_hashes() {
    let (a, b) = (out_dir("cli-twice-a"), out_dir("cli-twice-b"));
    let (first, second) = (stdout_json(&render_sine(&a)), stdout_json(&render_sine(&b)));
    for hash in ["audio_fnv1a64", "log_fnv1a64"] {
        assert_eq!(first["manifest"][hash], second["manifest"][hash], "{hash}");
    }
    assert_eq!(
        std::fs::read(a.join("audio.wav")).unwrap(),
        std::fs::read(b.join("audio.wav")).unwrap()
    );
}

#[test]
fn frames_and_overrides() {
    let out = out_dir("cli-frames");
    let output = signal(&[
        "render",
        &fixture("sine"),
        "--frames",
        "100",
        "--sample-rate",
        "44100",
        "--seed",
        "9",
        "--out",
        &out.display().to_string(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let manifest = &stdout_json(&output)["manifest"];
    assert_eq!(manifest["frames"], 100);
    assert_eq!(manifest["sample_rate"], 44_100);
    assert_eq!(manifest["seed"], 9);
}

#[test]
fn validation_warnings_are_printed() {
    let out = out_dir("cli-warning");
    let project = fixture("unused_instance");
    let output = signal(&[
        "render",
        &project,
        "--frames",
        "10",
        "--out",
        &out.display().to_string(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("project.toml:10:1: warning: /spare: no sink is reachable"),
        "{stderr}"
    );
}

#[test]
fn an_invalid_project_exits_1_with_its_location() {
    let out = out_dir("cli-invalid");
    let project = fixture("unknown_module");
    let out = out.display().to_string();
    let args = ["render", &project, "--seconds", "0.1", "--out", &out];

    let output = signal(&args);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("project.toml:9:18: error: /osc: unknown module `std.sine`"),
        "{stderr}"
    );
    assert!(!Path::new(&out).exists(), "nothing is written");

    let output = signal(&[&args[..], &["--json"]].concat());
    assert_eq!(output.status.code(), Some(1));
    let error = &stdout_json(&output)["error"];
    assert_eq!(error["kind"], "validation");
    let diagnostic = &error["diagnostics"][0];
    assert!(
        diagnostic["file"]
            .as_str()
            .unwrap()
            .ends_with("project.toml")
    );
    assert_eq!(
        (&diagnostic["line"], &diagnostic["column"]),
        (&9.into(), &18.into())
    );
    assert_eq!(diagnostic["severity"], "error");
}

#[test]
fn usage_errors_exit_1() {
    let sine = fixture("sine");
    for args in [
        vec!["render", sine.as_str(), "--seconds", "0.1"],
        vec![
            "render",
            sine.as_str(),
            "--seconds",
            "0.1",
            "--frames",
            "4",
            "--out",
            "x",
        ],
        vec!["render", sine.as_str(), "--seconds", "-1", "--out", "x"],
        vec!["frobnicate"],
    ] {
        let output = signal(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
    }
    let output = signal(&["render", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["kind"], "usage");
    assert_eq!(signal(&["--help"]).status.code(), Some(0));
}

#[test]
fn a_missing_project_exits_1() {
    let out = out_dir("cli-missing");
    let output = signal(&[
        "render",
        &fixture("no-such-project"),
        "--frames",
        "1",
        "--out",
        &out.display().to_string(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["kind"], "io");
}

#[test]
fn modules_lists_the_catalogue() {
    let output = signal(&["modules", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let modules = stdout_json(&output);
    let names: Vec<&str> = modules
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["type_name"].as_str().unwrap())
        .collect();
    for name in [
        "std.sine_osc",
        "std.audio_out",
        "std.osc",
        "std.svf",
        "std.onepole",
        "std.adsr",
        "std.lag",
    ] {
        assert!(names.contains(&name), "{name}");
    }

    let output = signal(&["modules"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    for name in [
        "std.sine_osc",
        "std.osc",
        "std.svf",
        "std.onepole",
        "std.adsr",
        "std.lag",
    ] {
        assert!(text.contains(&format!("{name} (direct)")), "{name}");
    }
}

#[test]
fn schema_output() {
    let output = signal(&["schema"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    insta::assert_snapshot!("schema", text);

    let output = signal(&["schema", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let schema = stdout_json(&output);
    assert_eq!(
        schema["$defs"]["ModuleRef"]["anyOf"][0]["enum"][0],
        "std.add"
    );
}
