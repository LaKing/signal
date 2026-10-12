//! `signal bench` (M2-5 AC1): a short run prints every field. Fields and
//! shape only: the numbers are measurements of whatever machine runs the test
//! (a debug build here), so no timing is ever compared to a threshold.

use std::process::{Command, Stdio};

use serde_json::Value;
use signal_core::Registry;

fn number(value: &Value, what: &str) -> f64 {
    let number = value
        .as_f64()
        .unwrap_or_else(|| panic!("{what} is not a number: {value}"));
    assert!(number.is_finite(), "{what} is not finite");
    number
}

/// `{median_ns, p99_ns, repeats}`.
fn timing(value: &Value, what: &str) {
    number(&value["median_ns"], &format!("{what}.median_ns"));
    number(&value["p99_ns"], &format!("{what}.p99_ns"));
    assert!(
        value["repeats"].as_u64().is_some_and(|n| n >= 5),
        "{what}.repeats: {value}"
    );
}

#[test]
fn bench_json_has_every_field() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["--json", "bench", "--seconds", "0.1"])
        .stdin(Stdio::null())
        .output()
        .expect("the signal binary runs");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let bench: Value = serde_json::from_slice(&output.stdout).expect("JSON on stdout");

    assert_eq!(bench["label"], "measurement");
    assert!(["debug", "release"].contains(&bench["profile"].as_str().unwrap()));
    for key in [
        "model",
        "cpu",
        "cores",
        "performance_cores",
        "memory_bytes",
        "os",
    ] {
        let value = &bench["machine"][key];
        assert!(value.is_string() || value.is_null(), "machine.{key}");
    }
    assert_eq!(bench["sample_rate"], 96_000);
    let budget = number(&bench["budget_ns"], "budget_ns");
    assert!((budget - 1e9 / 96_000.0).abs() < 1e-9);
    assert_eq!(bench["seconds_per_case"], 0.1);
    assert_eq!(bench["frames_per_repeat"], 1024);
    timing(&bench["engine_empty"], "engine_empty");
    timing(&bench["stimulus"], "stimulus");

    // Every registered std.* primitive, in registry order, once.
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let expected: Vec<&str> = registry
        .iter()
        .map(|t| t.descriptor.type_name)
        .filter(|name| name.starts_with("std."))
        .collect();
    let primitives = bench["primitives"].as_array().unwrap();
    let names: Vec<&str> = primitives
        .iter()
        .map(|p| p["type"].as_str().unwrap())
        .collect();
    assert_eq!(names, expected);
    for name in ["std.osc", "std.svf", "std.delay", "std.step_seq"] {
        assert!(
            names.contains(&name),
            "the M2 primitive {name} is benchmarked"
        );
    }
    for primitive in primitives {
        let name = primitive["type"].as_str().unwrap();
        timing(primitive, name);
        timing(&primitive["graph"], &format!("{name}.graph"));
        assert_eq!(primitive["instances"], 16);
    }

    timing(&bench["kick"], "kick");
    number(&bench["kick"]["budget_fraction"], "kick.budget_fraction");

    let voices = &bench["kick_voices"];
    assert_eq!(voices["criterion"], "median");
    for (key, fraction) in [("within_25_percent", 0.25), ("within_50_percent", 0.5)] {
        let within = &voices[key];
        let limit = number(&within["limit_ns"], "limit_ns");
        assert!((limit - fraction * budget).abs() < 1e-6, "{key}.limit_ns");
        let n = within["voices"].as_u64().unwrap_or_else(|| panic!("{key}"));
        if n == 0 {
            assert!(within["timing"].is_null() && within["budget_fraction"].is_null());
        } else {
            timing(&within["timing"], key);
            number(&within["budget_fraction"], key);
        }
    }
    let measured = voices["measured"].as_array().unwrap();
    assert!(
        !measured.is_empty(),
        "at least one voice count was measured"
    );
    for entry in measured {
        assert!(entry["voices"].as_u64().is_some_and(|n| n >= 1));
        timing(entry, "kick_voices.measured");
    }
}

#[test]
fn bench_refuses_a_non_positive_duration() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["--json", "bench", "--seconds", "0"])
        .stdin(Stdio::null())
        .output()
        .expect("the signal binary runs");
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["kind"], "usage");
}
