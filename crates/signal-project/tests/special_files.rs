//! Review 2026-11-01 (special-file-project-toml): a project's file that is
//! not a regular file, a FIFO (whose open blocks without a writer) or a
//! link to `/dev/zero` (which reads without end), is refused with a
//! diagnostic, at once: `read_regular`, `read_sources` (so every load of
//! the project) and `transaction::current` (every edit of a file, the
//! window layout's included) each complete within a bound.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use signal_project::{LoadError, read_regular, read_sources, transaction};

/// How long any of these may take.
const BOUND: Duration = Duration::from_secs(5);

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "signal-project-special-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `path` made a FIFO; false (the case skipped, with a message) where
/// `mkfifo` is missing.
fn fifo(path: &Path) -> bool {
    let made = Command::new("mkfifo")
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("no mkfifo: the FIFO case is skipped");
    }
    made
}

/// `run` on another thread, its answer within [`BOUND`].
fn bounded<T: Send + 'static>(what: &str, run: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(run());
    });
    answer
        .recv_timeout(BOUND)
        .unwrap_or_else(|_| panic!("{what} did not complete within {BOUND:?}"))
}

#[test]
fn a_special_file_is_refused_at_once() {
    let dir = folder("read");
    fs::write(dir.join("plain.toml"), "a = 1\n").unwrap();
    assert_eq!(read_regular(dir.join("plain.toml")).unwrap(), b"a = 1\n");
    // A link to a regular file is followed, as before.
    std::os::unix::fs::symlink(dir.join("plain.toml"), dir.join("linked.toml")).unwrap();
    assert_eq!(read_regular(dir.join("linked.toml")).unwrap(), b"a = 1\n");
    assert_eq!(
        read_regular(dir.join("missing.toml")).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        read_regular(&dir).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    std::os::unix::fs::symlink("/dev/zero", dir.join("zero.toml")).unwrap();
    let zero = dir.join("zero.toml");
    let error = bounded("a read of /dev/zero", move || read_regular(zero)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    assert!(error.to_string().contains("not a regular file"), "{error}");
    let pipe = dir.join("fifo.toml");
    if fifo(&pipe) {
        let error = bounded("a read of a FIFO", move || read_regular(pipe)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        let pipe = dir.join("fifo.toml");
        let error = bounded("transaction::current of a FIFO", move || {
            transaction::current(&pipe)
        })
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A project whose `project.toml` is a FIFO or a link to `/dev/zero` does
/// not load, and says why, at once.
#[test]
fn a_special_project_toml_does_not_load_and_never_blocks() {
    for (case, make) in [
        (
            "zero",
            (|p: &Path| {
                std::os::unix::fs::symlink("/dev/zero", p).unwrap();
                true
            }) as fn(&Path) -> bool,
        ),
        ("fifo", fifo),
    ] {
        let dir = folder(case);
        if !make(&dir.join("project.toml")) {
            continue;
        }
        let at = dir.clone();
        match bounded(&format!("loading a {case} project.toml"), move || {
            read_sources(&at)
        }) {
            Err(LoadError::Io { path, error }) => {
                assert_eq!(path, dir.join("project.toml"));
                assert_eq!(error.kind(), ErrorKind::InvalidInput, "{case}");
            }
            Err(other) => panic!("{case}: {other}"),
            Ok(_) => panic!("{case}: read"),
        }
        let at = dir.clone();
        let loaded = bounded(
            &format!("Project::load of a {case} project.toml"),
            move || signal_project::Project::load(&at).is_err(),
        );
        assert!(loaded, "{case}: a special project.toml must not load");
        let _ = fs::remove_dir_all(&dir);
    }
}
