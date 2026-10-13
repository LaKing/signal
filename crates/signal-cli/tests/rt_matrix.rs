//! M2.7-1 AC3 (C13): `scripts/rt-matrix.sh --driver null` runs end to end
//! unattended and writes its CSV and Markdown table, every row labelled
//! sandboxed; and (C8) its default matrix is shown, from its defaults, to take
//! under 15 minutes. Shape only: no number here is a measurement.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/rt-matrix.sh")
}

fn run(args: &[&str]) -> Output {
    Command::new("bash")
        .arg(script())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("bash runs")
}

#[test]
fn the_default_matrix_takes_under_15_minutes() {
    let output = run(&["--estimate"]);
    assert!(output.status.success());
    let estimate = String::from_utf8_lossy(&output.stderr);
    assert!(
        estimate.contains("48 cells (4 policies × 3 thread counts × 2 voice counts × 2 budgets) × (10 s + 4 s overhead) = 672 s (11.2 min)"),
        "{estimate}"
    );
    assert!(estimate.contains("labelled device"), "{estimate}");
}

#[test]
fn the_null_matrix_runs_end_to_end_labelled_sandboxed() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("rt-matrix");
    let _ = fs::remove_dir_all(&out);
    let output = run(&[
        "--driver",
        "null",
        "--signal",
        env!("CARGO_BIN_EXE_signal"),
        "--policies",
        "park50,semaphore",
        "--threads",
        "2",
        "--voices",
        "4",
        "--computation",
        "0.5",
        "--seconds",
        "1",
        "--settle",
        "0",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let csv = fs::read_to_string(out.join("cells.csv")).unwrap();
    let mut lines = csv.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    for column in [
        "label",
        "policy",
        "xruns",
        "worst",
        "p99",
        "median",
        "wake_p99_us",
        "first_join_p99_us",
        "last_join_max_us",
        "slowest_task_p99_us",
        "demotion_samples",
        "status",
    ] {
        assert!(header.contains(&column), "{column} in {header:?}");
    }
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
    assert_eq!(rows.len(), 2, "{csv}");
    for (row, policy) in rows.iter().zip(["park50", "semaphore"]) {
        assert_eq!(row[0], "sandboxed");
        assert_eq!(row[1], policy);
        assert_eq!(*row.last().unwrap(), "ok", "{csv}");
        assert_eq!(
            row[28 - 1],
            if policy == "semaphore" {
                "semaphore"
            } else {
                "park"
            }
        );
        assert!(row[9].parse::<f64>().is_ok(), "worst in {row:?}");
    }
    let table = fs::read_to_string(out.join("table.md")).unwrap();
    assert!(table.contains("SANDBOXED (null driver)"), "{table}");
    assert!(table.contains("| park50 | 2 | 4 | 0.5 |"), "{table}");
    assert!(out.join("cell-semaphore-t2-v4-c0.5.json").exists());

    // An unknown policy stops it before anything runs.
    let bad = run(&["--policies", "spin", "--estimate"]);
    assert_eq!(bad.status.code(), Some(2));
}
