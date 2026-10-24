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
//! Saves too (audit: a 64 MiB asset held the control thread 728 ms): each
//! packs and writes on a thread of its own, one at a time in the order
//! asked, and clears *edited* only if the folder did not change since it
//! started.

use std::collections::VecDeque;
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

/// A save's ticket: its result is told by it ([`Document::take_saved`]).
pub type Ticket = u64;

/// How many times a save packs the folder again when it changed while it
/// was packed, before it writes the last packing (and the folder stays
/// edited).
const SETTLE_TRIES: usize = 3;

/// A save asked for and not yet started: to the document's file as it is
/// when it starts (`None`), or Save As to `Some` file.
#[derive(Debug)]
struct Asked {
    ticket: Ticket,
    file: Option<PathBuf>,
    /// Whether a later Save to the same file may share it: one asked
    /// through [`Document::start_save`], whose result is told.
    shared: bool,
}

/// What a save's thread sends: the bytes written and whether the folder
/// still packed to them after they were packed (it did not change during
/// the packing), or why nothing was written.
type Packed = Result<(Vec<u8>, bool), String>;

/// A save running on its own thread: the folder's files as looked at when
/// it started, so that its completion clears *edited* only if nothing
/// changed since.
#[derive(Debug)]
struct Saving {
    ticket: Ticket,
    file: PathBuf,
    rename: bool,
    look: Look,
    result: mpsc::Receiver<Packed>,
}

/// Packs `working` and writes it over `file` atomically: a save's work, on
/// its thread. A folder that changed while it was packed (it no longer
/// packs to the bytes made, [`archive::packs_to`]) is packed again, at most
/// [`SETTLE_TRIES`] times; the bytes written are always one packing's, and
/// say whether that packing settled.
fn pack_and_write(working: &Path, file: &Path, hold: &SaveHold) -> Packed {
    let failed = |e: ArchiveError| format!("{} was not saved: {e}", file.display());
    let mut bytes = archive::pack(working).map_err(failed)?;
    let mut settled = false;
    for _ in 0..SETTLE_TRIES {
        if archive::packs_to(working, &bytes).is_ok_and(|same| same) {
            settled = true;
            break;
        }
        bytes = archive::pack(working).map_err(failed)?;
    }
    held(hold);
    archive::write_atomic(file, &bytes).map_err(failed)?;
    Ok((bytes, settled))
}

/// Test hooks: what holds a save after it packed and before it writes
/// (nothing without them).
#[cfg(feature = "test-hooks")]
type SaveHold = Arc<Hold>;
#[cfg(not(feature = "test-hooks"))]
#[derive(Debug, Clone)]
struct SaveHold;

#[cfg(feature = "test-hooks")]
fn no_hold() -> SaveHold {
    Arc::default()
}

#[cfg(not(feature = "test-hooks"))]
fn no_hold() -> SaveHold {
    SaveHold
}

#[cfg(feature = "test-hooks")]
fn held(hold: &SaveHold) {
    hold.wait();
}

#[cfg(not(feature = "test-hooks"))]
fn held(_: &SaveHold) {}

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
    fn set(&self, held: bool) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = held;
        self.changed.notify_all();
    }

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
    /// The save running, at most one, and those asked after it, in order.
    saving: Option<Saving>,
    asked: VecDeque<Asked>,
    /// Finished saves' results, until taken.
    saved_results: Vec<(Ticket, Result<(), String>)>,
    next_ticket: Ticket,
    save_hold: SaveHold,
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
            saving: None,
            asked: VecDeque::new(),
            saved_results: Vec::new(),
            next_ticket: 1,
            save_hold: no_hold(),
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
        self.collect_save();
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
        self.hold.set(held);
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

    /// Save, waiting for it: the working folder packed and written over the
    /// file atomically, after the saves asked before it. For a caller that
    /// must have it done before it goes on (Open with "Save"); the control
    /// thread's Save is [`start_save`](Document::start_save).
    pub fn save(&mut self) -> Result<(), String> {
        let ticket = self.ask(None, false);
        self.wait_for(ticket)
    }

    /// Save As, waiting for it: written to `file`, which the project is from
    /// then on.
    pub fn save_as(&mut self, file: &Path) -> Result<(), String> {
        let ticket = self.ask(Some(file.to_owned()), false);
        self.wait_for(ticket)
    }

    /// Save on a thread of its own: never waits for the packing or the
    /// writing; its result is told by the ticket returned
    /// ([`take_saved`](Document::take_saved)). It packs the folder as it is
    /// when it starts, after the saves asked before it; a Save asked while
    /// another Save waits to start shares that one, which packs the folder
    /// later than either was asked. It clears *edited* only if the folder's
    /// files did not change from when it started and while it packed them.
    pub fn start_save(&mut self) -> Ticket {
        self.ask(None, true)
    }

    /// Save As `file` on a thread of its own, as
    /// [`start_save`](Document::start_save): the project is that file from
    /// when it is written.
    pub fn start_save_as(&mut self, file: &Path) -> Ticket {
        self.ask(Some(file.to_owned()), true)
    }

    /// Whether a save is running or waiting to start.
    pub fn saving(&self) -> bool {
        self.saving.is_some() || !self.asked.is_empty()
    }

    /// The results of the saves finished since they were last taken, in the
    /// order they finished (the order asked). Taken by
    /// [`refresh`](Document::refresh) and
    /// [`finish_saves`](Document::finish_saves).
    pub fn take_saved(&mut self) -> Vec<(Ticket, Result<(), String>)> {
        std::mem::take(&mut self.saved_results)
    }

    /// Waits for every save asked to finish, keeping their results for
    /// [`take_saved`](Document::take_saved): before a decision on the
    /// folder, and before it is removed.
    pub fn finish_saves(&mut self) {
        while self.saving() {
            self.wait_save();
        }
    }

    fn ask(&mut self, file: Option<PathBuf>, shared: bool) -> Ticket {
        if shared
            && let Some(last) = self.asked.back()
            && last.shared
            && last.file == file
        {
            return last.ticket;
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.asked.push_back(Asked {
            ticket,
            file,
            shared,
        });
        self.next_save();
        ticket
    }

    /// Waits for the save `ticket` (and those asked before it); its result.
    fn wait_for(&mut self, ticket: Ticket) -> Result<(), String> {
        loop {
            if let Some(at) = self.saved_results.iter().position(|(t, _)| *t == ticket) {
                return self.saved_results.remove(at).1;
            }
            if !self.saving() {
                return Err(format!("{} was not saved", self.file.display()));
            }
            self.wait_save();
        }
    }

    /// Waits for the running save to finish, and starts the next.
    fn wait_save(&mut self) {
        match &self.saving {
            Some(job) => {
                let outcome = job.result.recv().map_err(|_| ());
                self.finish_save(outcome);
            }
            None => self.next_save(),
        }
    }

    /// The running save's result, if it has finished.
    fn collect_save(&mut self) {
        let Some(job) = &self.saving else {
            return;
        };
        let outcome = match job.result.try_recv() {
            Ok(outcome) => Ok(outcome),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(()),
        };
        self.finish_save(outcome);
    }

    /// Takes the running save's outcome (`Err(())`: its thread ended without
    /// one) and starts the next save asked. The file holds what was written,
    /// whatever changed since, so the folder is compared with that from now
    /// on; *edited* is cleared only if the folder's files are as they were
    /// when the save started and its packing settled. Otherwise the folder
    /// stays edited until a comparison with what was written, due at once,
    /// says otherwise.
    fn finish_save(&mut self, outcome: Result<Packed, ()>) {
        let Some(job) = self.saving.take() else {
            return;
        };
        let result = match outcome {
            Ok(Ok((bytes, settled))) => {
                if job.rename {
                    self.file = job.file;
                }
                self.saved = Arc::new(bytes);
                // A comparison still running compared with the old content.
                self.version += 1;
                self.looked = Some(Instant::now());
                self.compared = Instant::now();
                let current = look(&self.working);
                let unchanged = settled && current == job.look;
                self.look = current;
                self.edited = !unchanged;
                self.wanted = !unchanged;
                Ok(())
            }
            Ok(Err(message)) => Err(message),
            Err(()) => Err(format!(
                "{} was not saved: the save ended without a result",
                job.file.display()
            )),
        };
        self.saved_results.push((job.ticket, result));
        self.next_save();
    }

    /// Starts the next save asked when none is running (here, when no
    /// thread can be made).
    fn next_save(&mut self) {
        while self.saving.is_none() {
            let Some(asked) = self.asked.pop_front() else {
                return;
            };
            let rename = asked.file.is_some();
            let file = asked.file.unwrap_or_else(|| self.file.clone());
            // Looked at before it packs: a change from here on keeps the
            // folder edited.
            let started = look(&self.working);
            let (send, result) = mpsc::sync_channel(1);
            let working = self.working.clone();
            let target = file.clone();
            let hold = self.save_hold.clone();
            let spawned = thread::Builder::new()
                .name("signal-save".to_owned())
                .spawn(move || {
                    let _ = send.send(pack_and_write(&working, &target, &hold));
                });
            let made = spawned.is_ok();
            let packed = (!made).then(|| pack_and_write(&self.working, &file, &no_hold()));
            self.saving = Some(Saving {
                ticket: asked.ticket,
                file,
                rename,
                look: started,
                result,
            });
            if let Some(packed) = packed {
                self.finish_save(Ok(packed));
            }
        }
    }

    /// Test hooks: holds (or releases) save threads after they packed and
    /// before they write (each waits at most 10 s).
    #[cfg(feature = "test-hooks")]
    pub fn hold_saves(&self, held: bool) {
        self.save_hold.set(held);
    }

    /// Test hooks: what releases held save threads from another thread
    /// (while this one waits for a save, in a close).
    #[cfg(feature = "test-hooks")]
    pub fn save_releaser(&self) -> impl FnOnce() + Send + 'static {
        let hold = Arc::clone(&self.save_hold);
        move || hold.set(false)
    }

    /// Test hooks: how many save threads are held now.
    #[cfg(feature = "test-hooks")]
    pub fn saves_held(&self) -> usize {
        self.save_hold
            .waiting
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Closes the document: its working folder is removed, unless it is
    /// edited and `discard` is false; then it is kept, and returned. The
    /// saves asked are finished first, so that none reads a folder being
    /// removed and one that saved the edits lets it go; their results are
    /// not told (a caller that tells them takes them after
    /// [`finish_saves`](Document::finish_saves)).
    pub fn close(mut self, discard: bool) -> Option<PathBuf> {
        self.finish_saves();
        if !discard && self.check() {
            return Some(self.working);
        }
        let _ = fs::remove_dir_all(&self.working);
        None
    }
}
