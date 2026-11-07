//! Saving the open project (app mode): Save, Save As, the window layout's
//! write and `project.save`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use signal_project::archive;
use signal_project::transaction;

use crate::app;
use crate::document::{Document, Look, Ticket};

use super::app_mode::app_only;
use super::failure::{Failure, fail};
use super::queue::{Request, WhenWaiting};
use super::writer::WriteCtx;
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

/// A folder project's Save As packing on a thread of its own (review
/// 2026-11-01, `control-and-realtime-cost`: it packed on the control
/// thread): the new file, its id, and what the thread reports, the outcome
/// and the folder's look when it packed (packed again while the folder
/// changed under it).
pub(super) struct FolderSave {
    file: PathBuf,
    id: String,
    result: std::sync::mpsc::Receiver<(Result<(), String>, Look)>,
    done: SaveDone,
}

/// The most times a folder Save As packs again because the folder changed
/// while it packed.
const FOLDER_SAVE_TRIES: usize = 4;

/// How a pending folder Save As is collected.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FolderCollect {
    /// If it finished: the new file opened, `done` answered (a step).
    Poll,
    /// Waited for, then as `Poll` (the blocking Save As).
    Wait,
    /// Waited for until the file is written, `done` answered, and nothing
    /// opened or played: the project is closing or another one opening
    /// (review of c24cf1e: opening it then played audio during shutdown,
    /// or opened two projects).
    WaitWithoutOpening,
}

/// A cheap look at the files a pack takes from `dir` (paths, sizes, change
/// times): what a hidden file, `.git` or an editor's swap file does not
/// change (review of c24cf1e: they forced a pack again).
fn folder_look(dir: &Path) -> Look {
    archive::project_file_paths(dir)
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let meta = fs::metadata(&path).ok();
            let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
            let changed = meta.and_then(|m| m.modified().ok());
            (path, size, changed)
        })
        .collect()
}

/// Packs `dir` with project id `id` into `file`, again while its files
/// change under the packing (at most [`FOLDER_SAVE_TRIES`] times); the
/// outcome and the look the bytes were packed at. Its own thread, so it may
/// wait for the project's writer lock (ADR 0040): Signal's own writers never
/// commit half of a transaction under the pack (review of c24cf1e, P3), and
/// the looks still catch an editor outside Signal.
fn pack_folder(ctx: &WriteCtx, dir: &Path, file: &Path, id: &str) -> (Result<(), String>, Look) {
    let mut last = folder_look(dir);
    for _ in 0..FOLDER_SAVE_TRIES {
        let packed = match ctx.lock() {
            Ok(_lock) => archive::pack_identified(dir, Some(id)),
            Err(error) => {
                return (
                    Err(format!(
                        "{} was not saved: the project's writer lock: {error}",
                        file.display()
                    )),
                    last,
                );
            }
        };
        let now = folder_look(dir);
        if now == last {
            let saved = packed
                .and_then(|bytes| archive::write_atomic(file, &bytes))
                .map_err(|e| format!("{} was not saved: {e}", file.display()));
            return (saved, now);
        }
        last = now;
    }
    (
        Err(format!(
            "{} was not saved: the project's files kept changing while it was packed",
            file.display()
        )),
        last,
    )
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
        self.submit(Request::Windows {
            dir: dir.to_owned(),
            edit,
            done,
        });
    }

    /// [`Host::write_windows_then`]'s write, now.
    pub(super) fn write_windows_now(
        &mut self,
        ctx: &WriteCtx,
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
        let _lock = ctx.lock().map_err(|e| failed(e.to_string()))?;
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
        let (answer, answered) = std::sync::mpsc::channel();
        self.submit(Request::Save {
            save_as: None,
            when_waiting: WhenWaiting::Refuse,
            done: Box::new(move |result| {
                let _ = answer.send(result);
            }),
        });
        // Refused or written before `submit` returns.
        answered
            .try_recv()
            .unwrap_or_else(|_| Err("not saved".to_owned()))
    }

    /// [`Host::save_document`], refused while requests wait for the writer
    /// lock: nothing saved, `done` told why.
    pub(super) fn save_document_refused(&mut self, done: SaveDone) {
        self.answer_saves();
        done(Err(format!(
            "not saved: {} request(s) (file writes or Saves) wait for another writer to release the project's lock; try again once they are done",
            self.queue.waiting.len()
        )));
    }

    /// [`Host::save_document`], now, waiting for the write.
    pub(super) fn save_document_here(&mut self, ctx: &WriteCtx, done: SaveDone) {
        if let Err(error) = self.correct_before_save(ctx) {
            self.answer_saves();
            return done(Err(error));
        }
        let result = match self.opening.document.as_mut() {
            Some(document) => document.save(),
            None => Ok(()),
        };
        self.answer_saves();
        done(result);
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
        self.submit(Request::Save {
            save_as: None,
            when_waiting: WhenWaiting::Queue,
            done,
        });
    }

    /// [`Host::save_document_then`], now.
    pub(super) fn save_document_now(&mut self, ctx: &WriteCtx, done: SaveDone) {
        if let Err(error) = self.correct_before_save(ctx) {
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
        self.folder_save.is_some() || self.opening.document.as_ref().is_some_and(Document::saving)
    }

    /// Save As (app mode), waiting for it: the project written to the
    /// `.signal` file `file`, which it is from then on (the Setup's last
    /// project). A folder project's folder is left as it is: the project is
    /// opened again, from the new file, stopped. Refused, nothing saved,
    /// while requests wait for the writer lock, as [`Host::save_document`]
    /// is: the answer is always this Save As's own outcome, never a job
    /// still to run.
    pub fn save_as(&mut self, file: &Path) -> Result<(), String> {
        let (answer, answered) = std::sync::mpsc::channel();
        self.submit(Request::Save {
            save_as: Some(file.to_owned()),
            when_waiting: WhenWaiting::Refuse,
            done: Box::new(move |result| {
                let _ = answer.send(result);
            }),
        });
        // Refused or written before `submit` returns, or started then and
        // finished here (a folder's: the new file opened).
        self.collect_folder_save(FolderCollect::Wait);
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
        self.submit(Request::Save {
            save_as: Some(file.to_owned()),
            when_waiting: WhenWaiting::Queue,
            done,
        });
    }

    /// [`Host::save_as_then`], now.
    pub(super) fn save_as_now(&mut self, ctx: &WriteCtx, file: &Path, done: SaveDone) {
        if let Err(error) = self.correct_before_save(ctx) {
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
                if self.folder_save.is_some() {
                    return done(Err(format!(
                        "{} was not saved: another Save As is being written",
                        file.display()
                    )));
                }
                // The new file's own id, and the folder's machine-local
                // state copied to it (ADR 0038 §6, amendment of
                // 2026-11-01); the folder keeps its own. Packed on a
                // thread of its own; a later step opens the new file.
                let id = crate::local_state::fresh_id(file);
                self.start_folder_save(file.to_owned(), id, done);
            }
        }
    }

    /// Waits for the document's saves (and a folder's Save As, its file
    /// written but not opened: what calls this closes or opens a project)
    /// to finish, and answers them.
    pub(super) fn finish_saves(&mut self) {
        if let Some(document) = self.opening.document.as_mut() {
            document.finish_saves();
        }
        self.collect_folder_save(FolderCollect::WaitWithoutOpening);
        self.answer_saves();
    }

    /// Starts a folder project's Save As to `file` (id `id`) on a thread of
    /// its own; `done` is called by the step that sees it written.
    fn start_folder_save(&mut self, file: PathBuf, id: String, done: SaveDone) {
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = self.opening.dir.clone();
        let ctx = self.write_ctx();
        let target = file.clone();
        let key = id.clone();
        let spawned = std::thread::Builder::new()
            .name("signal-save-as".to_owned())
            .spawn(move || {
                let _ = tx.send(pack_folder(&ctx, &dir, &target, &key));
            });
        if let Err(error) = spawned {
            return done(Err(format!("{} was not saved: {error}", file.display())));
        }
        self.folder_save = Some(FolderSave {
            file,
            id,
            result: rx,
            done,
        });
    }

    /// A folder Save As finished (`how`: polled or waited for): its state
    /// copied and the new file opened (not with
    /// [`FolderCollect::WaitWithoutOpening`]), unless the folder changed
    /// since it was packed, which packs it again, at most
    /// [`FOLDER_SAVE_TRIES`] times.
    pub(super) fn collect_folder_save(&mut self, how: FolderCollect) {
        for _ in 0..=FOLDER_SAVE_TRIES {
            let Some(save) = self.folder_save.as_ref() else {
                return;
            };
            let outcome = if how == FolderCollect::Poll {
                match save.result.try_recv() {
                    Ok(outcome) => Some(outcome),
                    Err(std::sync::mpsc::TryRecvError::Empty) => return,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
                }
            } else {
                save.result.recv().ok()
            };
            let Some(save) = self.folder_save.take() else {
                return;
            };
            let (saved, packed_at) = outcome.unwrap_or_else(|| {
                (
                    Err(format!(
                        "{} was not saved: the save ended without a result",
                        save.file.display()
                    )),
                    Vec::new(),
                )
            });
            if saved.is_ok() && folder_look(&self.opening.dir) != packed_at {
                // Edited after it was packed: the file must hold the edit.
                self.start_folder_save(save.file, save.id, save.done);
                if how == FolderCollect::Poll {
                    return;
                }
                continue;
            }
            if saved.is_ok() {
                self.copy_folder_state(&save.file, &save.id);
            }
            let file = save.file;
            let result = if how == FolderCollect::WaitWithoutOpening {
                if saved.is_ok() {
                    app::opened(&mut self.setup, file.clone());
                }
                saved
            } else {
                saved.and_then(|()| self.open_project(&file, Edits::Discard))
            };
            return (save.done)(result);
        }
        // Still changing after every pack: answered, not packed again.
        if let Some(save) = self.folder_save.take() {
            (save.done)(Err(format!(
                "{} was not saved: the project's files kept changing while it was packed",
                save.file.display()
            )));
        }
    }

    /// Answers the document's saves that finished: a Save As written is the
    /// Setup's last project from then on; a client's `project.save` is
    /// answered `saved` (or `not-saved`) and is the window's message, as an
    /// app message's outcome is.
    pub(super) fn answer_saves(&mut self) {
        self.collect_folder_save(FolderCollect::Poll);
        let Some(document) = self.opening.document.as_mut() else {
            return;
        };
        let finished = document.take_saved();
        let saved_any = finished.iter().any(|(_, result)| result.is_ok());
        // A Save As written: this machine's state follows the new file.
        self.follow_saved_id();
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
                        self.save_setup()
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
        ctx: &WriteCtx,
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
        self.correct_before_save(ctx)
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
