//! A `.signal` project opened in Signal.app (M2.8-12, ADR 0025): the file,
//! and the working folder it is unpacked into under
//! `~/Library/Application Support/Signal/Open/`, where live edits, live
//! reload and the panel work as on any project folder. The project is
//! *edited* while that folder packs to other bytes than the file held when
//! it was opened or last saved. Save writes the archive back atomically;
//! closing removes the working folder, unless it is edited and the edits
//! were not let go, so edits are never lost silently. Control-thread code;
//! the display's content comparisons run on a thread of their own, one at
//! a time, and a result counts only if nothing changed since it started.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use signal_project::archive::{self, ArchiveError};

/// How often a refresh looks at the working folder's files.
const LOOK_EVERY: Duration = Duration::from_millis(250);

/// How often a refresh compares the working folder's content with the file
/// even when no file's size or time changed (an edit can keep both): at
/// least every 2 s, and no more often than twenty times the last comparison
/// took, so a large project's display poll keeps a core busy at most about
/// 5 % of the time. The comparison runs on its own thread, so the control
/// thread never waits for it. Decisions compare at once ([`Document::check`]).
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

/// A comparison running on its own thread: the version of the folder it
/// was started for, and where its result (edited, and how long it took)
/// arrives.
#[derive(Debug)]
struct Job {
    version: u64,
    result: mpsc::Receiver<(bool, Duration)>,
}

/// Whether the working folder differs from `saved`, the archive it was
/// packed to when opened or saved; a folder that no longer packs
/// (project.toml gone) differs. Nothing is compressed
/// ([`archive::packs_to`]).
fn differs(working: &Path, saved: &[u8]) -> bool {
    archive::packs_to(working, saved).map_or(true, |same| !same)
}

/// Test hooks: holds comparison threads after they compared and before
/// their result is sent, as if a comparison took long: a test sees one
/// pending, and its result arrive late (each waits at most 10 s).
#[cfg(feature = "test-hooks")]
#[derive(Debug, Default)]
struct Hold {
    held: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
    waiting: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "test-hooks")]
impl Hold {
    fn wait(&self) {
        use std::sync::atomic::Ordering;
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self
            .changed
            .wait_timeout_while(held, Duration::from_secs(10), |held| *held);
        self.waiting.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An opened `.signal` file and its working folder.
#[derive(Debug)]
pub struct Document {
    file: PathBuf,
    working: PathBuf,
    /// What the working folder packed to when opened or last saved; shared
    /// with a comparison's thread.
    saved: Arc<Vec<u8>>,
    look: Look,
    looked: Option<Instant>,
    /// When the content was last compared, and how long that took.
    compared: Instant,
    cost: Duration,
    edited: bool,
    /// Counts what can make a comparison stale: a change of the folder's
    /// files seen by a look, a comparison made at once, a save. A result
    /// counts only for the version it was started for.
    version: u64,
    /// Whether this version still needs a comparison.
    wanted: bool,
    /// The comparison running, at most one.
    job: Option<Job>,
    #[cfg(feature = "test-hooks")]
    hold: Arc<Hold>,
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
            saved: Arc::new(saved),
            looked: Some(Instant::now()),
            compared: Instant::now(),
            cost: Duration::ZERO,
            edited: false,
            version: 0,
            wanted: false,
            job: None,
            #[cfg(feature = "test-hooks")]
            hold: Arc::default(),
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

    /// What the window shows; never waits for a comparison (a decision uses
    /// [`check`](Document::check)). Takes a finished comparison's result,
    /// looks at the working folder at most every 250 ms, and starts a
    /// comparison on its own thread when its files changed, or when
    /// [`COMPARE_EVERY`] (stretched for a large project) has passed. A change
    /// of the files counts as edited at once, until a comparison of the
    /// folder as changed says otherwise; a comparison started before a
    /// change is stale and let go. Returns whether it is edited.
    pub fn refresh(&mut self) -> bool {
        self.collect();
        if !self.looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            self.looked = Some(Instant::now());
            let current = look(&self.working);
            if current != self.look {
                self.look = current;
                self.changed();
            }
        }
        if self.compared.elapsed() >= COMPARE_EVERY.max(self.cost * 20) {
            self.wanted = true;
        }
        if self.wanted && self.job.is_none() {
            self.start();
        }
        self.edited
    }

    /// The folder's files changed: edited, until compared again.
    fn changed(&mut self) {
        self.version += 1;
        self.edited = true;
        self.wanted = true;
    }

    /// A finished comparison's result, if it is not stale: one whose files
    /// changed while it ran (looked at again here) counts as a change.
    fn collect(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let (edited, cost) = match job.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            // Its thread ended without a result: edited, as for a folder
            // that does not pack, until the next comparison is due.
            Err(mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                self.edited = true;
                self.wanted = false;
                self.compared = Instant::now();
                return;
            }
        };
        let version = job.version;
        self.job = None;
        self.compared = Instant::now();
        self.cost = cost;
        if version != self.version {
            return;
        }
        self.looked = Some(Instant::now());
        let current = look(&self.working);
        if current != self.look {
            self.look = current;
            self.changed();
            return;
        }
        self.edited = edited;
        self.wanted = false;
    }

    /// Starts a comparison of the current version on its own thread (here,
    /// when no thread can be made).
    fn start(&mut self) {
        let (send, result) = mpsc::sync_channel(1);
        let working = self.working.clone();
        let saved = Arc::clone(&self.saved);
        #[cfg(feature = "test-hooks")]
        let hold = Arc::clone(&self.hold);
        let spawned = thread::Builder::new()
            .name("signal-document".to_owned())
            .spawn(move || {
                let started = Instant::now();
                let edited = differs(&working, &saved);
                let cost = started.elapsed();
                #[cfg(feature = "test-hooks")]
                hold.wait();
                let _ = send.send((edited, cost));
            });
        match spawned {
            Ok(_) => {
                self.job = Some(Job {
                    version: self.version,
                    result,
                });
            }
            Err(_) => self.compare(),
        }
    }

    /// Compares the working folder's content with the file now, whatever
    /// its files' sizes and times say: before every decision that may
    /// discard or replace it (a prompt, Open, close). A comparison still
    /// running is stale from then on.
    pub fn check(&mut self) -> bool {
        self.looked = Some(Instant::now());
        self.look = look(&self.working);
        self.version += 1;
        self.compare();
        self.edited
    }

    fn compare(&mut self) {
        let started = Instant::now();
        self.edited = differs(&self.working, &self.saved);
        self.wanted = false;
        self.compared = Instant::now();
        self.cost = started.elapsed();
    }

    /// Test hooks: holds (or releases) comparison threads before they send
    /// their result.
    #[cfg(feature = "test-hooks")]
    pub fn hold_comparisons(&self, held: bool) {
        *self.hold.held.lock().unwrap_or_else(|e| e.into_inner()) = held;
        self.hold.changed.notify_all();
    }

    /// Test hooks: how many comparison threads are held now.
    #[cfg(feature = "test-hooks")]
    pub fn comparisons_held(&self) -> usize {
        self.hold.waiting.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Test hooks: whether a comparison is running (or its result not yet
    /// taken).
    #[cfg(feature = "test-hooks")]
    pub fn comparing(&self) -> bool {
        self.job.is_some()
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
        self.saved = Arc::new(bytes);
        self.look = look(&self.working);
        self.looked = Some(Instant::now());
        self.compared = Instant::now();
        self.edited = false;
        // A comparison still running compared with the old content.
        self.version += 1;
        self.wanted = false;
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
