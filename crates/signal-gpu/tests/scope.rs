//! M2.7-2, AC5 and AC2b: no process-wide GPU service remains in this crate,
//! and only the crates allowed to link wgpu depend on it in a default build.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// AC5: the only `static` of the crate's sources is the channel id counter;
/// nothing lazily made or thread-local holds a service or a pool.
#[test]
fn no_global_service_instance() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut statics = Vec::new();
    for entry in fs::read_dir(&src).unwrap().flatten() {
        let path = entry.path();
        let text = fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            let words = [
                "static ",
                "OnceLock",
                "LazyLock",
                "thread_local!",
                "lazy_static",
            ];
            if words.iter().any(|w| code.contains(w)) && !code.contains("'static") {
                statics.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert_eq!(statics.len(), 1, "{statics:#?}");
    assert!(
        statics[0].contains("static NEXT_CHANNEL: AtomicU64"),
        "{statics:#?}"
    );
}

/// The `[dependencies]` lines of `manifest` that name `name`, and whether
/// each is optional.
fn dependencies_on(manifest: &Path, name: &str) -> Vec<bool> {
    let text = fs::read_to_string(manifest).unwrap();
    let mut section = "";
    let mut found = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line;
            continue;
        }
        if section == "[dependencies]" && line.split([' ', '=']).next() == Some(name) {
            found.push(line.contains("optional = true"));
        }
    }
    found
}

/// AC2b (decision operator-request:007, option A): in a default build only
/// `signal-gpu` itself and `signal-app` link wgpu. `signal-gpu` is a required
/// dependency of `signal-app` only; elsewhere it is optional, behind a
/// feature no default turns on (`signal-cli`: `gpu`, `lab`; `signal-lab`:
/// `lab`).
#[test]
fn only_signal_gpu_and_signal_app_link_wgpu_by_default() {
    let root = root();
    let mut manifests: Vec<PathBuf> = fs::read_dir(root.join("crates"))
        .unwrap()
        .flatten()
        .map(|e| e.path().join("Cargo.toml"))
        .collect();
    manifests.push(root.join("lab/Cargo.toml"));
    let mut required = Vec::new();
    for manifest in &manifests {
        let krate = manifest
            .parent()
            .and_then(Path::file_name)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for dependency in ["signal-gpu", "wgpu", "eframe"] {
            for optional in dependencies_on(manifest, dependency) {
                if !optional {
                    required.push(format!("{krate} → {dependency}"));
                }
            }
        }
        let text = fs::read_to_string(manifest).unwrap();
        assert!(
            !text
                .lines()
                .any(|l| l.trim_start().starts_with("default =")),
            "{krate} declares default features"
        );
    }
    required.sort();
    assert_eq!(
        required,
        [
            "signal-app → eframe",
            "signal-app → signal-gpu",
            "signal-gpu → wgpu"
        ]
    );
}
