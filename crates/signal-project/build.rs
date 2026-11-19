//! The build's identity, embedded when this crate is built (the review of
//! 4d37bf8): the compiler that builds it, the Signal revision, whether the
//! checkout differed from it, and the hash of `Cargo.lock`, read then
//! (`provenance::build`). A binary run after the checkout moved on, or
//! with another `rustc` on the PATH, still reports what built it. A source
//! that cannot be read is "unknown"; it never fails the build.

use std::path::Path;
use std::process::{Command, Stdio};
use std::{env, fs};

fn main() {
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join("../..");
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let git = |args: &[&str]| {
        output(
            Command::new("git")
                .arg("--no-optional-locks")
                .arg("-C")
                .arg(&root)
                .args(args),
        )
    };
    let revision = git(&["rev-parse", "HEAD"]).filter(|r| !r.is_empty());
    // `None`: unknown; an empty status is a clean checkout.
    let dirty =
        git(&["status", "--porcelain"]).map(|s| if s.is_empty() { "false" } else { "true" });
    let compiler = output(Command::new(rustc).arg("-V")).filter(|v| !v.is_empty());
    let lock = fs::read(root.join("Cargo.lock"))
        .ok()
        .map(|bytes| format!("{:016x}", fnv1a64(&bytes)));
    for (key, value) in [
        ("SIGNAL_BUILD_REVISION", revision.as_deref()),
        ("SIGNAL_BUILD_DIRTY", dirty),
        ("SIGNAL_BUILD_RUSTC", compiler.as_deref()),
        ("SIGNAL_BUILD_LOCK", lock.as_deref()),
    ] {
        println!("cargo:rustc-env={key}={}", value.unwrap_or("unknown"));
    }
    // Read again when what they come from changes: the revision, the
    // index and the sources a binary is built from, the lock file, the
    // toolchain.
    for path in [
        ".git/HEAD",
        ".git/index",
        ".git/refs",
        ".git/packed-refs",
        "Cargo.lock",
        "Cargo.toml",
        "rust-toolchain.toml",
        "crates",
        "lab",
    ] {
        // Only what exists: a path that does not (`.git` a file in a
        // worktree, no packed refs) would run this again on every build.
        let path = root.join(path);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    println!("cargo:rerun-if-env-changed=RUSTC");
}

/// The trimmed stdout of `command`, or `None` if it could not start or
/// failed.
fn output(command: &mut Command) -> Option<String> {
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// FNV-1a 64, as `signal_core::fnv1a64`.
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
