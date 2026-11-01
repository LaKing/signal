//! M2.7-9 (the re-review: CI ran default features only): the CI workflow has
//! a `lab` job, beside the default ones, that runs clippy and the tests with
//! `--features signal-cli/lab` on a target directory of its own. The seat
//! runs the same two command lines. Every job that runs the workspace's
//! tests provisions Node with a pinned major, since with `CI` set a missing
//! `node` fails the tests that run it.

use std::fs;
use std::path::Path;

/// The lab job's commands, exactly.
const CLIPPY: &str =
    "cargo clippy --workspace --all-targets --locked --features signal-cli/lab -- -D warnings";
const TESTS: &str =
    "cargo nextest run --workspace --locked --features signal-cli/lab --no-tests=pass";

/// The job named `name`: its lines, up to the next job.
fn job<'a>(workflow: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let header = format!("  {name}:");
    let mut lines = workflow.lines().skip_while(|l| *l != header);
    lines.next()?;
    Some(
        lines
            .take_while(|l| l.is_empty() || l.starts_with("    ") || l.starts_with("  #"))
            .collect(),
    )
}

#[test]
fn ci_runs_clippy_and_the_tests_with_the_lab_feature_on_its_own_target() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workflow = fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    for default in ["check", "release-tests", "model-tests"] {
        assert!(job(&workflow, default).is_some(), "the {default} job stays");
    }
    let lab = job(&workflow, "lab").expect("a `lab` job");
    let runs: Vec<&str> = lab
        .iter()
        .filter_map(|l| l.trim_start().strip_prefix("run: "))
        .collect();
    assert!(runs.contains(&CLIPPY), "{runs:#?}");
    assert!(runs.contains(&TESTS), "{runs:#?}");
    let target = lab
        .iter()
        .find_map(|l| l.trim_start().strip_prefix("CARGO_TARGET_DIR: "))
        .expect("the lab job sets its own CARGO_TARGET_DIR");
    let config = fs::read_to_string(root.join(".cargo/config.toml")).unwrap();
    assert!(
        !config.contains(&format!("\"{target}\"")),
        "the lab target directory {target} is not the default one"
    );
}

#[test]
fn every_test_job_provisions_a_pinned_node() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workflow = fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    for name in ["check", "release-tests", "lab"] {
        let lines = job(&workflow, name).unwrap_or_else(|| panic!("the {name} job"));
        let at = lines
            .iter()
            .position(|l| l.trim() == "- uses: actions/setup-node@v4")
            .unwrap_or_else(|| panic!("the {name} job sets up Node: {lines:#?}"));
        let version = lines[at..]
            .iter()
            .find_map(|l| l.trim().strip_prefix("node-version: "))
            .unwrap_or_else(|| panic!("the {name} job pins Node's version"));
        let major = version.trim_matches('\'');
        assert!(
            !major.is_empty() && major.chars().all(|c| c.is_ascii_digit()),
            "the {name} job pins a Node major, not {version}"
        );
        let tests = lines
            .iter()
            .position(|l| l.contains("cargo nextest run"))
            .unwrap_or_else(|| panic!("the {name} job runs the tests"));
        assert!(at < tests, "the {name} job sets up Node before its tests");
    }
}
