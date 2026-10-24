//! Several writers on one project folder (ADR 0040): regressions of the
//! external audit of 2026-10-23, each a fixed interleaving, never a race
//! left to timing.

use std::fs;
use std::path::PathBuf;

use signal_project::transaction::{ProjectLock, stage, write_atomic};
use signal_project::vendor::{self, VendorError};
use signal_project::{Library, archive};

fn folder(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("signal-transaction-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// audit 2026-10-23 (writer): writer 2 starts while writer 1's bytes are
/// staged and not yet committed, and is paused after writing its own first
/// byte. Writer 1 commits exactly its own complete bytes (before the fix it
/// returned `Ok(())` having committed writer 2's one-byte file); writer 2's
/// staging file is neither removed nor renamed by writer 1, and later
/// commits its own bytes.
#[test]
fn a_writer_commits_only_its_own_staged_bytes() {
    let dir = folder("two-writers");
    let path = dir.join("main.html");
    fs::write(&path, "old").unwrap();
    let a = "A".repeat(1 << 20);
    let first = stage(&path, a.as_bytes()).unwrap();
    let second = stage(&path, b"B").unwrap();
    assert_ne!(first.path(), second.path());
    let second_path = second.path().to_owned();
    first.replace(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), a.as_bytes());
    assert_eq!(
        fs::read(&second_path).unwrap(),
        b"B",
        "writer 2's file intact"
    );
    second.replace(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"B");
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        1,
        "no staging file left"
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// A staging file another writer has open under the old shared name
/// (`.<name>.tmp`, a process of an earlier build), or under any other
/// name, is never removed, renamed or written through: the write takes a
/// name of its own.
#[test]
fn another_writers_staging_file_is_left_alone() {
    let dir = folder("foreign");
    let path = dir.join("project.toml");
    fs::write(&path, "old").unwrap();
    let foreign = dir.join(".project.toml.tmp");
    fs::write(&foreign, "B").unwrap();
    write_atomic(&path, b"new, in full").unwrap();
    archive::write_atomic(&path, b"newer, in full").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"newer, in full");
    assert_eq!(fs::read(&foreign).unwrap(), b"B");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
    fs::remove_dir_all(&dir).unwrap();
}

const PROJECT: &str = "schema_version = 1\n[instances]\nclock = { module = \"lib.transport\" }\nout = { module = \"std.audio_out\", gain = 0.25 }\n";

/// audit 2026-10-23 (vendor): vendoring is prepared, the output gain is
/// edited from 0.25 to 0.5, then the prepared vendoring is committed. It is
/// refused as a conflict naming `project.toml`, and nothing is written: the
/// edit stays and no copy is made (before the fix it succeeded and wrote
/// 0.25 back).
#[test]
fn vendoring_refuses_an_edit_made_after_it_was_prepared() {
    let dir = folder("vendor-conflict");
    let path = dir.join("project.toml");
    fs::write(&path, PROJECT).unwrap();
    let prepared = vendor::prepare(&dir, "lib.transport", None, Library::builtin()).unwrap();
    let edited = PROJECT.replace("gain = 0.25", "gain = 0.5");
    fs::write(&path, &edited).unwrap();
    let error = vendor::write(&dir, prepared, Library::builtin()).unwrap_err();
    assert!(
        matches!(&error, VendorError::Changed(file) if file == "project.toml"),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("changed after vendoring was prepared")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
    assert!(
        !dir.join("composites").exists()
            || fs::read_dir(dir.join("composites")).unwrap().count() == 0
    );
    // Prepared again from the files as they are now, it is written, the
    // edit kept.
    let vendored = vendor::vendor(&dir, "lib.transport", None, Library::builtin()).unwrap();
    assert!(
        vendored
            .copied
            .contains(&"composites/transport.toml".to_owned())
    );
    let after = fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("gain = 0.5") && after.contains("module = \"transport\""),
        "{after}"
    );
    let hidden = fs::read_dir(&dir)
        .unwrap()
        .chain(fs::read_dir(dir.join("composites")).unwrap())
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')
        })
        .count();
    assert_eq!(hidden, 0, "no staging file left");
    fs::remove_dir_all(&dir).unwrap();
}

/// A file added to the project after preparation (here a composite of the
/// name a copy takes, made by another writer) is a conflict too, and it is
/// left as it is.
#[test]
fn vendoring_refuses_a_file_added_after_it_was_prepared() {
    let dir = folder("vendor-added");
    fs::write(dir.join("project.toml"), PROJECT).unwrap();
    let prepared = vendor::prepare(&dir, "lib.transport", None, Library::builtin()).unwrap();
    fs::create_dir_all(dir.join("composites")).unwrap();
    let theirs = dir.join("composites/transport.toml");
    fs::write(&theirs, "# someone else's\n").unwrap();
    let error = vendor::write(&dir, prepared, Library::builtin()).unwrap_err();
    assert!(matches!(error, VendorError::Changed(_)), "{error:?}");
    assert_eq!(fs::read_to_string(&theirs).unwrap(), "# someone else's\n");
    assert_eq!(
        fs::read_to_string(dir.join("project.toml")).unwrap(),
        PROJECT
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// Vendoring holds the project's writer lock while it commits: it releases
/// it when done, so the next writer takes it at once.
#[test]
fn vendoring_releases_the_project_lock() {
    let dir = folder("vendor-lock");
    fs::write(dir.join("project.toml"), PROJECT).unwrap();
    vendor::vendor(&dir, "lib.transport", None, Library::builtin()).unwrap();
    assert!(ProjectLock::try_acquire(&dir).unwrap().is_some());
    fs::remove_dir_all(&dir).unwrap();
}
