//! Live edits from project files (ADR 0006, D17): [`ProjectWatcher`] notices
//! that a project folder's files changed, and [`send_edit`] turns a changed
//! project into commands on a real-time [`Controller`], recorded through a
//! [`LiveRecorder`]. Control-thread code: it allocates and does I/O.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use signal_core::{Frame, Registry};
use signal_engine::{Controller, Stamp};

use crate::session::{LiveRecorder, sink_of};
use crate::{Diagnostic, Project, ProjectEdit, diff};

/// How often `--watch` polls a project folder.
pub const WATCH_INTERVAL_MS: u64 = 100;

/// Each file of a folder (relative path) with its modification time and length.
type Snapshot = BTreeMap<PathBuf, (Option<SystemTime>, u64)>;

/// Notices changes to the files of a project folder by polling their
/// modification times and lengths (no file-system events, no new crates).
/// Hidden files and folders are ignored.
pub struct ProjectWatcher {
    dir: PathBuf,
    last: Snapshot,
}

impl ProjectWatcher {
    /// Watches `dir`, as it is now.
    pub fn new(dir: &Path) -> io::Result<ProjectWatcher> {
        Ok(ProjectWatcher {
            dir: dir.to_owned(),
            last: snapshot(dir)?,
        })
    }

    /// Whether a file was added, removed or changed since the last poll.
    pub fn poll(&mut self) -> io::Result<bool> {
        let now = snapshot(&self.dir)?;
        let changed = now != self.last;
        self.last = now;
        Ok(changed)
    }
}

fn snapshot(dir: &Path) -> io::Result<Snapshot> {
    let mut files = Snapshot::new();
    let mut folders = vec![dir.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                folders.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap_or(&path).to_owned();
                files.insert(relative, (metadata.modified().ok(), metadata.len()));
            }
        }
    }
    Ok(files)
}

/// What [`send_edit`] sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Parameter `set`s (or ramps) sent.
    pub sets: usize,
    /// The plan version swapped to, if the edit was structural.
    pub swap: Option<u64>,
    /// Instances suspended for removal before the swap.
    pub removed: usize,
}

/// What `--watch` reports: `applied a swap to version 3 (1 sets, 0 removed)`,
/// `applied 2 sets`, or `no change to apply`.
impl fmt::Display for Applied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.swap {
            Some(version) => write!(
                f,
                "applied a swap to version {version} ({} sets, {} removed)",
                self.sets, self.removed
            ),
            None if self.sets == 0 => f.write_str("no change to apply"),
            None => write!(f, "applied {} sets", self.sets),
        }
    }
}

/// Why [`send_edit`] sent nothing (or not everything).
#[derive(Debug)]
pub enum EditError {
    /// The new project cannot replace the running one: its plan is invalid,
    /// it changes the sample rate, seed or audio sink.
    Invalid(Vec<Diagnostic>),
    /// The Controller refused a command (a full queue, a refused swap).
    Refused(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::Invalid(diagnostics) => {
                for (i, diagnostic) in diagnostics.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{diagnostic}")?;
                }
                Ok(())
            }
            EditError::Refused(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EditError {}

/// Sends the edit that turns the running `old` project into `new` (M2-2
/// [`diff`]) through `controller`, recorded by `live`, and returns the project
/// now running.
///
/// Parameter changes are `set`s stamped [`Stamp::Default`]. A structural
/// change is a swap at the default frame with the survivors' changed
/// parameters stamped for the same frame; removed instances are first
/// suspended there and the swap follows at the removal frame
/// (`docs/semantics.md` §12, "Removal"). The Controller is polled first, so
/// the default frame is current. Nothing is sent when the new project cannot
/// replace the old one, when its swap cannot be prepared
/// ([`Controller::prepare_swap`]), or when the Controller cannot take every
/// command of the edit, and the resumes that would undo its suspends, now
/// ([`Controller::available`]).
pub fn send_edit(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    old: &Project,
    new: Project,
    registry: &Registry,
) -> Result<(Project, Applied), EditError> {
    let edit = diff(old, &new, registry).map_err(EditError::Invalid)?;
    let refused = |error: signal_engine::ControlError| EditError::Refused(error.to_string());
    let mut applied = Applied::default();
    match edit {
        ProjectEdit::Parameters(changes) => {
            controller.poll(live);
            room(controller, changes.len())?;
            for change in changes {
                controller
                    .send_event(change.port, change.payload, Stamp::Default)
                    .map_err(refused)?;
                applied.sets += 1;
            }
            Ok((new, applied))
        }
        ProjectEdit::Structural {
            plan,
            removed,
            sets,
            ..
        } => {
            let before = old.plan(registry).map_err(EditError::Invalid)?.plan;
            let sink = sink_of(old, &before).map_err(EditError::Invalid)?;
            let after = sink_of(&new, &plan).map_err(EditError::Invalid)?;
            if after != sink {
                return Err(EditError::Invalid(vec![new.instance_error(
                    &after.0,
                    format!(
                        "{}: the audio sink must stay {} with {} inputs while the project plays; restart to change it",
                        after.0, sink.0, sink.1
                    ),
                )]));
            }
            // Nothing is sent unless the whole edit can complete: a removal
            // stopped half-way would leave instances suspended in the plan
            // that keeps playing. So the swap is prepared first (every
            // refusal but a full queue happens there), and the batch
            // (suspends, swap, sets) and the resumes that would undo the
            // suspends must all fit in the queue.
            let commands = 2 * removed.len() + 1 + sets.len();
            controller.poll(live);
            room(controller, commands)?;
            let files = new.files.clone();
            let plan = live
                .swap_plan(new)
                .map_err(|e| EditError::Refused(e.to_string()))?;
            let ready = match controller.prepare_swap(&plan) {
                Ok(ready) => ready,
                Err(error) => {
                    live.swap_not_sent();
                    return Err(EditError::Refused(format!("{error}; nothing was sent")));
                }
            };
            // Preparing can take long while the runner plays on: poll again
            // so the frames below come from a fresh status, not one stale by
            // the preparation (else the suspend and the swap could both be
            // late and land on one frame, skipping the removal's fade).
            controller.poll(live);
            if let Err(error) = room(controller, commands) {
                live.swap_not_sent();
                return Err(error);
            }

            let start = controller.default_frame();
            for address in &removed {
                if let Err(error) = controller.send_suspend(address.clone(), Stamp::At(start)) {
                    live.swap_not_sent();
                    resume(controller, &removed[..applied.removed], start);
                    return Err(refused(error));
                }
                applied.removed += 1;
            }
            let frame = if removed.is_empty() {
                start
            } else {
                controller.removal_swap_frame(start)
            };
            if let Err(error) = controller.send_swap(ready, Stamp::At(frame)) {
                live.swap_not_sent();
                resume(controller, &removed, start);
                return Err(refused(error));
            }
            applied.swap = Some(plan.version);
            for change in sets {
                controller
                    .send_event(change.port, change.payload, Stamp::At(frame))
                    .map_err(refused)?;
                applied.sets += 1;
            }
            let current = Project::from_sources(files).map_err(EditError::Invalid)?;
            Ok((current, applied))
        }
    }
}

/// Why [`reload`] kept the running project.
#[derive(Debug)]
pub enum ReloadError {
    /// The folder's project does not load, or cannot replace the running one;
    /// the diagnostics' files are joined to the folder ([`located`](crate::located)).
    Invalid(Vec<Diagnostic>),
    /// The folder could not be read, or the Controller refused the edit.
    Failed(String),
}

/// What `--watch` (and Signal.app) does when a project folder changed: loads
/// `dir` and sends the edit from `current` to it ([`send_edit`]). On success
/// `current` is the new project; on an error nothing changed and the last
/// good version keeps playing.
pub fn reload(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    current: &mut Project,
    dir: &Path,
    registry: &Registry,
) -> Result<Applied, ReloadError> {
    let new = match Project::load(dir) {
        Ok(new) => new,
        Err(crate::LoadError::Invalid(diagnostics)) => {
            return Err(ReloadError::Invalid(crate::located(dir, diagnostics)));
        }
        Err(error) => return Err(ReloadError::Failed(error.to_string())),
    };
    match send_edit(controller, live, current, new, registry) {
        Ok((project, applied)) => {
            *current = project;
            Ok(applied)
        }
        Err(EditError::Invalid(diagnostics)) => {
            Err(ReloadError::Invalid(crate::located(dir, diagnostics)))
        }
        Err(EditError::Refused(message)) => Err(ReloadError::Failed(message)),
    }
}

/// Refuses an edit of `commands` commands that the Controller cannot take
/// whole now; nothing is sent.
fn room(controller: &Controller, commands: usize) -> Result<(), EditError> {
    let available = controller.available();
    if commands <= available {
        return Ok(());
    }
    let in_flight = controller.in_flight();
    Err(EditError::Refused(format!(
        "the edit needs {commands} commands but the command queue has room for {available} \
         ({in_flight} in flight, at most {}); nothing was sent. Save again once the \
         commands in flight have applied, or remove fewer instances at once (one save \
         can remove at most about half the queue: each removal also reserves its resume)",
        available + in_flight
    )))
}

/// Resumes instances suspended at `suspended` for a swap that was not sent.
/// It cannot be refused for room: the resumes were counted with the batch.
fn resume(
    controller: &mut Controller,
    addresses: &[signal_core::InstanceAddress],
    suspended: Frame,
) {
    for address in addresses {
        let _ = controller.send_resume(address.clone(), Stamp::At(Frame(suspended.0 + 1)));
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectWatcher;
    use std::fs;

    #[test]
    fn the_watcher_notices_changed_and_added_files_only() {
        let dir = std::env::temp_dir().join(format!("signal-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("composites")).unwrap();
        fs::write(dir.join("project.toml"), "a").unwrap();
        let mut watcher = ProjectWatcher::new(&dir).unwrap();
        assert!(!watcher.poll().unwrap(), "nothing changed");
        fs::write(dir.join("project.toml"), "ab").unwrap();
        assert!(watcher.poll().unwrap(), "a changed file");
        assert!(!watcher.poll().unwrap());
        fs::write(dir.join("composites/voice.toml"), "x").unwrap();
        assert!(watcher.poll().unwrap(), "an added file");
        fs::write(dir.join(".hidden"), "x").unwrap();
        assert!(!watcher.poll().unwrap(), "hidden files are ignored");
        fs::remove_dir_all(&dir).unwrap();
    }
}
