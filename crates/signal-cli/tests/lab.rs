//! `signal` built with `--features lab` (ADR 0015, M2.6-4): `modules` lists
//! `lab.gpu_partials` with its declared latency and reproducibility class,
//! and `bench --gpu` reports the GPU partial bank per block next to its CPU
//! reference (AC4), per GPU device (M2.7-2, AC9). Renders record the device
//! set and the placement, and say which Setup limited the devices (AC7,
//! AC7b). Every run reads a Setup of the test's own (`SIGNAL_SETUP`), never
//! the developer's. Without the feature this file is empty, and the lab
//! types are absent (`cli.rs` and its snapshots cover that build).

#![cfg(feature = "lab")]

use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn signal(args: &[&str]) -> Output {
    signal_with_setup(args, "absent-setup.toml")
}

/// `signal` with the Setup `setup` (a file name in the test's scratch
/// directory; a missing one allows every device).
fn signal_with_setup(args: &[&str], setup: &str) -> Output {
    let setup = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(setup);
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .env("SIGNAL_SETUP", setup)
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
    // Per device (AC9): measured, or `no_gpu` says why not (its channel
    // failed); with no device at all, the top-level `no_gpu` says why.
    let devices = gpu["devices"].as_array().unwrap();
    if devices.is_empty() {
        assert!(gpu["no_gpu"].is_string(), "{gpu}");
    }
    for device in devices {
        assert!(
            device["id"].is_string() && device["adapter"].is_string(),
            "{device}"
        );
        match &device["per_block"]["gpu_round_trip"] {
            Value::Null => assert!(device["no_gpu"].is_string(), "{device}"),
            round_trip => timing(round_trip),
        }
    }
}

/// AC7, AC7b: a Setup whose `gpu_devices` allows no adapter leaves the bank
/// without a device: the render records it (`placement` with a null
/// device), its due results are lost and logged, and the summary says which
/// Setup limited the devices (auditor-2-005).
#[test]
fn a_setup_without_gpu_devices_is_recorded_and_said() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(
        dir.join("no-gpu-setup.toml"),
        "version = 1\ngpu_devices = []\n",
    )
    .unwrap();
    let out = dir.join(format!("signal-lab-no-gpu-{}", std::process::id()));
    let project = concat!(env!("CARGO_MANIFEST_DIR"), "/../../projects/gpu-partials");
    let output = signal_with_setup(
        &[
            "--json",
            "render",
            project,
            "--out",
            out.to_str().unwrap(),
            "--frames",
            "2048",
        ],
        "no-gpu-setup.toml",
    );
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    let manifest = &summary["manifest"];
    assert_eq!(manifest["placement"][0]["instance"], "/bank", "{manifest}");
    assert!(manifest["placement"][0]["device"].is_null(), "{manifest}");
    assert!(manifest.get("compute_devices").is_none(), "{manifest}");
    assert!(summary["late_results"].as_u64().is_some_and(|n| n > 0));
    assert_eq!(summary["gpu_setup"]["gpu_devices"], "[]", "{summary}");
    assert_eq!(summary["gpu_setup"]["devices"], "none", "{summary}");
    let _ = std::fs::remove_dir_all(&out);
}

/// AC3 in a GPU build: the compute devices, with the GPU part built.
#[test]
fn devices_compute_lists_the_gpu_adapters() {
    let output = signal(&["--json", "devices", "--compute"]);
    let devices: Value = serde_json::from_slice(&output.stdout).unwrap();
    let gpu = &devices["gpu"];
    assert_eq!(gpu["built"], true, "{devices}");
    let adapters = gpu["adapters"].as_array().unwrap();
    assert_eq!(gpu["available"], !adapters.is_empty());
    if adapters.is_empty() {
        eprintln!("skipped: no usable GPU adapter ({})", gpu["reason"]);
        assert!(gpu["reason"].is_string());
    }
    for adapter in adapters {
        for key in ["id", "name", "backend", "type"] {
            assert!(adapter[key].is_string(), "{key}: {adapter}");
        }
        assert!(adapter["limits"]["max_buffer_size"].is_u64(), "{adapter}");
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
    // AC7b: the placement is recorded, with or without a device.
    assert_eq!(summary["manifest"]["placement"][0]["instance"], "/bank");
    let log = std::fs::read_to_string(out.join("log.jsonl")).unwrap();
    let lost = log.matches("late_result").count() as u64;
    match summary.get("late_results") {
        Some(late) => assert_eq!(late.as_u64(), Some(lost)),
        None => assert_eq!(lost, 0),
    }
    let _ = std::fs::remove_dir_all(&out);
}
