//! Opening, reopening and reverting the project.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use signal_project::PROJECT_FILE;
use signal_project::archive::{self, TempProject};
use signal_project::live::FolderState;

use crate::app;
use crate::document::{Document, project_name};
use crate::panel::Panel;
use crate::panel_write::sha256_hex;

use super::app_mode::app_only;
use super::failure::{Failure, fail};
use super::run::stop;
use super::{Edits, Host, ProjectStatus, project_ui};

/// The folder of `path`: its parent for a project's `project.toml`.
pub(super) fn folder_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if path.file_name().is_some_and(|n| n == PROJECT_FILE) && path.is_file() => {
            dir.to_owned()
        }
        _ => path.to_owned(),
    }
}

/// Why a project is opened: it only changes what the window says of the
/// unsaved values let go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Why {
    /// Another project (or the same one), opened by `project.open`, the
    /// window's Open…, a drop or Save As.
    Another,
    /// Revert to Saved: the same `.signal` file, its changes discarded.
    Revert,
}

/// The open project: its folder, its generation, and what it was opened
/// from.
pub(super) struct Opening {
    /// The project folder.
    pub(super) dir: PathBuf,
    /// The open project's generation: a new one each time another project
    /// (or a new copy of it) is opened, so a request queued for the old one
    /// is never run on the new one.
    pub(super) generation: u64,
    /// The project as given to `signal ui` (its folder or `.signal` file).
    pub(super) source: PathBuf,
    /// `signal ui`'s temporary folder of a `.signal` file.
    pub(super) temporary: Option<TempProject>,
    /// App mode: where `.signal` files are unpacked.
    pub(super) open_root: PathBuf,
    /// App mode: the open `.signal` file and its working folder.
    pub(super) document: Option<Document>,
    /// ADR 0046 §8: the SHA-256 of every presentation text a patch of this
    /// host wrote or replaced (`ui/…`): a client's `panel.restore` of one
    /// of them (an undo or redo the host made) is taken whole; any other
    /// text is judged strictly ([`Host::panel_restore_allowed`]).
    pub(super) restorable: HashSet<String>,
}

impl Opening {
    /// A new opening of the folder `dir`: the only place the generation
    /// changes.
    pub(super) fn begin(&mut self, dir: &Path) {
        self.dir = dir.to_owned();
        self.generation += 1;
    }

    /// A mutation (a file write, a live edit, `project.save`, and the open
    /// project's `project.revert` or another `project.open`) names the
    /// project it means, or is taken to mean the open one (ADR 0049): a
    /// `generation` (the open project's, as `welcome`, `describe`, `graph`
    /// and `app` give it) must be the open project's generation, else a
    /// `projectId` (`graph.projectId`) must be its id; either one not
    /// matching is `project-changed`, and nothing is done. A message
    /// without either is taken to mean the open project (MCP, the CLI and
    /// pages that send none).
    pub(super) fn admit(&self, mutation: bool, message: &Value) -> Result<(), Failure> {
        if !mutation {
            return Ok(());
        }
        let changed = |what: &str| {
            Err(fail(
                "project-changed",
                format!(
                    "not done: the request names {what} of a project no longer open (another project, or a new opening of it, is open now)"
                ),
            ))
        };
        match message.get("generation") {
            None | Some(Value::Null) => {}
            Some(given) => {
                return match given.as_u64() {
                    Some(g) if g == self.generation => Ok(()),
                    Some(_) => changed("the generation"),
                    None => Err(fail("bad-value", "`generation` must be a whole number")),
                };
            }
        }
        match message.get("projectId") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(id)) if *id == self.project_id() => Ok(()),
            Some(Value::String(_)) => changed("the id"),
            Some(_) => Err(fail("bad-value", "`projectId` must be a string")),
        }
    }

    /// `graph.projectId` (M2.9-3): which project folder this is, opaque (the
    /// SHA-256 of its canonical path; the path itself is not sent), so a
    /// page's patch history stays with the project it was written in even
    /// when two have the same folder name and project name.
    pub(super) fn project_id(&self) -> String {
        let dir = fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone());
        sha256_hex(dir.as_os_str().as_encoded_bytes())
    }
}

impl Host {
    /// App mode: opens the project folder `dir` (stopped) for its rate,
    /// panel and diagnostics, and serves its `ui/`.
    fn open(&mut self, dir: &Path) {
        self.opening.begin(dir);
        self.idle_watch = None;
        self.server.set_project_ui(project_ui(dir));
        let state = FolderState::read(dir).ok();
        let loaded = self.load(dir);
        if let Ok((project, plan, _)) = &loaded {
            self.remember_stopped(state, project, plan);
        }
        let Some(mode) = self.app.as_mut() else {
            return;
        };
        mode.opened = true;
        mode.reload = None;
        match loaded {
            Ok((project, plan, _)) => {
                self.rate = project.sample_rate;
                mode.diagnostics.clear();
                self.panel = Panel::new(&project, &self.registry);
                self.plan = Some(plan);
            }
            Err(diagnostics) => {
                self.rate = 0;
                mode.diagnostics = diagnostics;
                self.panel = Panel::default();
                self.plan = None;
            }
        }
        // Of the panel just built (none when it does not load).
        self.refresh_warnings();
    }

    /// `project.open`. App mode: stops, opens the folder or `.signal` file at
    /// `path` (made absolute: it becomes the Setup's last project) and saves
    /// the Setup; while a `.signal` project is edited it is refused
    /// (`edited`) unless `unsaved` is `"save"` or `"discard"`. `signal ui`:
    /// only the project it serves, which is reopened ([`Host::reopen`]).
    pub(super) fn project_open(&mut self, message: &Value) -> Result<Value, Failure> {
        let path = message
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| fail("bad-message", "project.open needs `path`"));
        if self.app.is_none() {
            return self.reopen(path?);
        }
        let path = path?;
        let edits = match message.get("unsaved").and_then(Value::as_str) {
            None => Edits::Ask,
            Some("save") => Edits::Save,
            Some("discard") => Edits::Discard,
            Some(other) => {
                return Err(fail(
                    "bad-message",
                    format!("`unsaved` is \"save\" or \"discard\", not \"{other}\""),
                ));
            }
        };
        let path = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
        self.open_with(&path, edits, Why::Another)
    }

    /// App mode: opens the folder (or its `project.toml`) or the `.signal`
    /// file at `path`, as `project.open` does; the error is what to show.
    pub fn open_project(&mut self, path: &Path, edits: Edits) -> Result<(), String> {
        self.open_project_as(path, edits, Why::Another)
    }

    /// [`Host::open_project`], saying why it opens (the window's message).
    fn open_project_as(&mut self, path: &Path, edits: Edits, opening: Why) -> Result<(), String> {
        match self.open_with(path, edits, opening) {
            // As after a `project.open`: every client is told.
            Ok(_) => {
                let describe = self.describe(false);
                self.broadcast(&describe);
                self.push_graph(false);
                Ok(())
            }
            Err(failure) => {
                if let Some(mode) = self.app.as_mut() {
                    mode.message = Some(failure.message.clone());
                }
                Err(failure.message)
            }
        }
    }

    fn open_with(&mut self, path: &Path, edits: Edits, opening: Why) -> Result<Value, Failure> {
        let opened = self.open_stopped(path, edits, opening)?;
        // ADR 0033: switched on, it plays (a failure is the window's message).
        self.autoplay();
        Ok(opened)
    }

    /// [`Host::open_with`] without playing.
    fn open_stopped(&mut self, path: &Path, edits: Edits, opening: Why) -> Result<Value, Failure> {
        if self.app.is_none() {
            return Err(app_only());
        }
        // A save still running decides what is edited: finished first.
        self.finish_saves();
        // Writes waiting for the writer lock are edits not made yet (ADR
        // 0048 item 5): asked about as edits, a Save refused until they are
        // written, a Discard lets them go (each answered `project-changed`).
        // Only file writes count: a live edit, Save or open queued (behind
        // a queued `project.open`, say) is no edit of the files.
        let queued = self.opening.document.is_some() && self.queue.writes_queued() > 0;
        let edited = self.opening.document.as_mut().is_some_and(Document::check) || queued;
        if edited && edits == Edits::Ask {
            return Err(fail(
                "edited",
                format!("Save changes to {}?", self.project_name()),
            ));
        }
        // Saved first, so that reopening the same file opens what was saved.
        if edited && edits == Edits::Save {
            self.save_document().map_err(|m| fail("not-saved", m))?;
        }
        // The new project is unpacked before anything stops: a refused one
        // changes nothing.
        // A folder named `*.signal` is a folder.
        let prepared = if archive::is_project_file(path) && !path.is_dir() {
            Some(Document::open(path, &self.opening.open_root).map_err(|m| fail("archive", m))?)
        } else {
            None
        };
        if let Some(mode) = self.app.as_mut() {
            mode.message = None;
            // Opened stopped: switching on again plays nothing (with
            // `plays` it plays, ADR 0033).
            mode.power.forget();
            mode.held = false;
        }
        // Another project: the unsaved values of this one are let go, and
        // the window says so.
        let unsaved = self.unsaved().len();
        self.stop_app();
        if let Some(previous) = self.opening.document.take() {
            // Not edited, saved, or let go on request.
            previous.close(true);
        }
        let dir = match &prepared {
            Some(document) => document.working().to_owned(),
            None => folder_of(path),
        };
        self.opening.document = prepared;
        self.open(&dir);
        self.live.failed.clear();
        app::opened(&mut self.setup, path.to_owned());
        let saved = self.save_setup();
        let mode = self.app.as_mut();
        if let Some(mode) = mode {
            mode.message = match (&saved, unsaved, opening) {
                (Err(message), _, _) => Some(message.clone()),
                (Ok(()), 0, _) => None,
                (Ok(()), 1, Why::Another) => {
                    Some("1 unsaved edit of the previous project was not written".to_owned())
                }
                (Ok(()), n, Why::Another) => Some(format!(
                    "{n} unsaved edits of the previous project were not written"
                )),
                (Ok(()), 1, Why::Revert) => {
                    Some("1 unsaved edit was discarded by Revert to Saved".to_owned())
                }
                (Ok(()), n, Why::Revert) => Some(format!(
                    "{n} unsaved edits were discarded by Revert to Saved"
                )),
            };
        }
        let diagnostics = self
            .app
            .as_ref()
            .map_or_else(Vec::new, |m| m.diagnostics.clone());
        Ok(json!({
            "type": "project",
            "dir": dir.display().to_string(),
            "file": self.opening.document.as_ref().map(|d| d.file().display().to_string()),
            "loads": diagnostics.is_empty(),
            "diagnostics": diagnostics,
        }))
    }

    /// App mode: File ▸ Revert to Saved… (`project.revert`): the open
    /// `.signal` file opened again exactly as `project.open` of it with
    /// `unsaved: "discard"` opens it: a save still running is waited for,
    /// the working folder's changes and the unsaved live values are let go,
    /// the file is unpacked again into a new working folder (the old one
    /// removed) and played from frame 0. The error is what to show.
    pub fn revert_project(&mut self) -> Result<(), String> {
        let file = self.revert_file().map_err(|failure| failure.message)?;
        self.open_project_as(&file, Edits::Discard, Why::Revert)
    }

    /// `project.revert`: [`Host::revert_project`] for a client.
    pub(super) fn project_revert(&mut self) -> Result<Value, Failure> {
        let file = self.revert_file()?;
        self.open_with(&file, Edits::Discard, Why::Revert)
    }

    /// The file Revert to Saved… opens again: the open project's `.signal`
    /// file, looked at after every save still running has finished (a Save
    /// As in flight changes which file that is). Refused outside app mode
    /// (`not-allowed`), without a project (`no-project`), for a folder
    /// project (`no-file`), which Signal writes straight to its files, when
    /// there is nothing to revert (`not-edited`: [`ProjectStatus::revertible`],
    /// the menu item's test), and when the path is no longer a file
    /// (`archive`: removed, or a folder in its place). Refused, nothing
    /// changes: the working folder keeps its edits.
    fn revert_file(&mut self) -> Result<PathBuf, Failure> {
        let Some(mode) = self.app.as_ref() else {
            return Err(app_only());
        };
        if !mode.opened {
            return Err(fail("no-project", "no project is open"));
        }
        if self.opening.document.is_none() {
            return Err(fail(
                "no-file",
                format!(
                    "{} is a folder project: Signal writes it straight to its files, so there is no saved version to revert to",
                    self.project_name()
                ),
            ));
        }
        let status = self.project_status();
        let Some(file) = status.as_ref().and_then(|s| s.file.clone()) else {
            return Err(fail("no-file", "the project has no file to revert to"));
        };
        if !status.as_ref().is_some_and(ProjectStatus::revertible) {
            return Err(fail(
                "not-edited",
                format!(
                    "{} has no changes since it was last saved or opened: there is nothing to revert",
                    self.project_name()
                ),
            ));
        }
        if !file.is_file() {
            return Err(fail(
                "archive",
                format!(
                    "{} is no longer a file: nothing was reverted, and the changes are kept",
                    file.display()
                ),
            ));
        }
        Ok(file)
    }

    /// App mode, at start: opens `path`, a folder or a `.signal` file.
    pub(super) fn open_path(&mut self, path: &Path) -> Result<(), String> {
        if archive::is_project_file(path) && !path.is_dir() {
            let document = Document::open(path, &self.opening.open_root)?;
            let dir = document.working().to_owned();
            self.opening.document = Some(document);
            self.open(&dir);
        } else {
            self.open(&folder_of(path));
        }
        Ok(())
    }

    /// The open project's name: its file's stem, or its folder's name.
    /// The project folder's name, telling a client's drawing of one project
    /// from a `graph.error` of another (`graph`, `graph.error`).
    pub(super) fn folder_name(&self) -> Option<String> {
        self.opening
            .dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }

    pub(super) fn project_name(&self) -> String {
        match &self.opening.document {
            Some(document) => document.name(),
            None => project_name(&self.opening.dir),
        }
    }

    /// App mode: the open project as the window shows it, its edited state
    /// and unsaved count looked at now (after a save still running, which
    /// it waits for: the window asks before a decision, the "Save changes?"
    /// and "Revert to Saved?" prompts); `None` without one.
    pub fn project_status(&mut self) -> Option<ProjectStatus> {
        self.finish_saves();
        if let Some(document) = self.opening.document.as_mut() {
            document.check();
        }
        let unsaved = self.unsaved().len();
        self.project_shown()
            .map(|status| ProjectStatus { unsaved, ..status })
    }

    /// App mode: the open project as last looked at (each step looks at
    /// most every 250 ms); `None` without one.
    pub fn project_shown(&self) -> Option<ProjectStatus> {
        if !self.app.as_ref().is_some_and(|m| m.opened) {
            return None;
        }
        Some(ProjectStatus {
            dir: self.opening.dir.clone(),
            file: self.opening.document.as_ref().map(|d| d.file().to_owned()),
            name: self.project_name(),
            edited: self.opening.document.as_ref().is_some_and(Document::edited),
            unsaved: self.unsaved_count(),
        })
    }

    /// `signal ui`'s `project.open`: only the project it serves (its folder,
    /// its `project.toml`, or the `.signal` file it was started with), which
    /// is played again from frame 0 from its files: a `.signal` file is
    /// unpacked again into a new temporary folder. Anything else is refused
    /// (`docs/ui-protocol.md` "Security notes").
    fn reopen(&mut self, path: &str) -> Result<Value, Failure> {
        let asked = Path::new(path);
        let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        let served = same(asked, &self.opening.source)
            || (asked.file_name().is_some_and(|n| n == PROJECT_FILE)
                && asked
                    .parent()
                    .is_some_and(|p| same(p, &self.opening.source)));
        if !served {
            return Err(app_only());
        }
        if self.recording {
            return Err(fail(
                "recording",
                "a run that records a session or its audio is not reopened (a recording covers one run from frame 0)",
            ));
        }
        let temporary =
            match &self.opening.temporary {
                Some(_) => Some(TempProject::unpack(&self.opening.source).map_err(|e| {
                    fail("archive", format!("{}: {e}", self.opening.source.display()))
                })?),
                None => None,
            };
        if let Some(old) = self.run.take() {
            stop(old);
        }
        self.abandon();
        self.live.failed.clear();
        if let Some(temporary) = temporary {
            self.opening.begin(temporary.dir());
            // The previous copy is removed once its run is stopped.
            self.opening.temporary = Some(temporary);
            self.server.set_project_ui(project_ui(&self.opening.dir));
        }
        let built = self.build(true);
        let error = built.as_ref().err().cloned();
        match built {
            Ok((run, notes)) => {
                self.install(run);
                self.rearm();
                self.messages.extend(notes);
            }
            Err(_) => self.build_failed(),
        }
        Ok(json!({
            "type": "project",
            "dir": self.opening.dir.display().to_string(),
            "file": self.opening.temporary.as_ref().map(|_| self.opening.source.display().to_string()),
            "loads": error.is_none(),
            "diagnostics": error.into_iter().collect::<Vec<_>>(),
        }))
    }
}
