//! M2.7-3 (the audit's note on fault-injection hooks): the engine's task
//! hook, its test-only instrumentation and every injected fault exist only
//! with the cargo feature `test-hooks`, which only dev-dependencies turn on,
//! so no production build can reach them. Every `.rs` file and manifest
//! under `crates/` and `lab/` is read; comment lines are skipped, so prose
//! may name the items.

use std::fs;
use std::path::{Path, PathBuf};

/// Each test-only item, by its file (relative to the repository root) and
/// the text of its definition or re-export.
const GATED: &[(&str, &str)] = &[
    ("crates/signal-engine/src/shared.rs", "pub enum HookPoint"),
    (
        "crates/signal-engine/src/shared.rs",
        "pub struct Exclusivity",
    ),
    ("crates/signal-engine/src/shared.rs", "pub fn exclusivity()"),
    ("crates/signal-engine/src/engine.rs", "pub fn set_task_hook"),
    ("crates/signal-engine/src/engine.rs", "pub fn inject"),
    ("crates/signal-engine/src/engine.rs", "pub enum Injection"),
    (
        "crates/signal-engine/src/realtime.rs",
        "pub fn set_task_hook",
    ),
    ("crates/signal-engine/src/pool.rs", "pub fn spawned_workers"),
    (
        "crates/signal-engine/src/lib.rs",
        "pub use shared::{Exclusivity, HookPoint, exclusivity};",
    ),
    (
        "crates/signal-engine/src/lib.rs",
        "pub use pool::spawned_workers;",
    ),
    (
        "crates/signal-engine/src/lib.rs",
        "pub use engine::Injection;",
    ),
    (
        "crates/signal-gpu/src/lib.rs",
        "pub fn set_completion_delay",
    ),
    // M2.7-5: the device service's hooks and test-only accessors.
    ("crates/signal-gpu/src/lib.rs", "pub fn set_execution_delay"),
    ("crates/signal-gpu/src/lib.rs", "pub fn last_dispatch"),
    ("crates/signal-gpu/src/lib.rs", "pub enum Injected"),
    ("crates/signal-gpu/src/lib.rs", "pub fn fail_next_job"),
    ("crates/signal-gpu/src/lib.rs", "pub fn lose_device"),
    ("crates/signal-gpu/src/lib.rs", "pub fn peak_in_flight"),
    ("crates/signal-gpu/src/lib.rs", "pub fn kernels_cached"),
];

/// Names no production source outside the two crates that define them may use.
const NAMES: [&str; 12] = [
    "HookPoint",
    "set_task_hook",
    "exclusivity(",
    "spawned_workers",
    ".inject(",
    "set_completion_delay",
    "set_execution_delay",
    "last_dispatch",
    "fail_next_job",
    "lose_device",
    "peak_in_flight",
    "kernels_cached",
];

const FEATURE: &str = "feature = \"test-hooks\"";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files(dir: &Path, found: &mut Vec<PathBuf>, keep: &dyn Fn(&Path) -> bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                files(&path, found, keep);
            }
        } else if keep(&path) {
            found.push(path);
        }
    }
}

fn is_comment(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')
}

/// Every test-only item is directly preceded (among its attributes and doc
/// comment) by a `cfg` naming the feature.
#[test]
fn every_test_only_item_needs_the_feature() {
    let mut missing = Vec::new();
    for (file, item) in GATED {
        let text = fs::read_to_string(root().join(file)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let Some(at) = lines.iter().position(|l| l.trim_start().starts_with(item)) else {
            missing.push(format!("{file}: `{item}` not found"));
            continue;
        };
        let gated = lines[..at]
            .iter()
            .rev()
            .take_while(|l| {
                let l = l.trim_start();
                l.starts_with("#[") || l.starts_with("///")
            })
            .any(|l| l.contains("cfg") && l.contains(FEATURE));
        if !gated {
            missing.push(format!("{file}: `{item}` is not behind `{FEATURE}`"));
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
}

/// No production source of another crate names a test-only item (the
/// tests, benches and examples may).
#[test]
fn no_production_source_uses_them() {
    let root = root();
    let mut sources = Vec::new();
    let rust = |p: &Path| p.extension().is_some_and(|e| e == "rs");
    for crate_dir in fs::read_dir(root.join("crates")).unwrap().flatten() {
        let name = crate_dir.file_name();
        if name == "signal-engine" || name == "signal-gpu" {
            continue;
        }
        files(&crate_dir.path().join("src"), &mut sources, &rust);
    }
    files(&root.join("lab/src"), &mut sources, &rust);
    assert!(sources.len() > 20, "the walk found {} files", sources.len());
    let mut offenders = Vec::new();
    for file in &sources {
        let text = fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            if !is_comment(line) && NAMES.iter().any(|name| line.contains(name)) {
                offenders.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

/// Only a crate's own `test-hooks` feature definition and its
/// `[dev-dependencies]` name the feature: no other feature forwards to it
/// and no normal dependency turns it on.
#[test]
fn only_dev_dependencies_turn_it_on() {
    let root = root();
    let mut manifests = Vec::new();
    let manifest = |p: &Path| p.file_name().is_some_and(|n| n == "Cargo.toml");
    files(&root.join("crates"), &mut manifests, &manifest);
    files(&root.join("lab"), &mut manifests, &manifest);
    manifests.push(root.join("Cargo.toml"));
    let mut dev = 0;
    let mut offenders = Vec::new();
    for file in &manifests {
        let text = fs::read_to_string(file).unwrap();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line.to_owned();
                continue;
            }
            if line.starts_with('#') || !line.contains("test-hooks") {
                continue;
            }
            match section.as_str() {
                "[dev-dependencies]" => dev += 1,
                "[features]" if line.starts_with("test-hooks = []") => {}
                _ => offenders.push(format!("{} {section}: {line}", file.display())),
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
    assert!(dev >= 4, "the dev-dependencies that turn it on: {dev}");
}
