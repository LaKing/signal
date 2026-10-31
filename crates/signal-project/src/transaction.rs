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
//! - A thread that must never wait (the UI host's control thread, which
//!   owns the `Controller`) asks for the lock without waiting: it takes it
//!   with [`ProjectLock::try_hold`] before it runs a file request, so the
//!   writers it calls find it held by their own thread and go on at once,
//!   and it runs every other step under a [`NoWait`], under which
//!   [`ProjectLock::acquire`] tries once and fails at once rather than
//!   sleep. A request that finds the lock held waits in the host's queue,
//!   not on the thread (ADR 0048).
//!
//! The temporary names are not semantics (`AGENTS.md` rule 3): they never
//! reach a project's files, renders or logs.

use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
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
    /// replaces a file). On a file system without hard links (exFAT:
    /// `ENOTSUP`) the bytes are copied into `target` created exclusively
    /// instead, which never replaces a file either. The staging name is
    /// removed either way.
    pub fn create(self, target: &Path) -> io::Result<()> {
        self.create_with(target, |from, to| fs::hard_link(from, to))
    }

    /// [`Staged::create`] with `link` in place of [`fs::hard_link`].
    fn create_with(
        self,
        target: &Path,
        link: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> io::Result<()> {
        match link(&self.path, target) {
            Err(error) if unsupported(&error) => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(target)?;
                let copied = File::open(&self.path)
                    .and_then(|mut from| io::copy(&mut from, &mut file))
                    .and_then(|_| file.sync_all());
                if copied.is_err() {
                    // Only the file made here.
                    let _ = fs::remove_file(target);
                }
                copied
            }
            linked => linked,
        }
    }
}

/// Whether `error` says the file system cannot do this at all (no locks, no
/// hard links), rather than that it failed this time.
fn unsupported(error: &io::Error) -> bool {
    // ENOTSUP, EOPNOTSUPP, ENOLCK.
    #[cfg(target_os = "linux")]
    const CODES: [i32; 3] = [95, 95, 37];
    #[cfg(not(target_os = "linux"))]
    const CODES: [i32; 3] = [45, 102, 77];
    error.kind() == io::ErrorKind::Unsupported
        || error
            .raw_os_error()
            .is_some_and(|code| CODES.contains(&code))
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
    stage_numbered(target, bytes, || NEXT.fetch_add(1, Ordering::Relaxed))
}

/// [`stage`] with the staging names numbered by `next`.
fn stage_numbered(
    target: &Path,
    bytes: &[u8],
    mut next: impl FnMut() -> u64,
) -> io::Result<Staged> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let pid = std::process::id();
    let mut attempts = 0u32;
    let (path, mut file) = loop {
        let n = next();
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

/// Writes `bytes` to `path` atomically, as [`write_atomic`] does, only while
/// `path` still holds `before` (`None`: it does not exist): the file is read
/// again once the bytes are staged, just before the rename. `Ok(false)`: it
/// changed, and nothing was written. Under the [`ProjectLock`] this catches
/// a writer that takes no lock (an editor); without one (a volume without
/// locks) it also catches another Signal writer, except one renaming in the
/// instant between that read and this rename.
pub fn replace_unchanged(path: &Path, before: Option<&[u8]>, bytes: &[u8]) -> io::Result<bool> {
    let staged = stage(path, bytes)?;
    // Dropped unrenamed when it changed: only this writer's staging file goes.
    if current(path)?.as_deref() != before {
        return Ok(false);
    }
    staged.replace(path)?;
    Ok(true)
}

/// How often [`edit_text`] reads and edits a file again that changed
/// before its rename.
const EDIT_TRIES: usize = 3;

/// A read-modify-write of `path` for a writer that holds no `base` (the
/// schematic's positions, the window layout): `edit` is given the file's
/// text (empty when it does not exist) and returns the new text, or `None`
/// when nothing is to be written. Written with [`replace_unchanged`], its
/// folder created first; a file changed meanwhile is read and edited again
/// (at most [`EDIT_TRIES`] times), so another writer's edit is kept, never
/// overwritten. `Ok(false)`: nothing was to be written.
pub fn edit_text(
    path: &Path,
    edit: impl Fn(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    for _ in 0..EDIT_TRIES {
        let before = current(path).map_err(|e| e.to_string())?;
        let text = match &before {
            Some(bytes) => {
                std::str::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".to_owned())?
            }
            None => "",
        };
        let Some(after) = edit(text)? else {
            return Ok(false);
        };
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        }
        if replace_unchanged(path, before.as_deref(), after.as_bytes())
            .map_err(|e| e.to_string())?
        {
            return Ok(true);
        }
    }
    Err(format!(
        "another writer changed it each of {EDIT_TRIES} times it was edited; nothing was written"
    ))
}

thread_local! {
    /// The folder (device and inode) whose lock this thread holds through a
    /// [`Holding`]: [`ProjectLock::acquire`] of it on this thread goes on at
    /// once, without taking it again.
    static HELD: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
    /// Inside a [`NoWait`]: how long the request has waited already.
    static NO_WAIT: Cell<Option<Duration>> = const { Cell::new(None) };
}

/// A folder's identity, for [`HELD`].
fn folder_id(dir: &Path) -> io::Result<(u64, u64)> {
    let metadata = fs::metadata(dir)?;
    Ok((metadata.dev(), metadata.ino()))
}

/// While it lives, [`ProjectLock::acquire`] on this thread never waits: it
/// tries once and, when another writer holds the lock, fails at once
/// ([`io::ErrorKind::WouldBlock`]). For a thread that must not block (the
/// UI host's control thread); `waited` is how long the request it runs has
/// waited for the lock already, which the failure states.
#[derive(Debug)]
pub struct NoWait {
    previous: Option<Duration>,
}

impl NoWait {
    /// Enters a scope without waiting, for a request that has waited
    /// `waited` so far.
    pub fn enter(waited: Duration) -> NoWait {
        NoWait {
            previous: NO_WAIT.replace(Some(waited)),
        }
    }
}

impl Drop for NoWait {
    fn drop(&mut self) {
        NO_WAIT.set(self.previous);
    }
}

/// A project folder's lock held by this thread for the writers it calls
/// ([`ProjectLock::try_hold`]): their [`ProjectLock::acquire`] of the same
/// folder, on this thread, goes on at once. Released when dropped.
#[derive(Debug)]
pub struct Holding {
    previous: Option<(u64, u64)>,
    _lock: ProjectLock,
}

impl Drop for Holding {
    fn drop(&mut self) {
        // Then the lock itself, as a field.
        HELD.set(self.previous);
    }
}

/// The project folder's writer lock (see the module's notes), held while
/// the guard lives.
#[derive(Debug)]
pub struct ProjectLock {
    /// `None`: the file system has no locks; the writer goes on unlocked.
    folder: Option<File>,
}

impl ProjectLock {
    /// Takes the lock of the project folder `dir`, waiting at most
    /// [`LOCK_WAIT`] for another writer to finish. A writer that already
    /// holds it must not take it again (it would wait for itself), unless
    /// its thread holds it through a [`Holding`]: then this goes on at once
    /// with a guard that releases nothing. Inside a [`NoWait`] it never
    /// waits: another writer holding the lock fails it at once.
    pub fn acquire(dir: &Path) -> io::Result<ProjectLock> {
        if let Some(held) = HELD.get()
            && folder_id(dir).ok() == Some(held)
        {
            return Ok(ProjectLock { folder: None });
        }
        if let Some(waited) = NO_WAIT.get() {
            return ProjectLock::try_acquire(dir)?.ok_or_else(|| {
                let message = if waited >= LOCK_WAIT {
                    format!(
                        "{}: another writer has held the project's lock for over {} s",
                        dir.display(),
                        LOCK_WAIT.as_secs()
                    )
                } else {
                    format!("{}: another writer holds the project's lock", dir.display())
                };
                io::Error::new(io::ErrorKind::WouldBlock, message)
            });
        }
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

    /// The lock of `dir` taken now for this thread's writers (a
    /// [`Holding`]), `None` when another writer holds it. Never waits. On a
    /// thread that holds it already (an outer `Holding` of the same
    /// folder) it is a `Holding` that releases nothing.
    pub fn try_hold(dir: &Path) -> io::Result<Option<Holding>> {
        let id = folder_id(dir)?;
        if HELD.get() == Some(id) {
            return Ok(Some(Holding {
                previous: Some(id),
                _lock: ProjectLock { folder: None },
            }));
        }
        Ok(ProjectLock::try_acquire(dir)?.map(|lock| Holding {
            previous: HELD.replace(Some(id)),
            _lock: lock,
        }))
    }

    /// The lock of `dir` if no other writer holds it now, `None` otherwise.
    /// When the file system gives no lock (`Unsupported`, `ENOTSUP`,
    /// `EOPNOTSUPP`: some network and FAT volumes have none; `ENOLCK`: none
    /// is available now) the writer goes on without one, after a warning
    /// printed once per process, and the next writer asks again. Unlocked,
    /// every writer still compares the files it writes with what it read
    /// just before renaming (ADR 0040): a change made before that read is
    /// refused, but the read and the rename are not one step, so a writer
    /// renaming in between is not seen.
    pub fn try_acquire(dir: &Path) -> io::Result<Option<ProjectLock>> {
        let folder = File::open(dir)?;
        match folder.try_lock() {
            Ok(()) => Ok(Some(ProjectLock {
                folder: Some(folder),
            })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) if unsupported(&error) => {
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    eprintln!(
                        "signal: {}: the file system has no file locks ({error}); project files are written without the writer lock",
                        dir.display()
                    );
                });
                Ok(Some(ProjectLock { folder: None }))
            }
            Err(fs::TryLockError::Error(error)) => Err(error),
        }
    }
}

/// Whether the file system of the project folder `dir` gives the writer
/// lock now: `false` when writers go on unlocked (see
/// [`ProjectLock::try_acquire`]). Another writer holding it counts as
/// giving it. Asks without waiting and prints no warning.
pub fn locks_work(dir: &Path) -> io::Result<bool> {
    let folder = File::open(dir)?;
    match folder.try_lock() {
        Ok(()) => {
            let _ = folder.unlock();
            Ok(true)
        }
        Err(fs::TryLockError::WouldBlock) => Ok(true),
        Err(fs::TryLockError::Error(error)) if unsupported(&error) => Ok(false),
        Err(fs::TryLockError::Error(error)) => Err(error),
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        // Released explicitly, not only by the close: a child process
        // forked meanwhile (and not yet exec'd) shares the descriptor and
        // would keep the lock until it execs.
        if let Some(folder) = &self.folder {
            let _ = folder.unlock();
        }
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

    /// ADR 0040 item 5: `replace_unchanged` renames only over the bytes it
    /// was given (or over nothing, for `None`); a changed file is kept and
    /// no staging file is left.
    #[test]
    fn replace_unchanged_refuses_a_changed_file() {
        let dir = folder("unchanged");
        let path = dir.join("project.toml");
        fs::write(&path, "theirs").unwrap();
        assert!(!replace_unchanged(&path, Some(b"ours"), b"new").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"theirs");
        assert!(
            !replace_unchanged(&path, None, b"new").unwrap(),
            "it exists"
        );
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "no staging file left"
        );
        assert!(replace_unchanged(&path, Some(b"theirs"), b"new").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"new");
        let fresh = dir.join("fresh.toml");
        assert!(replace_unchanged(&fresh, None, b"first").unwrap());
        assert_eq!(fs::read(&fresh).unwrap(), b"first");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// `edit_text` edits a file changed before its rename again, keeping the
    /// other writer's edit; `None` writes nothing.
    #[test]
    fn edit_text_edits_a_changed_file_again() {
        let dir = folder("edit");
        let path = dir.join("ui/schematic.toml");
        let tries = std::cell::Cell::new(0);
        let written = edit_text(&path, |text| {
            tries.set(tries.get() + 1);
            if tries.get() == 1 {
                // Another writer, after this read and before the rename.
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, "theirs\n").unwrap();
            }
            Ok(Some(format!("{text}ours\n")))
        })
        .unwrap();
        assert!(written);
        assert_eq!(tries.get(), 2);
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs\nours\n");
        assert!(!edit_text(&path, |_| Ok(None)).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs\nours\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The lock excludes a second writer, in this process too, until the
    /// first one's guard drops.
    #[test]
    fn the_lock_excludes_a_second_writer() {
        let dir = folder("lock");
        let held = ProjectLock::acquire(&dir).unwrap();
        assert!(ProjectLock::try_acquire(&dir).unwrap().is_none());
        assert!(
            locks_work(&dir).unwrap(),
            "held by another writer: it works"
        );
        drop(held);
        assert!(locks_work(&dir).unwrap());
        assert!(ProjectLock::try_acquire(&dir).unwrap().is_some());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "no lock file");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// ADR 0048: a thread holding the lock through
    /// a `Holding` lets its own writers take it at once; under a `NoWait`
    /// a held lock fails `acquire` at once instead of waiting; both scopes
    /// end with their guards.
    #[test]
    fn a_held_lock_is_reentered_and_no_wait_never_waits() {
        let dir = folder("hold");
        let holding = ProjectLock::try_hold(&dir).unwrap().unwrap();
        let started = Instant::now();
        let inner = ProjectLock::acquire(&dir).unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
        drop(inner);
        assert!(
            ProjectLock::try_acquire(&dir).unwrap().is_none(),
            "the inner guard released nothing"
        );
        // A nested hold on the holding thread: given, and releasing nothing.
        let nested = ProjectLock::try_hold(&dir).unwrap().expect("nested hold");
        drop(nested);
        assert!(ProjectLock::try_acquire(&dir).unwrap().is_none());
        assert!(ProjectLock::acquire(&dir).is_ok(), "still re-entered");
        // Another thread is another writer: it finds the lock held.
        let other = dir.clone();
        let refused = std::thread::spawn(move || {
            let _no_wait = NoWait::enter(Duration::ZERO);
            ProjectLock::acquire(&other).map(|_| ())
        })
        .join()
        .unwrap()
        .unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
        assert!(refused.to_string().contains("holds the project's lock"));
        assert!(
            ProjectLock::try_hold(&folder("hold-other"))
                .unwrap()
                .is_some()
        );
        drop(holding);
        // Held by another thread now; this one, without waiting.
        let (release, taken) = std::sync::mpsc::channel::<()>();
        let (held, wait) = std::sync::mpsc::channel::<()>();
        let other = dir.clone();
        let writer = std::thread::spawn(move || {
            let _lock = ProjectLock::acquire(&other).unwrap();
            held.send(()).unwrap();
            let _ = taken.recv();
        });
        wait.recv().unwrap();
        {
            let _no_wait = NoWait::enter(LOCK_WAIT);
            let started = Instant::now();
            let refused = ProjectLock::acquire(&dir).unwrap_err();
            assert!(started.elapsed() < Duration::from_millis(100));
            assert!(refused.to_string().contains("for over 10 s"), "{refused}");
            assert!(ProjectLock::try_hold(&dir).unwrap().is_none());
        }
        release.send(()).unwrap();
        writer.join().unwrap();
        assert!(ProjectLock::acquire(&dir).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A link planted at the next staging name is neither followed nor
    /// removed: the name after it is taken.
    #[test]
    fn a_planted_staging_name_is_skipped() {
        let dir = folder("planted");
        let path = dir.join("project.toml");
        let outside = dir.join("outside.txt");
        fs::write(&outside, "outside").unwrap();
        let pid = std::process::id();
        let planted = dir.join(format!(".project.toml.{pid}.0.tmp"));
        std::os::unix::fs::symlink(&outside, &planted).unwrap();
        let mut n = 0;
        let staged = stage_numbered(&path, b"new", || {
            n += 1;
            n - 1
        })
        .unwrap();
        assert_eq!(
            staged.path(),
            dir.join(format!(".project.toml.{pid}.1.tmp"))
        );
        staged.replace(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
        assert!(
            fs::symlink_metadata(&planted)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Without hard links (exFAT answers `ENOTSUP`, 45 on macOS) a new file
    /// is copied into a file created exclusively: it never replaces one.
    #[test]
    fn create_copies_where_links_are_unsupported() {
        let dir = folder("nolink");
        let path = dir.join("a.toml");
        #[cfg(target_os = "linux")]
        let enotsup = 95;
        #[cfg(not(target_os = "linux"))]
        let enotsup = 45;
        let no_link = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(enotsup));
        stage(&path, b"one")
            .unwrap()
            .create_with(&path, no_link)
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        let again = stage(&path, b"two")
            .unwrap()
            .create_with(&path, no_link)
            .unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"one");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no staging left");
        // Another failure is reported as it is, nothing copied.
        let other = dir.join("b.toml");
        let denied = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(13));
        assert!(
            stage(&other, b"x")
                .unwrap()
                .create_with(&other, denied)
                .is_err()
        );
        assert!(!other.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The errors that mean "no locks or links here" and those that do not.
    #[test]
    fn unsupported_errors() {
        #[cfg(target_os = "linux")]
        let codes = [95, 37];
        #[cfg(not(target_os = "linux"))]
        let codes = [45, 102, 77];
        for code in codes {
            assert!(unsupported(&io::Error::from_raw_os_error(code)), "{code}");
        }
        assert!(unsupported(&io::Error::from(io::ErrorKind::Unsupported)));
        assert!(!unsupported(&io::Error::from_raw_os_error(13)));
        assert!(!unsupported(&io::Error::from(io::ErrorKind::WouldBlock)));
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
