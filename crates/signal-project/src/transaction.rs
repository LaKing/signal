//! Writing a project's files safely when several writers share a project
//! folder (ADR 0040): two Signal or `signal mcp` processes, `signal vendor`,
//! a person's editor.
//!
//! - [`stage`] writes bytes to a **staging file of the writer's own**:
//!   `.<name>.<pid>.<n>.tmp` beside the target (hidden, so the watch and
//!   [`crate::archive::pack`] ignore it), created exclusively (`O_EXCL`,
//!   which never follows a link), with `n` a counter of this process. An
//!   entry already at that name is left alone and the next name is taken.
//!   The [`Staged`] file then replaces its target by a rename
//!   ([`Staged::replace`]) or becomes a new file by a hard link, which never
//!   replaces one ([`Staged::create`]). Only that staging file is ever
//!   removed: a writer never deletes or renames another writer's file.
//! - [`write_atomic`] is the two together: a reader sees the old file or the
//!   new one, and a failure leaves the old one as it was.
//! - [`ProjectLock`] serialises the writers of one project folder: an
//!   advisory lock (`flock`) on the folder itself, so no lock file is left in
//!   the person's folder. A writer takes it before it checks that the files
//!   are still the version it read (the conflict check) and holds it until
//!   its files are written, so no other Signal writer commits in between. It
//!   is released when the guard drops, and by the system when a process
//!   ends. A person's editor does not take it: what it changes is caught by
//!   the conflict checks, which compare the files' bytes as they are now.
//!
//! The temporary names are not semantics (`AGENTS.md` rule 3): they never
//! reach a project's files, renders or logs.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long [`ProjectLock::acquire`] waits for another writer before it
/// gives up: far longer than any write of project text takes.
pub const LOCK_WAIT: Duration = Duration::from_secs(10);

/// A staging file of this writer's, holding the bytes to commit; removed
/// when dropped unless it was committed by [`Staged::replace`].
#[derive(Debug)]
pub struct Staged {
    path: PathBuf,
    committed: bool,
}

impl Staged {
    /// The staging file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Renames the staging file over `target`, at once.
    pub fn replace(mut self, target: &Path) -> io::Result<()> {
        fs::rename(&self.path, target)?;
        self.committed = true;
        Ok(())
    }

    /// Links the staging file to `target` as a new file: fails with
    /// [`io::ErrorKind::AlreadyExists`] when `target` exists (a link never
    /// replaces a file). The staging name is removed either way.
    pub fn create(self, target: &Path) -> io::Result<()> {
        fs::hard_link(&self.path, target)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.committed {
            // Only the file this writer created.
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// `bytes` in a new staging file of this writer's beside `target`, synced.
pub fn stage(target: &Path, bytes: &[u8]) -> io::Result<Staged> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let pid = std::process::id();
    let mut attempts = 0u32;
    let (path, mut file) = loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = target.with_file_name(format!(".{name}.{pid}.{n}.tmp"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            // Someone else's entry (left by an earlier process of this pid,
            // or planted): never touched, the next name is taken.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempts < 1000 => {
                attempts += 1;
            }
            Err(error) => return Err(error),
        }
    };
    let staged = Staged {
        path,
        committed: false,
    };
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(staged)
}

/// Writes `bytes` to `path` atomically: a staging file of this writer's
/// ([`stage`]), renamed over `path`. A failure leaves the old file as it was
/// and removes only the staging file made here.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    stage(path, bytes)?.replace(path)
}

/// The project folder's writer lock (see the module's notes), held while
/// the guard lives.
#[derive(Debug)]
pub struct ProjectLock {
    folder: File,
}

impl ProjectLock {
    /// Takes the lock of the project folder `dir`, waiting at most
    /// [`LOCK_WAIT`] for another writer to finish. A writer that already
    /// holds it must not take it again (it would wait for itself).
    pub fn acquire(dir: &Path) -> io::Result<ProjectLock> {
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match ProjectLock::try_acquire(dir)? {
                Some(lock) => return Ok(lock),
                None if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        format!(
                            "{}: another writer has held the project's lock for over {} s",
                            dir.display(),
                            LOCK_WAIT.as_secs()
                        ),
                    ));
                }
            }
        }
    }

    /// The lock of `dir` if no other writer holds it now, `None` otherwise.
    pub fn try_acquire(dir: &Path) -> io::Result<Option<ProjectLock>> {
        let folder = File::open(dir)?;
        match folder.try_lock() {
            Ok(()) => Ok(Some(ProjectLock { folder })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(error),
        }
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        // Released explicitly, not only by the close: a child process
        // forked meanwhile (and not yet exec'd) shares the descriptor and
        // would keep the lock until it execs.
        let _ = self.folder.unlock();
    }
}

/// The bytes of `path` now, `None` when it does not exist.
pub fn current(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("signal-transaction-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Two writers staging the same file never share or touch each other's
    /// staging file; each commits exactly its own bytes.
    #[test]
    fn writers_stage_apart() {
        let dir = folder("apart");
        let path = dir.join("main.html");
        fs::write(&path, "old").unwrap();
        let first = stage(&path, b"first, in full").unwrap();
        let second = stage(&path, b"B").unwrap();
        assert_ne!(first.path(), second.path());
        drop(second);
        assert!(first.path().exists());
        first.replace(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first, in full");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The lock excludes a second writer, in this process too, until the
    /// first one's guard drops.
    #[test]
    fn the_lock_excludes_a_second_writer() {
        let dir = folder("lock");
        let held = ProjectLock::acquire(&dir).unwrap();
        assert!(ProjectLock::try_acquire(&dir).unwrap().is_none());
        drop(held);
        assert!(ProjectLock::try_acquire(&dir).unwrap().is_some());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "no lock file");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A new file is created by a link only where none is.
    #[test]
    fn create_never_replaces() {
        let dir = folder("create");
        let path = dir.join("a.toml");
        stage(&path, b"one").unwrap().create(&path).unwrap();
        let again = stage(&path, b"two").unwrap().create(&path).unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"one");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
