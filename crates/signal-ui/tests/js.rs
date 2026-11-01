//! The UI library's logic tests (`node --test ui/`), run from `cargo test`.
//! Without `node` on PATH they fail where `CI` is set (the workflow
//! provisions Node) and are skipped elsewhere, with a `SKIPPED` line written
//! past the test harness's capture, so a skip never reads as a pass. Also checks that
//! `ui/index.js`, the entry `node --test ui/` runs, imports every test file.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn test_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            test_files(&path, found);
        } else if path.to_string_lossy().ends_with(".test.js") {
            found.push(path);
        }
    }
}

#[test]
fn the_test_entry_imports_every_test_file() {
    let ui = root().join("ui");
    let entry = fs::read_to_string(ui.join("index.js")).unwrap();
    let mut found = Vec::new();
    test_files(&ui, &mut found);
    assert!(!found.is_empty());
    for path in found {
        let relative = path
            .strip_prefix(&ui)
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(
            entry.contains(&format!("import './{relative}';")),
            "ui/index.js does not import {relative}"
        );
    }
}

/// Whether this is a CI run (`CI` set, and not empty, `0` or `false`).
fn in_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false"))
}

/// `node` is not on PATH: a failure in CI, else a skip said on the terminal
/// (written to stderr itself, which the harness does not capture).
fn without_node(what: &str) {
    assert!(
        !in_ci(),
        "{what}: node is not on PATH, and CI is set (.github/workflows/ci.yml provisions Node)"
    );
    let _ = writeln!(
        std::io::stderr(),
        "SKIPPED {what}: node is not on PATH (outside CI only); install Node to run it"
    );
}

#[test]
fn node_test_passes() {
    let output = match Command::new("node")
        .arg("--test")
        // TAP explicitly: newer Node versions default to the `spec` reporter,
        // which prints `ℹ fail 0` instead of `# fail 0`.
        .arg("--test-reporter=tap")
        .arg("ui/")
        .current_dir(root())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return without_node("node --test ui/");
        }
        Err(error) => panic!("node did not run: {error}"),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "node --test ui/ failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("# fail 0"), "{stdout}");
}
