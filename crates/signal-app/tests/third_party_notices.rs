//! ADR 0031 §2: the committed `THIRD-PARTY-NOTICES.md` is what
//! `scripts/third-party-notices.mjs` makes of Cargo.lock now, and every crate
//! the shipped binaries link is under an allowed licence. The script's
//! `--check` mode decides both; here it also fails on a stale file and on a
//! linked crate whose licence is outside the set. Without `node` on PATH it
//! fails where `CI` is set (the workflow provisions Node) and is skipped
//! elsewhere, with a `SKIPPED` line written past the harness's capture.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The script with `args`, run with this build's cargo; `None` without node.
fn notices(args: &[&std::ffi::OsStr]) -> Option<Output> {
    match Command::new("node")
        .arg(root().join("scripts/third-party-notices.mjs"))
        .args(args)
        .env("CARGO", env!("CARGO"))
        .current_dir(root())
        .output()
    {
        Ok(output) => Some(output),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let ci = std::env::var("CI").is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false"));
            assert!(
                !ci,
                "node is not on PATH, and CI is set (.github/workflows/ci.yml provisions Node)"
            );
            // Written to stderr itself, which the harness does not capture.
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED the licence check: node is not on PATH (outside CI only); install Node to run it"
            );
            None
        }
        Err(error) => panic!("node did not run: {error}"),
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_committed_notices_are_current() {
    let Some(output) = notices(&["--check".as_ref()]) else {
        return;
    };
    assert!(output.status.success(), "{}", text(&output));
}

#[test]
fn a_stale_file_fails_the_check() {
    let dir = scratch("notices-stale");
    let committed = fs::read_to_string(root().join("THIRD-PARTY-NOTICES.md")).unwrap();
    // As if Cargo.lock had moved on: one linked crate's row names another version.
    let row = committed
        .lines()
        .find(|l| l.starts_with("| serde_json | "))
        .expect("serde_json is linked");
    let stale = dir.join("THIRD-PARTY-NOTICES.md");
    fs::write(
        &stale,
        committed.replace(
            row,
            &row.replacen("| serde_json | ", "| serde_json | 0.", 1),
        ),
    )
    .unwrap();
    let Some(output) = notices(&["--check".as_ref(), "--out".as_ref(), stale.as_os_str()]) else {
        return;
    };
    assert!(!output.status.success());
    assert!(
        text(&output).contains("is stale against Cargo.lock"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_linked_crate_outside_the_allowed_licences_fails() {
    let dir = scratch("notices-licence");
    // The metadata the script reads, with the same arguments; the unchanged
    // copy must pass, so a failure below is the licence and nothing else.
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--offline", "--locked", "--format-version", "1"])
        .args(["--filter-platform", "aarch64-apple-darwin"])
        .args(["--features", "signal-cli/gpu"])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    let mut metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let unchanged = dir.join("unchanged.json");
    fs::write(&unchanged, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let Some(output) = notices(&[
        "--check".as_ref(),
        "--metadata".as_ref(),
        unchanged.as_os_str(),
    ]) else {
        return;
    };
    assert!(output.status.success(), "{}", text(&output));

    for licence in [
        "GPL-3.0-only",
        "MIT AND GPL-2.0-or-later",
        "LGPL-2.1 OR GPL-3.0",
    ] {
        let package = metadata["packages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["name"] == "serde_json")
            .unwrap();
        package["license"] = Value::from(licence);
        let changed = dir.join("changed.json");
        fs::write(&changed, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let output = notices(&[
            "--check".as_ref(),
            "--metadata".as_ref(),
            changed.as_os_str(),
        ])
        .unwrap();
        assert!(!output.status.success(), "{licence} was accepted");
        let said = text(&output);
        assert!(
            said.contains("licence outside ADR 0031's allowed set")
                && said.contains("  serde_json ")
                && said.contains(&format!(": {licence}\n")),
            "{said}"
        );
    }
}
