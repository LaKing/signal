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

/// M2.6-1 AC5: the kick and the kick voices (the count within 25 % of the
/// budget) are timed in blocks of k = 1, 64 and 256 frames, in ns per frame.
#[test]
fn bench_json_reports_block_sizes() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["--json", "bench", "--seconds", "0.1"])
        .stdin(Stdio::null())
        .output()
        .expect("the signal binary runs");
    assert!(output.status.success());
    let bench: Value = serde_json::from_slice(&output.stdout).expect("JSON on stdout");
    let blocks = &bench["blocks"];
    let within = bench["kick_voices"]["within_25_percent"]["voices"]
        .as_u64()
        .unwrap();
    assert_eq!(blocks["voices"].as_u64(), Some(within.max(1)));
    let measured = blocks["measured"].as_array().unwrap();
    let sizes: Vec<u64> = measured.iter().map(|m| m["k"].as_u64().unwrap()).collect();
    assert_eq!(sizes, [1, 64, 256]);
    let cores = blocks["threads"].as_u64().unwrap();
    assert!(cores >= 1);
    for entry in measured {
        timing(&entry["kick"], "blocks.kick");
        timing(&entry["kick_voices"], "blocks.kick_voices");
        let all = &entry["on_performance_cores"];
        assert_eq!(all["threads"].as_u64(), Some(cores));
        timing(&all["kick"], "blocks.on_performance_cores.kick");
        timing(
            &all["kick_voices"],
            "blocks.on_performance_cores.kick_voices",
        );
    }
}

/// M2.6-2 AC4 and AC14: the scaling table (kick voices within 25 % and 50 %
/// of the budget at k = 128 on 1, 2, 4, 8 and all performance cores) and the
/// small-plan threshold rows are there, with their shape.
#[test]
fn bench_json_reports_scaling_and_the_threshold() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["--json", "bench", "--seconds", "0.1"])
        .stdin(Stdio::null())
        .output()
        .expect("the signal binary runs");
    assert!(output.status.success());
    let bench: Value = serde_json::from_slice(&output.stdout).expect("JSON on stdout");
    let scaling = &bench["scaling"];
    assert_eq!(scaling["block"], 128);
    let cores = scaling["performance_cores"].as_u64().unwrap();
    assert!(["sysctl", "fallback"].contains(&scaling["cores_source"].as_str().unwrap()));
    let rows = scaling["rows"].as_array().unwrap();
    let threads: Vec<u64> = rows
        .iter()
        .map(|r| r["threads"].as_u64().unwrap())
        .collect();
    let mut expected = vec![1, 2, 4, 8, cores];
    expected.sort_unstable();
    expected.dedup();
    assert_eq!(threads, expected);
    for row in rows {
        for key in ["within_25_percent", "within_50_percent"] {
            let within = &row[key];
            if within["voices"].as_u64().unwrap() > 0 {
                timing(&within["timing"], key);
            }
        }
        assert!(!row["measured"].as_array().unwrap().is_empty());
    }

    let threshold = &bench["parallel_threshold"];
    assert_eq!(threshold["threads"], 2);
    assert!(threshold["crossover_ns"].is_u64() || threshold["crossover_ns"].is_null());
    let rows = threshold["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 5 * 4);
    for row in rows {
        assert!(row["tasks"].as_u64().unwrap() >= 1);
        assert!(row["units"].as_u64() >= row["tasks"].as_u64());
        assert!(row["estimate_ns"].as_u64().unwrap() > 0);
        timing(&row["one_thread"], "one_thread");
        timing(&row["two_threads"], "two_threads");
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

fn signal(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("the signal binary runs")
}

/// M2.6-3: `signal bench --write-voices <N> <DIR>` writes a project that
/// loads and plans, the kick's `drums` duplicated N times into a tree of
/// `std.mix`, with the `accent` line `scripts/soak.sh` edits; a directory
/// that is not empty, and N = 0, are refused.
#[test]
fn write_voices_writes_a_valid_many_voice_project() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-voices");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.display().to_string();
    let output = signal(&["--json", "bench", "--write-voices", "5", &path]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["voices"], 5);

    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let project = signal_project::Project::load(&dir).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    assert_eq!(
        json["instances"].as_u64(),
        Some(plan.instances.len() as u64)
    );
    let count = |module: &str| {
        project
            .root
            .instances
            .iter()
            .filter(|i| i.module == module)
            .count()
    };
    assert_eq!(
        (count("drums"), count("std.mix")),
        (5, 3),
        "5 voices, mixed by 2 then 1"
    );
    assert_eq!((project.sample_rate, project.seed), (96_000, 42));
    let text = std::fs::read_to_string(dir.join("project.toml")).unwrap();
    let accent = r#"accent = { module = "std.exp_env", decay_ms = "#;
    assert!(text.lines().any(|l| l.starts_with(accent)), "{text}");

    let again = signal(&["--json", "bench", "--write-voices", "5", &path]);
    assert_eq!(again.status.code(), Some(1), "not empty");
    let zero = signal(&["--json", "bench", "--write-voices", "0", &path]);
    assert_eq!(zero.status.code(), Some(1), "no voices");
}

/// M2.6-3: `signal bench --realtime` prints a row per thread count (1, 8,
/// the real-time auto count and all of them) and variant, with its
/// per-block timing and each worker's path. Shape only.
#[test]
fn realtime_json_has_every_field() {
    let output = signal(&[
        "--json",
        "bench",
        "--realtime",
        "--rt-voices",
        "2",
        "--seconds",
        "0.01",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bench: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(bench["table"], "realtime_workers");
    assert_eq!(bench["block_frames"], 128);
    assert!(bench["blocks_per_row"].as_u64().unwrap() >= 100);
    number(&bench["policy"]["period_ticks"], "policy.period_ticks");
    let cores = bench["performance_cores"].as_u64().unwrap();
    let mut expected = vec![1, 8, (cores * 2 / 3).max(1), cores];
    expected.sort_unstable();
    expected.dedup();
    let rows = bench["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3 * expected.len());
    for (row, threads) in rows.iter().zip(expected.iter().flat_map(|&t| [t, t, t])) {
        assert_eq!(row["threads"].as_u64(), Some(threads));
        assert_eq!(row["workers"].as_u64(), Some(threads - 1));
        for key in ["median_ns", "p99_ns", "max_ns", "median_fraction"] {
            number(&row[key], key);
        }
        let paths = row["paths"].as_array().unwrap();
        let caller = row["caller"].as_str().unwrap();
        match row["variant"].as_str() {
            Some("ordinary") => assert!(paths.is_empty() && caller == "ordinary"),
            Some(variant @ ("rt-workers" | "rt-all")) => {
                assert_eq!(caller == "ordinary", variant == "rt-workers", "{caller}");
                assert_eq!(paths.len() as u64, threads - 1);
                for path in paths {
                    let path = path.as_str().unwrap();
                    assert!(
                        path.starts_with("time-constraint") || path.starts_with("none"),
                        "{path}"
                    );
                }
            }
            other => panic!("variant {other:?}"),
        }
    }
}
