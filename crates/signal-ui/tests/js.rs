//! The UI library's logic tests (`node --test ui/`), run from `cargo test`;
//! skipped, with a message, when `node` is not on PATH. Also checks that
//! `ui/index.js`, the entry `node --test ui/` runs, imports every test file.

use std::fs;
use std::io::ErrorKind;
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
            eprintln!("skipping the JS tests: node is not on PATH");
            return;
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
