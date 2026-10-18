//! The caller-supplied fields of `manifest.json` (ADR 0008): where the Signal
//! source, the compiler and the project files came from. A source that cannot
//! be read gives `"unknown"`; it never fails the render. Used by `signal
//! render` and `signal replay`, and by anything that records a session
//! (`SessionRecorder::new` takes the build's provenance).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use signal_core::fnv1a64;
use signal_engine::Provenance;

use crate::SourceFile;

const UNKNOWN: &str = "unknown";

/// The Signal workspace this binary was built from.
pub fn source_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// The provenance of a render of `files`, with the Signal source at `source`.
pub fn collect(source: &Path, files: &[SourceFile]) -> Provenance {
    let git = |args: &[&str]| {
        let mut command = Command::new("git");
        command.arg("-C").arg(source).args(args);
        output(&mut command, source)
    };
    let rustc = output(Command::new("rustc").arg("-V"), source);
    Provenance {
        signal_revision: text_from(git(&["rev-parse", "HEAD"]).as_deref())
            .unwrap_or_else(|| UNKNOWN.to_owned()),
        signal_dirty: dirty_from(git(&["status", "--porcelain"]).as_deref()),
        rustc_version: text_from(rustc.as_deref()).unwrap_or_else(|| UNKNOWN.to_owned()),
        project_file_hashes: files
            .iter()
            .map(|file| (file.path.clone(), hex(fnv1a64(file.text.as_bytes()))))
            .collect(),
        // The caller's: what the project's plan uses (`Project::library_hashes`).
        library_composite_hashes: Default::default(),
        cargo_lock_hash: fs::read(source.join("Cargo.lock"))
            .map_or_else(|_| UNKNOWN.to_owned(), |bytes| hex(fnv1a64(&bytes))),
    }
}

/// The stdout of `command` run in `dir`, or `None` if it could not be started
/// or exited unsuccessfully.
fn output(command: &mut Command, dir: &Path) -> Option<Vec<u8>> {
    let output = command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// The dirty flag from `git status --porcelain`: `None` (unknown) when the
/// command failed, else whether it listed anything. Empty output from a
/// successful run means a clean tree.
fn dirty_from(status: Option<&[u8]>) -> Option<bool> {
    status.map(|stdout| !stdout.is_empty())
}

/// A one-line answer (a revision, a version): `None` (unknown) when the
/// command failed or printed nothing usable.
fn text_from(stdout: Option<&[u8]>) -> Option<String> {
    let text = std::str::from_utf8(stdout?).ok()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn hex(hash: u64) -> String {
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap_or_else(|e| panic!("git {args:?} could not start: {e}"));
        assert!(status.success(), "git {args:?} failed");
    }

    /// Every source that cannot be read is "unknown", and collecting succeeds.
    #[test]
    fn unavailable_sources_are_unknown() {
        let file = SourceFile {
            path: "project.toml".to_owned(),
            text: "x".to_owned(),
        };
        let provenance = collect(Path::new("/nonexistent/signal-provenance"), &[file]);
        assert_eq!(provenance.signal_revision, UNKNOWN);
        assert_eq!(provenance.signal_dirty, None);
        assert_eq!(provenance.rustc_version, UNKNOWN);
        assert_eq!(provenance.cargo_lock_hash, UNKNOWN);
        assert_eq!(
            provenance.project_file_hashes["project.toml"],
            hex(fnv1a64(b"x"))
        );
        let json = serde_json::to_value(&provenance).unwrap();
        assert_eq!(json["signal_dirty"], "unknown");
    }

    /// A clean status (success, empty output) is `false`, not unknown; only a
    /// failed command is unknown. Empty output means unknown only for text.
    #[test]
    fn classification_rules() {
        assert_eq!(dirty_from(Some(b"")), Some(false));
        assert_eq!(dirty_from(Some(b" M src/x.rs\n")), Some(true));
        assert_eq!(dirty_from(None), None);
        assert_eq!(text_from(Some(b"")), None);
        assert_eq!(text_from(Some(b"  \n")), None);
        assert_eq!(text_from(Some(b"abc\n")), Some("abc".to_owned()));
        assert_eq!(text_from(None), None);
    }

    /// In a real repository: clean after a commit, dirty with an untracked
    /// file. Needs `git`; fails (not skips) without it.
    #[test]
    fn clean_repository_is_not_dirty() {
        let dir = std::env::temp_dir().join(format!("signal-provenance-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]);
        fs::write(dir.join("Cargo.lock"), "lock").unwrap();
        run(&dir, &["add", "Cargo.lock"]);
        run(&dir, &["commit", "-q", "-m", "init"]);

        let clean = collect(&dir, &[]);
        assert_eq!(clean.signal_dirty, Some(false));
        assert_eq!(clean.signal_revision.len(), 40);
        assert!(clean.signal_revision.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(clean.cargo_lock_hash, hex(fnv1a64(b"lock")));
        let json = serde_json::to_value(&clean).unwrap();
        assert_eq!(json["signal_dirty"], false);

        fs::write(dir.join("untracked"), "").unwrap();
        assert_eq!(collect(&dir, &[]).signal_dirty, Some(true));
        fs::remove_dir_all(&dir).unwrap();
    }
}
