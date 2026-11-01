//! Saving the open project (app mode): Save, Save As, the window layout's
//! write and `project.save`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use signal_project::archive;
use signal_project::transaction::{self, ProjectLock};

use crate::app;
use crate::document::{Document, Ticket};

use super::app_mode::app_only;
use super::failure::{Failure, fail};
use super::queue::Job;
use super::{Edits, Host, project_ui};

/// An edit of `ui/windows.toml`'s text ([`Host::write_windows_then`]).
pub type WindowsEdit = Box<dyn Fn(&str) -> Result<String, String> + Send>;

/// What a finished save of the document is told to: Signal.app's window
/// (its answer), or the client that sent `project.save` (`saved`, or
/// `not-saved`, with its message's id).
pub type SaveDone = Box<dyn FnOnce(Result<(), String>) + Send>;

enum SaveWaiter {
    Client { client: u64, id: Value },
    Call(SaveDone),
}

/// A save of the document not yet answered: its ticket, the file of a Save
/// As (recorded in the Setup when written), and whom to tell.
pub(super) struct SaveWait {
    ticket: Ticket,
    save_as: Option<PathBuf>,
    waiter: SaveWaiter,
}

impl Host {
    /// App mode (ADR 0032): `ui/windows.toml` of the project folder `dir`
    /// edited by `edit` (given the file's text, empty when it is missing),
    /// written atomically through a hidden temporary file, and taken as
    /// seen by the watch, so it reloads nothing; `done` is called with the
    /// outcome. Refused when `dir` is no longer the open project (the layout
    /// was of another one), and when `ui/` or the file is a link (only the
    /// project's own files are written); nothing is written then. Written
    /// at once when the project's writer lock is free; otherwise it waits
    /// in the host's queue (a later step calls `done`), never on the control
    /// thread (ADR 0048).
    pub fn write_windows_then(&mut self, dir: &Path, edit: WindowsEdit, done: SaveDone) {
        let _no_wait = transaction::NoWait::enter(Duration::ZERO);
        if self.queue.waiting.is_empty() {
            match ProjectLock::try_hold(&self.opening.dir) {
                Ok(Some(holding)) => {
                    done(self.write_windows_now(dir, edit));
                    drop(holding);
                    return;
                }
                Ok(None) => {}
                Err(_) => return done(self.write_windows_now(dir, edit)),
            }
        }
        self.wait_for_lock(Job::Windows {
            dir: dir.to_owned(),
            edit,
            done,
        });
    }

    /// [`Host::write_windows_then`]'s write, now.
    pub(super) fn write_windows_now(
        &mut self,
        dir: &Path,
        edit: impl Fn(&str) -> Result<String, String>,
    ) -> Result<(), String> {
        let file = Path::new(crate::WINDOWS_FILE);
        if self.opening.dir != dir || self.app.as_ref().is_some_and(|m| !m.opened) {
            return Err(format!(
                "{} was not written: {} is not the open project",
                file.display(),
                dir.display()
            ));
        }
        let path = self.opening.dir.join(file);
        let link =
            |path: &Path| fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        if link(&project_ui(&self.opening.dir)) || link(&path) {
            return Err(format!(
                "{} was not written: it is reached through a link, and the layout is written only into the project's own files",
                file.display()
            ));
        }
        let failed = |error: String| format!("{} was not written: {error}", path.display());
        let _lock = ProjectLock::acquire(&self.opening.dir).map_err(|e| failed(e.to_string()))?;
        // Edited again if it changes before the rename (ADR 0040).
        let written = transaction::edit_text(&path, |before| {
            edit(before).map(|after| (after != before).then_some(after))
        })
        .map_err(failed)?;
        if written {
            self.seen(file);
        }
        Ok(())
    }

    /// Save of a `.signal` project, waiting for it: the working folder
    /// written back to the file atomically. A folder project has nothing to
    /// write here (its parameters are written by `retry`). The control
    /// thread's Save is [`Host::save_document_then`]. Refused, nothing
    /// saved, while requests wait for the writer lock (ADR 0048 item 5): a
    /// Save made now would leave out writes asked before it, and an Open
    /// after it would then refuse them as another project's.
    pub fn save_document(&mut self) -> Result<(), String> {
        if self.opening.document.is_some() && !self.queue.waiting.is_empty() {
            self.answer_saves();
            return Err(format!(
                "not saved: {} request(s) wait for another writer to release the project's lock; try again once they are written",
                self.queue.waiting.len()
            ));
        }
        if let Err(error) = self.correct_before_save() {
            self.answer_saves();
            return Err(error);
        }
        let result = match self.opening.document.as_mut() {
            Some(document) => document.save(),
            None => Ok(()),
        };
        self.answer_saves();
        result
    }

    /// Save of a `.signal` project on a thread of its own (the window's
    /// Save): `done` is called with its result by a later step, when it is
    /// written; the control thread goes on meanwhile. A folder project's
    /// is called at once.
    ///
    /// Requests waiting for the writer lock are done first (ADR 0048): the
    /// Save waits behind them, as it does when its corrections need the
    /// lock another writer holds; `done` is then called by a later step.
    pub fn save_document_then(&mut self, done: SaveDone) {
        let _no_wait = transaction::NoWait::enter(Duration::ZERO);
        if !self.queue.waiting.is_empty() || self.save_must_wait() {
            return self.wait_for_lock(Job::Save {
                save_as: None,
                done,
            });
        }
        self.save_document_now(done);
    }

    /// [`Host::save_document_then`], now.
    pub(super) fn save_document_now(&mut self, done: SaveDone) {
        if let Err(error) = self.correct_before_save() {
            return done(Err(error));
        }
        match self.opening.document.as_mut() {
            Some(document) => {
                let ticket = document.start_save();
                self.saves.push(SaveWait {
                    ticket,
                    save_as: None,
                    waiter: SaveWaiter::Call(done),
                });
            }
            None => done(Ok(())),
        }
    }

    /// Whether a save of the document is running or waiting to start (the
    /// control loop then steps rather than sleeping in a wait).
    pub fn saving(&self) -> bool {
        self.opening.document.as_ref().is_some_and(Document::saving)
    }

    /// Save As (app mode), waiting for it: the project written to the
    /// `.signal` file `file`, which it is from then on (the Setup's last
    /// project). A folder project's folder is left as it is: the project is
    /// opened again, from the new file, stopped.
    pub fn save_as(&mut self, file: &Path) -> Result<(), String> {
        let (answer, answered) = std::sync::mpsc::channel();
        self.save_as_then(
            file,
            Box::new(move |result| {
                let _ = answer.send(result);
            }),
        );
        self.finish_saves();
        answered
            .try_recv()
            .unwrap_or_else(|_| Err(format!("{} was not saved", file.display())))
    }

    /// Save As (app mode) of a `.signal` project on a thread of its own, as
    /// [`Host::save_document_then`]; the Setup records the new file when it
    /// is written. A folder project is packed and opened again from the new
    /// file at once (opening stops the run anyway), and `done` called.
    /// Requests waiting for the writer lock are done first, as for
    /// [`Host::save_document_then`].
    pub fn save_as_then(&mut self, file: &Path, done: SaveDone) {
        if self.app.is_none() {
            return done(Err(app_only().message));
        }
        let _no_wait = transaction::NoWait::enter(Duration::ZERO);
        if !self.queue.waiting.is_empty() || self.save_must_wait() {
            return self.wait_for_lock(Job::Save {
                save_as: Some(file.to_owned()),
                done,
            });
        }
        self.save_as_now(file, done);
    }

    /// [`Host::save_as_then`], now.
    pub(super) fn save_as_now(&mut self, file: &Path, done: SaveDone) {
        if let Err(error) = self.correct_before_save() {
            return done(Err(error));
        }
        match self.opening.document.as_mut() {
            Some(document) => {
                let ticket = document.start_save_as(file);
                self.saves.push(SaveWait {
                    ticket,
                    save_as: Some(file.to_owned()),
                    waiter: SaveWaiter::Call(done),
                });
            }
            None => {
                let saved = archive::pack(&self.opening.dir)
                    .and_then(|bytes| archive::write_atomic(file, &bytes))
                    .map_err(|e| format!("{} was not saved: {e}", file.display()));
                done(saved.and_then(|()| self.open_project(file, Edits::Discard)));
            }
        }
    }

    /// Waits for the document's saves to finish, and answers them.
    pub(super) fn finish_saves(&mut self) {
        if let Some(document) = self.opening.document.as_mut() {
            document.finish_saves();
        }
        self.answer_saves();
    }

    /// Answers the document's saves that finished: a Save As written is the
    /// Setup's last project from then on; a client's `project.save` is
    /// answered `saved` (or `not-saved`) and is the window's message, as an
    /// app message's outcome is.
    pub(super) fn answer_saves(&mut self) {
        let Some(document) = self.opening.document.as_mut() else {
            return;
        };
        let finished = document.take_saved();
        let saved_any = finished.iter().any(|(_, result)| result.is_ok());
        for (ticket, result) in finished {
            let (answered, waiting): (Vec<SaveWait>, Vec<SaveWait>) =
                std::mem::take(&mut self.saves)
                    .into_iter()
                    .partition(|wait| wait.ticket == ticket);
            self.saves = waiting;
            for wait in answered {
                // The file this save wrote, whatever the document's file is
                // by now (a Save As asked after it may have written since).
                let written = result.as_ref().ok().cloned();
                let result = match (&result, &wait.save_as) {
                    (Ok(_), Some(file)) => {
                        app::opened(&mut self.setup, file.clone());
                        app::save(&self.setup, &self.setup_path)
                    }
                    (Ok(_), None) => Ok(()),
                    (Err(message), _) => Err(message.clone()),
                };
                match wait.waiter {
                    SaveWaiter::Call(done) => done(result),
                    SaveWaiter::Client { client, id } => {
                        let reply = self.noted(result.map_err(|m| fail("not-saved", m)));
                        let reply = match reply {
                            Ok(()) => json!({
                                "type": "saved",
                                "id": id,
                                "file": written.map(|file| file.display().to_string()),
                            }),
                            Err(failure) => json!({
                                "type": "error",
                                "id": id,
                                "code": failure.code,
                                "message": failure.message,
                            }),
                        };
                        self.send_text(client, &reply);
                    }
                }
            }
        }
        // ADR 0042 §3: saved, though the corrected values were not written
        // first (they stay pending, and the warnings still count them).
        if saved_any
            && let Some(mode) = self.app.as_mut()
            && let Some(error) = mode.correction_error.take()
        {
            mode.message = Some(format!("saved; {error}"));
        }
    }

    /// `project.save` (app mode): a `.signal` project's working folder
    /// written back to its file on a thread of its own; answered (`saved`,
    /// or `not-saved`) when it is written, other messages being answered
    /// meanwhile. A folder project's is answered at once.
    pub(super) fn project_save(
        &mut self,
        client: u64,
        id: &Value,
    ) -> Result<Option<Value>, Failure> {
        if self.app.is_none() {
            return Err(app_only());
        }
        if self.opening.document.is_none() {
            self.noted(Ok::<(), Failure>(()))?;
            return Ok(Some(json!({"type": "saved", "file": null})));
        }
        self.correct_before_save()
            .map_err(|error| fail("not-saved", error))?;
        let Some(document) = self.opening.document.as_mut() else {
            return Ok(Some(json!({"type": "saved", "file": null})));
        };
        let ticket = document.start_save();
        self.saves.push(SaveWait {
            ticket,
            save_as: None,
            waiter: SaveWaiter::Client {
                client,
                id: id.clone(),
            },
        });
        Ok(None)
    }
}
