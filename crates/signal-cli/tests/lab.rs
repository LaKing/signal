//! `signal` built with `--features lab` (ADR 0015, M2.6-4): `modules` lists
//! `lab.gpu_partials` with its declared latency and reproducibility class,
//! and `bench --gpu` reports the GPU partial bank per block next to its CPU
//! reference (AC4). Without the feature this file is empty, and the lab
//! types are absent (`cli.rs` and its snapshots cover that build).

#![cfg(feature = "lab")]

use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn signal(args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("the signal binary runs");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn modules_lists_the_gpu_partial_bank_with_its_latency() {
    let modules: Value = serde_json::from_slice(&signal(&["--json", "modules"]).stdout).unwrap();
    let bank = modules
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["type_name"] == "lab.gpu_partials")
        .expect("lab.gpu_partials is listed");
    assert_eq!(bank["feedthrough"], "state_output");
    assert_eq!(bank["latency"]["latency_blocks"], 2);
    assert_eq!(bank["latency"]["block_frames"], 256);
    assert_eq!(bank["reproducibility"], "same_machine");
    // A std type keeps its JSON: no latency, no reproducibility key.
    let sine = modules
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["type_name"] == "std.sine_osc")
        .unwrap();
    assert!(sine.get("latency").is_none() && sine.get("reproducibility").is_none());

    let text = String::from_utf8(signal(&["modules"]).stdout).unwrap();
    assert!(
        text.contains("lab.gpu_partials (state-output, 2 blocks of 256 frames late, same_machine)"),
        "{text}"
    );
}

/// Fields and shape only: the numbers are measurements of whatever machine
/// runs the test. Without a usable GPU the round trip is null and `no_gpu`
/// says why.
#[test]
fn bench_gpu_reports_the_gpu_bank_next_to_the_cpu_reference() {
    let output = signal(&["--json", "bench", "--gpu", "--seconds", "0.05"]);
    let bench: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(bench["label"], "measurement");
    let gpu = &bench["gpu_partials"];
    assert_eq!(gpu["partials"], 1024);
    assert_eq!(gpu["block_frames"], 256);
    assert_eq!(gpu["latency_blocks"], 2);
    let timing = |t: &Value| {
        assert!(t["median_ns"].as_f64().is_some_and(f64::is_finite), "{t}");
        assert!(t["p99_ns"].as_f64().is_some_and(f64::is_finite), "{t}");
        assert!(t["repeats"].as_u64().is_some_and(|n| n >= 5), "{t}");
    };
    timing(&gpu["per_block"]["cpu_reference"]);
    timing(&gpu["per_block"]["audio_side"]);
    // Measured, or `no_gpu` says why not (no adapter, or its channel failed).
    if gpu["per_block"]["gpu_round_trip"].is_null() {
        assert!(gpu["no_gpu"].is_string(), "{gpu}");
    } else {
        timing(&gpu["per_block"]["gpu_round_trip"]);
        assert!(
            gpu["adapter"].is_string() && gpu["no_gpu"].is_null(),
            "{gpu}"
        );
    }
}

/// Offline, a lost due result (no usable GPU) is said in the summary, not
/// only in `log.jsonl`; with a GPU nothing is lost and the key is absent.
#[test]
fn render_says_when_due_results_were_lost() {
    let out = std::env::temp_dir().join(format!("signal-lab-render-{}", std::process::id()));
    let project = concat!(env!("CARGO_MANIFEST_DIR"), "/../../projects/gpu-partials");
    let out_arg = out.to_str().unwrap();
    let output = signal(&[
        "--json", "render", project, "--out", out_arg, "--frames", "2048",
    ]);
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["manifest"]["same_machine"][0]["instance"], "/bank");
    let log = std::fs::read_to_string(out.join("log.jsonl")).unwrap();
    let lost = log.matches("late_result").count() as u64;
    match summary.get("late_results") {
        Some(late) => assert_eq!(late.as_u64(), Some(lost)),
        None => assert_eq!(lost, 0),
    }
    let _ = std::fs::remove_dir_all(&out);
}
