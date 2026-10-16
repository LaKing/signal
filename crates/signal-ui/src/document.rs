//! A `.signal` project opened in Signal.app (M2.8-12, ADR 0025): the file,
//! and the working folder it is unpacked into under
//! `~/Library/Application Support/Signal/Open/`, where live edits, live
//! reload and the panel work as on any project folder. The project is
//! *edited* while that folder packs to other bytes than the file held when
//! it was opened or last saved. Save writes the archive back atomically;
//! closing removes the working folder, unless it is edited and the edits
//! were not let go, so edits are never lost silently. Control-thread code.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use signal_project::archive::{self, ArchiveError};

/// How often a refresh looks at the working folder's files.
const LOOK_EVERY: Duration = Duration::from_millis(250);

/// How often a refresh compares the working folder's content with the file
/// even when no file's size or time changed (an edit can keep both): at
/// least every 2 s, and no more often than twenty times the last comparison
/// took, so a large project's display poll spends at most about 5 % of the
/// control thread on it. Decisions compare at once ([`Document::check`]).
pub const COMPARE_EVERY: Duration = Duration::from_secs(2);

/// Where opened `.signal` files are unpacked: `Signal/Open` beside the
/// Setup's default place, `~/Library/Application Support/Signal/`.
pub fn open_root(home: Option<OsString>) -> Option<PathBuf> {
    let home = home.filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support/Signal/Open"))
}

/// The name a project is shown and saved by: a file's stem, a folder's name.
pub fn project_name(path: &Path) -> String {
    let named = if archive::is_project_file(path) {
        path.file_stem()
    } else {
        path.file_name()
    };
    named.map_or_else(
        || "Untitled".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The working folder's files, as a cheap look: paths, sizes, change times.
type Look = Vec<(PathBuf, u64, Option<SystemTime>)>;

fn look(dir: &Path) -> Look {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(path),
                Ok(meta) => found.push((path, meta.len(), meta.modified().ok())),
                Err(_) => {}
            }
        }
    }
    found.sort();
    found
}

fn reason(file: &Path) -> impl FnOnce(ArchiveError) -> String + '_ {
    move |error| match error {
        error @ ArchiveError::Io { .. } => error.to_string(),
        error => format!("{} was not opened: {error}", file.display()),
    }
}

/// An opened `.signal` file and its working folder.
#[derive(Debug)]
pub struct Document {
    file: PathBuf,
    working: PathBuf,
    /// What the working folder packed to when opened or last saved.
    saved: Vec<u8>,
    look: Look,
    looked: Option<Instant>,
    /// When the content was last compared, and how long that took.
    compared: Instant,
    cost: Duration,
    edited: bool,
}

impl Document {
    /// Unpacks `file` into a new working folder under `root`; a refused
    /// archive makes nothing, and the error names the reason.
    pub fn open(file: &Path, root: &Path) -> Result<Document, String> {
        let bytes = archive::read_archive(file).map_err(reason(file))?;
        let working =
            archive::unique_folder(root, &project_name(file)).map_err(|e| e.to_string())?;
        if let Err(error) = archive::unpack(&bytes, &working) {
            let _ = fs::remove_dir_all(&working);
            return Err(reason(file)(error));
        }
        // What the file means, as this reader packs it: an archive written
        // by another tool is not edited merely by being opened.
        let saved = match archive::pack(&working) {
            Ok(saved) => saved,
            Err(error) => {
                let _ = fs::remove_dir_all(&working);
                return Err(reason(file)(error));
            }
        };
        Ok(Document {
            file: file.to_owned(),
            look: look(&working),
            working,
            saved,
            looked: Some(Instant::now()),
            compared: Instant::now(),
            cost: Duration::ZERO,
            edited: false,
        })
    }

    /// The `.signal` file.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The working folder.
    pub fn working(&self) -> &Path {
        &self.working
    }

    /// The project's name: the file's stem.
    pub fn name(&self) -> String {
        project_name(&self.file)
    }

    /// Whether the working folder differs from the file, as last refreshed.
    pub fn edited(&self) -> bool {
        self.edited
    }

    /// Looks at the working folder at most every 250 ms and packs it again
    /// when its files changed, or when [`COMPARE_EVERY`] (stretched for a
    /// large project) has passed; returns whether it is edited. (What the
    /// window shows; a decision uses [`check`](Document::check).)
    pub fn refresh(&mut self) -> bool {
        if self.looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return self.edited;
        }
        self.looked = Some(Instant::now());
        let current = look(&self.working);
        let due = self.compared.elapsed() >= COMPARE_EVERY.max(self.cost * 20);
        if current != self.look || due {
            self.look = current;
            self.compare();
        }
        self.edited
    }

    /// Compares the working folder's content with the file now, whatever
    /// its files' sizes and times say: before every decision that may
    /// discard or replace it (a prompt, Open, close).
    pub fn check(&mut self) -> bool {
        self.looked = Some(Instant::now());
        self.look = look(&self.working);
        self.compare();
        self.edited
    }

    fn compare(&mut self) {
        let started = Instant::now();
        // A folder that no longer packs (project.toml gone) differs.
        self.edited = archive::pack(&self.working).map_or(true, |bytes| bytes != self.saved);
        self.compared = Instant::now();
        self.cost = started.elapsed();
    }

    /// Save: the working folder packed and written over the file atomically.
    pub fn save(&mut self) -> Result<(), String> {
        let file = self.file.clone();
        self.write(&file)
    }

    /// Save As: written to `file`, which the project is from then on.
    pub fn save_as(&mut self, file: &Path) -> Result<(), String> {
        self.write(file)?;
        self.file = file.to_owned();
        Ok(())
    }

    fn write(&mut self, file: &Path) -> Result<(), String> {
        let bytes = archive::pack(&self.working)
            .map_err(|e| format!("{} was not saved: {e}", file.display()))?;
        archive::write_atomic(file, &bytes)
            .map_err(|e| format!("{} was not saved: {e}", file.display()))?;
        self.saved = bytes;
        self.look = look(&self.working);
        self.looked = Some(Instant::now());
        self.compared = Instant::now();
        self.edited = false;
        Ok(())
    }

    /// Closes the document: its working folder is removed, unless it is
    /// edited and `discard` is false; then it is kept, and returned.
    pub fn close(mut self, discard: bool) -> Option<PathBuf> {
        if !discard && self.check() {
            return Some(self.working);
        }
        let _ = fs::remove_dir_all(&self.working);
        None
    }
}
