//! `patch` (M2.9-3): structural edits of the project's files, and the
//! presentation files that follow them.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use signal_project::patch::{self, Op};
use signal_project::schematic::SCHEMATIC_FILE;
use signal_project::session::sink_of;
use signal_project::snapshot::{self as snapshots, SNAPSHOTS_FILE, SnapshotError};
use signal_project::transaction::{self, ProjectLock};
use signal_project::{Diagnostic, PROJECT_FILE, Project, located};

use crate::bindings::{MAIN_PANEL, panel_content};
use crate::panel_write::sha256_hex;
use crate::server::MAX_MESSAGE;

use super::failure::{Failure, fail, fail_with};
use super::writer::WriteCtx;
use super::{Host, project_ui, with};

/// ADR 0037, editable insides: what a patch's other files that could not be
/// put back say (`"; composites/x.toml could not be put back (…): it holds
/// the patch"`), empty when every one was.
pub(super) fn unrestored(results: &[(String, std::io::Result<bool>)]) -> String {
    results
        .iter()
        .filter_map(|(path, result)| match result {
            Ok(true) => None,
            Ok(false) => Some(format!(
                "; {path} could not be put back (another writer changed it meanwhile; its edit is kept)"
            )),
            Err(error) => Some(format!(
                "; {path} could not be put back ({error}): it holds the patch"
            )),
        })
        .collect()
}

impl Host {
    /// `patch` (ADR 0026 §6, M2.9-3, `docs/ui-protocol.md` "patch"): `ops`
    /// written into one level's file (and positions into
    /// `ui/schematic.toml`) as text, after the same load and compile as a
    /// reload, atomically; the watch's reload then applies it live (stopped,
    /// `graph` is pushed at once). Nothing is written when any step refuses.
    pub(super) fn patch(&mut self, writer: &WriteCtx, message: &Value) -> Result<Value, Failure> {
        if self.app.as_ref().is_some_and(|m| !m.opened) {
            return Err(fail("no-project", "open a project folder first"));
        }
        let text = |key: &str| {
            message
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| fail("bad-message", format!("patch needs `{key}`")))
        };
        let (level, base) = (text("level")?, text("base")?);
        let ops: Vec<Op> = match message.get("ops") {
            Some(ops @ Value::Array(list)) if !list.is_empty() => {
                serde_json::from_value(ops.clone())
                    .map_err(|e| fail("bad-message", format!("`ops`: {e}")))?
            }
            _ => return Err(fail("bad-message", "patch needs `ops`, a list of ops")),
        };
        // ADR 0046 §8: what an untrusted `panel.restore` may write.
        for op in &ops {
            if let Op::PanelRestore {
                file,
                text: Some(text),
                ..
            } = op
            {
                self.panel_restore_allowed(file, text)?;
            }
        }
        let sources = signal_project::read_sources(&self.opening.dir)
            .map_err(|e| fail("does-not-load", e.to_string()))?;
        // The level's file is one the project read: never a path from the client.
        let file = if level.is_empty() {
            PROJECT_FILE.to_owned()
        } else {
            format!("composites/{level}.toml")
        };
        let unknown = || {
            fail(
                "unknown-level",
                format!("the project has no composite `{level}`"),
            )
        };
        let index = sources
            .iter()
            .position(|s| s.path == file)
            .ok_or_else(unknown)?;
        if self.linked_file(&file) {
            return Err(fail(
                "unknown-level",
                format!(
                    "{file} is reached through a link: a patch writes only the project's own files"
                ),
            ));
        }
        let before = sources
            .get(index)
            .map(|s| s.text.clone())
            .ok_or_else(unknown)?;
        let current = sha256_hex(before.as_bytes());
        if current != base {
            return Err(fail_with(
                "conflict",
                format!("{file} changed since the version the patch names"),
                json!({"level": level, "file": file, "base": current}),
            ));
        }
        let positions_path = self.opening.dir.join(SCHEMATIC_FILE);
        let positions = match fs::read_to_string(&positions_path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(fail("write-failed", format!("{SCHEMATIC_FILE}: {error}"))),
        };
        // The project as its files now say, the running one when it is that.
        let parsed;
        let loaded = match &self.run {
            Some(run) if run.current.files == sources => Some(&run.current),
            _ => {
                parsed = Project::from_sources_in(sources.clone(), &self.opening.dir).ok();
                parsed.as_ref()
            }
        };
        let registry = &self.registry;
        let order = |module: &str| -> Option<Vec<String>> {
            // A built-in library composite (`lib.<name>`, ADR 0027 §4): its
            // exported parameters (no project file can take its name).
            if let Some(order) = signal_project::library_parameters(module) {
                return Some(order);
            }
            if let Some(t) = registry.get(module) {
                return Some(
                    t.descriptor
                        .parameters
                        .iter()
                        .map(|p| p.name.to_owned())
                        .collect(),
                );
            }
            sources
                .iter()
                .any(|s| s.path == format!("composites/{module}.toml"))
                .then(|| {
                    loaded
                        .and_then(|p| p.composites.iter().find(|c| c.name == module))
                        .map(|c| {
                            c.exports
                                .parameters
                                .iter()
                                .map(|e| e.name.clone())
                                .collect()
                        })
                        .unwrap_or_default()
                })
        };
        // ADR 0037: the slot ops' templates, and what a content offers (a
        // Replace keeps the `map` and `set` entries it accepts).
        let templates = signal_project::interface_templates(&sources);
        let dir = self.opening.dir.clone();
        let content = |load: &str| signal_project::content_names(loaded, &dir, load, registry);
        // ADR 0037, editable insides: a slot's own inside, the patch's other
        // file; ADR 0046: a slot's part and the panels (`ui/…`), read now.
        let file_of = |path: &str| {
            if path.starts_with("ui/") {
                return self.ui_text(path);
            }
            sources
                .iter()
                .find(|s| s.path == path)
                .map(|s| s.text.clone())
        };
        let source = |load: &str| signal_project::inside::source(&sources, load, registry);
        // ADR 0046 §8: what the host's panels bind on an instance of the level.
        let ui_dir = project_ui(&self.opening.dir);
        let level_paths: Vec<String> = if level.is_empty() {
            vec![String::new()]
        } else {
            loaded.map(|p| p.instances_of(level)).unwrap_or_default()
        };
        let bound = |name: &str| crate::parts::bound_on(&ui_dir, &level_paths, name);
        let ctx = patch::Context {
            level,
            module: &order,
            templates: &templates,
            content: &content,
            file: &file_of,
            source: &source,
            bound: &bound,
        };
        let mut patched = patch::apply(&before, positions.as_deref(), &ops, &ctx)
            .map_err(|e| fail(e.code, e.message))?;
        // ADR 0046 §7–8: the panels' side (controls moved into a part, a
        // part's controls on removed ports), more of the patch's files.
        self.part_edits(level, &ops, &before, &templates, &mut patched)?;
        let mut edited = sources.clone();
        if let Some(source) = edited.get_mut(index) {
            source.text = patched.text.clone();
        }
        for change in &patched.files {
            if self.linked_file(&change.path) {
                return Err(fail(
                    "write-failed",
                    format!(
                        "{} is reached through a link: a patch writes only the project's own files",
                        change.path
                    ),
                ));
            }
            // Presentation: no project source.
            if change.path.starts_with("ui/") {
                continue;
            }
            edited.retain(|s| s.path != change.path);
            if let Some(text) = &change.after {
                edited.push(signal_project::SourceFile {
                    path: change.path.clone(),
                    text: text.clone(),
                });
            }
        }
        let new = Project::from_sources_in(edited.clone(), &self.opening.dir);
        // The edited project when the current one does not load: a level's
        // edit leaves its exports and the paths of its instances as they were,
        // unless it edits the exports too (ADR 0037, editable insides: an
        // export op), when the edited one is judged if it loads.
        let edits_exports = ops.iter().any(|op| {
            matches!(
                op,
                Op::ExportConnect { .. } | Op::ExportDisconnect { .. } | Op::ExportRestore { .. }
            )
        });
        let judged = if edits_exports {
            new.as_ref().ok().or(loaded)
        } else {
            loaded.or(new.as_ref().ok())
        };
        let references = self.references(level, &patch::removed_instances(&ops), judged);
        if !references.is_empty() {
            let named: Vec<String> = references
                .iter()
                .map(|r| r["text"].as_str().unwrap_or_default().to_owned())
                .collect();
            return Err(fail_with(
                "in-use",
                format!("not removed: in use by {}", named.join("; ")),
                json!({"references": references}),
            ));
        }
        // ADR 0037 §2: where a renamed slot port's bindings are: the level's
        // instances' paths (the top level's is "").
        let instance_paths: Vec<String> = if patched.renamed.is_empty() {
            Vec::new()
        } else if level.is_empty() {
            vec![String::new()]
        } else {
            loaded
                .or(new.as_ref().ok())
                .map(|p| p.instances_of(level))
                .unwrap_or_default()
        };
        // The edited project, loaded and compiled as a reload would.
        let does_not_load = |diagnostics: Vec<Diagnostic>| {
            let lines: Vec<String> = located(&self.opening.dir, diagnostics)
                .iter()
                .map(ToString::to_string)
                .collect();
            fail_with(
                "does-not-load",
                lines.join("\n"),
                json!({"diagnostics": lines}),
            )
        };
        self.loads_as_edit(new).map_err(does_not_load)?;
        // Every written patch stays undoable: its undo and redo requests fit
        // a client message, with room for every parameter its instances
        // could be committed with meanwhile.
        let margin: usize = ops
            .iter()
            .chain(&patched.inverse)
            .filter_map(|op| match op {
                Op::InstanceAdd { module, .. } => order(module),
                _ => None,
            })
            .flatten()
            .map(|name| name.len() + 32)
            .sum();
        let request = |ops: &[Op]| {
            let message = json!({"type": "patch", "id": u64::MAX, "level": level, "base": "0".repeat(64), "ops": ops});
            message.to_string().len() + margin
        };
        // The undo is tried on the files as the patch leaves them.
        let edited_file = |path: &str| {
            if path.starts_with("ui/") {
                return match patched.files.iter().find(|c| c.path == path) {
                    Some(change) => change.after.clone(),
                    None => self.ui_text(path),
                };
            }
            edited
                .iter()
                .find(|s| s.path == path)
                .map(|s| s.text.clone())
        };
        let undo_ctx = patch::Context {
            file: &edited_file,
            ..ctx
        };
        let redo = patch::apply(
            &patched.text,
            patched.positions.as_deref(),
            &patched.inverse,
            &undo_ctx,
        )
        .map_err(|e| {
            fail(
                "write-failed",
                format!("not written: the patch could not be undone ({e})"),
            )
        })?;
        let size = request(&patched.inverse).max(request(&redo.inverse));
        if size > MAX_MESSAGE {
            return Err(fail(
                "bad-value",
                format!(
                    "not written: undoing this patch needs a message of {size} bytes, over the {} KiB limit",
                    MAX_MESSAGE >> 10
                ),
            ));
        }
        if patched.positions_changed {
            self.schematic_unlinked()?;
        }
        // The file as it is now, not as it was read: an edit from elsewhere
        // since then is a conflict, never overwritten. Checked and written
        // under the project's writer lock (ADR 0040).
        let path = self.opening.dir.join(&file);
        #[cfg(feature = "test-hooks")]
        if let Some(hook) = self.before_lock.as_mut() {
            hook();
        }
        let lock = writer
            .lock()
            .map_err(|e| fail("write-failed", format!("{file} was not written: {e}")))?;
        let now = fs::read(&path).ok().map(|bytes| sha256_hex(&bytes));
        if now.as_deref() != Some(base) {
            return Err(fail_with(
                "conflict",
                format!("{file} changed since the version the patch names"),
                json!({"level": level, "file": file, "base": now}),
            ));
        }
        // The positions the patch was built on are still the file's: a
        // `schematic.move` from elsewhere meanwhile is a conflict, never lost.
        if patched.positions_changed {
            let now = match fs::read_to_string(&positions_path) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(fail("write-failed", format!("{SCHEMATIC_FILE}: {error}")));
                }
            };
            // The conflict names the level's file and its base (unchanged,
            // as just checked), the version a client builds the patch on
            // again; the positions are read again with it.
            if now != positions {
                return Err(fail_with(
                    "conflict",
                    format!(
                        "{SCHEMATIC_FILE} changed since the patch read it; nothing was written"
                    ),
                    json!({"level": level, "file": file, "base": base}),
                ));
            }
        }
        // ADR 0037, editable insides: the other files are still as read;
        // a new or changed one is written before the level's file (which
        // may name it), a removed one after.
        for change in &patched.files {
            let now = transaction::current(&self.opening.dir.join(&change.path))
                .ok()
                .flatten();
            if now.as_deref() != change.before.as_deref().map(str::as_bytes) {
                return Err(fail_with(
                    "conflict",
                    format!(
                        "{} changed since the patch read it; nothing was written",
                        change.path
                    ),
                    json!({"level": level, "file": file, "base": base}),
                ));
            }
        }
        let (early, late): (Vec<&patch::FileChange>, Vec<&patch::FileChange>) =
            patched.files.iter().partition(|c| c.after.is_some());
        let mut sides_done: Vec<&patch::FileChange> = Vec::new();
        for change in &early {
            match self.write_side(change, true) {
                Ok(true) => sides_done.push(change),
                outcome => {
                    let left = self.undo_sides(&sides_done);
                    let why = match outcome {
                        Err(error) => error.to_string(),
                        _ => "another writer changed it meanwhile".to_owned(),
                    };
                    let rest = if left.is_empty() {
                        "; nothing changed".to_owned()
                    } else {
                        left
                    };
                    return Err(fail(
                        "write-failed",
                        format!("{} was not written ({why}){rest}", change.path),
                    ));
                }
            }
        }
        // Compared once more just before the rename (ADR 0040): the file
        // still holds `before`, the text `base` names.
        let renamed =
            transaction::replace_unchanged(&path, Some(before.as_bytes()), patched.text.as_bytes())
                .map_err(|e| {
                    let left = self.undo_sides(&sides_done);
                    fail("write-failed", format!("{file} was not written: {e}{left}"))
                })?;
        if !renamed {
            let left = self.undo_sides(&sides_done);
            let now = fs::read(&path).ok().map(|bytes| sha256_hex(&bytes));
            return Err(fail_with(
                "conflict",
                format!("{file} changed since the version the patch names{left}"),
                json!({"level": level, "file": file, "base": now}),
            ));
        }
        if patched.positions_changed {
            // Each compared once more just before it changes (ADR 0040): a
            // positions file changed meanwhile is `Ok(false)`.
            let written = match &patched.positions {
                Some(text) => positions_path
                    .parent()
                    .map_or(Ok(()), fs::create_dir_all)
                    .and_then(|()| {
                        transaction::replace_unchanged(
                            &positions_path,
                            positions.as_deref().map(str::as_bytes),
                            text.as_bytes(),
                        )
                    }),
                None => transaction::current(&positions_path).and_then(|now| {
                    if now.as_deref() != positions.as_deref().map(str::as_bytes) {
                        return Ok(false);
                    }
                    match fs::remove_file(&positions_path) {
                        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                        _ => Ok(true),
                    }
                }),
            };
            let why = match written {
                Ok(true) => None,
                Ok(false) => Some((true, "another writer changed it meanwhile".to_owned())),
                Err(error) => Some((false, error.to_string())),
            };
            if let Some((mut changed, why)) = why {
                // The level's file put back only while it holds the patch.
                let put_back = transaction::replace_unchanged(
                    &path,
                    Some(patched.text.as_bytes()),
                    before.as_bytes(),
                );
                let left = if matches!(put_back, Ok(true)) {
                    self.undo_sides(&sides_done)
                } else {
                    String::new()
                };
                let message = match put_back {
                    Ok(true) if left.is_empty() => format!(
                        "{SCHEMATIC_FILE} was not written ({why}); {file} was put back: nothing changed"
                    ),
                    Ok(true) => format!(
                        "{SCHEMATIC_FILE} was not written ({why}); {file} was put back{left}"
                    ),
                    Ok(false) => {
                        changed = true;
                        format!(
                            "{SCHEMATIC_FILE} was not written ({why}) and {file} could not be put back (another writer changed it meanwhile; its edit is kept): the positions do not hold the patch"
                        )
                    }
                    Err(again) => format!(
                        "{SCHEMATIC_FILE} was not written ({why}) and {file} could not be put back ({again}): {file} holds the patch, the positions do not"
                    ),
                };
                if changed {
                    let now = fs::read(&path).ok().map(|bytes| sha256_hex(&bytes));
                    return Err(fail_with(
                        "conflict",
                        message,
                        json!({"level": level, "file": file, "base": now}),
                    ));
                }
                return Err(fail("write-failed", message));
            }
            self.seen(Path::new(SCHEMATIC_FILE));
        }
        // ADR 0037, editable insides: a removed inside goes once the level
        // no longer names it; one that cannot go is said, and kept.
        let mut notes = patched.notes.clone();
        let mut kept: Vec<&str> = Vec::new();
        for change in &late {
            match self.write_side(change, true) {
                Ok(true) => {}
                Ok(false) => {
                    kept.push(&change.path);
                    notes.push(format!(
                        "{} was kept: another writer changed it meanwhile",
                        change.path
                    ));
                }
                Err(error) => {
                    kept.push(&change.path);
                    notes.push(format!("{} was kept: {error}", change.path));
                }
            }
        }
        // ADR 0037 §2: a slot port renamed, the panels' bindings on it follow.
        // ADR 0046: the panels the patch wrote (a part made or removed,
        // controls moved) taken as seen and shown to every client.
        for change in &patched.files {
            let Some(name) = change.path.strip_prefix("ui/") else {
                continue;
            };
            if kept.contains(&change.path.as_str()) {
                continue;
            }
            // What this host wrote and replaced: its undo and redo restore it whole.
            for text in [&change.before, &change.after].into_iter().flatten() {
                self.opening.restorable.insert(sha256_hex(text.as_bytes()));
            }
            self.seen(Path::new(&change.path));
            if !name.ends_with(".html") {
                continue;
            }
            let html = change.after.as_deref().and_then(|text| {
                panel_content(text)
                    .ok()
                    .and_then(|(start, end)| text.get(start..end))
            });
            self.broadcast(&json!({
                "type": "panel.written",
                "file": name,
                "base": change.after.as_ref().map(|t| sha256_hex(t.as_bytes())),
                "plate": change.after.as_deref().and_then(crate::plate::panel_plate),
                "html": html,
            }));
        }
        let mut rebound = self.rebind(&instance_paths, &patched.renamed, &mut notes);
        // ADR 0046 §7: a renamed interface port's part controls follow it.
        rebound.extend(self.rebind_parts(level, &patched.renamed, &mut notes));
        // ADR 0041: the snapshots' values of a renamed or removed slot
        // parameter follow it (top-level controls only), under the same lock.
        let resnapped = if level.is_empty() {
            self.resnap(&lock, &patched.follow, &mut notes)
        } else {
            false
        };
        drop(lock);
        if resnapped {
            self.snapshots_written();
        }
        // Playing, the watch reloads the level's file and pushes `graph`.
        if self.run.is_none() {
            self.push_graph(false);
        }
        let mut reply = json!({
            "type": "patched",
            "level": level,
            "file": file,
            "base": sha256_hex(patched.text.as_bytes()),
            // ADR 0046: a part whose removal was kept (another writer
            // changed it) is still there: the undo leaves it as it is.
            "inverse": patched.inverse.iter().filter(|op| !matches!(
                op,
                Op::PanelRestore { file, was: None, .. } if kept.contains(&format!("ui/{file}").as_str())
            )).collect::<Vec<_>>(),
        });
        // ADR 0037, editable insides: the other files written, each with
        // its level and new base (`null`: removed).
        if !patched.files.is_empty() {
            let files: Vec<Value> = patched
                .files
                .iter()
                .map(|c| {
                    let level = c
                        .path
                        .strip_prefix("composites/")
                        .and_then(|p| p.strip_suffix(".toml"));
                    // A removal that was kept: its file as it is now.
                    let base = if kept.contains(&c.path.as_str()) {
                        fs::read(self.opening.dir.join(&c.path))
                            .ok()
                            .map(|b| sha256_hex(&b))
                    } else {
                        c.after.as_ref().map(|t| sha256_hex(t.as_bytes()))
                    };
                    json!({"file": c.path, "level": level, "base": base})
                })
                .collect();
            reply = with(reply, "files", json!(files));
        }
        if !notes.is_empty() {
            reply = with(reply, "notes", json!(notes));
        }
        if !rebound.is_empty() {
            reply = with(reply, "panels", json!(rebound));
        }
        if resnapped {
            reply = with(
                reply,
                "snapshots",
                json!(signal_project::snapshot::SNAPSHOTS_FILE),
            );
        }
        Ok(reply)
    }

    /// ADR 0037, editable insides: a patch's other file `change` written
    /// (`forward`) or put back, only while it holds the text the other
    /// side names (ADR 0040); `Ok(false)` when it does not.
    fn write_side(&self, change: &patch::FileChange, forward: bool) -> std::io::Result<bool> {
        let path = self.opening.dir.join(&change.path);
        let (expected, target) = if forward {
            (&change.before, &change.after)
        } else {
            (&change.after, &change.before)
        };
        match target {
            Some(text) => {
                if let Some(folder) = path.parent() {
                    fs::create_dir_all(folder)?;
                }
                transaction::replace_unchanged(
                    &path,
                    expected.as_deref().map(str::as_bytes),
                    text.as_bytes(),
                )
            }
            None => {
                if transaction::current(&path)?.as_deref() != expected.as_deref().map(str::as_bytes)
                {
                    return Ok(false);
                }
                match fs::remove_file(&path) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => Ok(true),
                }
            }
        }
    }

    /// The patch's other files `done` put back (a later write failed);
    /// what could not be put back, said (empty when all were).
    fn undo_sides(&self, done: &[&patch::FileChange]) -> String {
        let results: Vec<(String, std::io::Result<bool>)> = done
            .iter()
            .rev()
            .map(|change| (change.path.clone(), self.write_side(change, false)))
            .collect();
        unrestored(&results)
    }

    /// ADR 0037 §2: the bindings of the project's panels on the slot ports
    /// `renamed` (`slot.old` → `slot.new`) under each instance path of
    /// `paths` (the level's: `""` for the top level) rewritten, each panel compared once more just
    /// before its rename (ADR 0040). Returns the panels written; one not
    /// written is said in `notes` (the level's file holds the rename).
    fn rebind(
        &mut self,
        paths: &[String],
        renamed: &[(String, String)],
        notes: &mut Vec<String>,
    ) -> Vec<String> {
        if renamed.is_empty() {
            return Vec::new();
        }
        let renames: Vec<(String, String)> = paths
            .iter()
            .flat_map(|path| {
                renamed
                    .iter()
                    .map(move |(old, new)| (format!("{path}/{old}"), format!("{path}/{new}")))
            })
            .collect();
        let ui = project_ui(&self.opening.dir);
        if fs::symlink_metadata(&ui).is_ok_and(|m| m.file_type().is_symlink()) {
            notes.push(
                "ui/: its bindings were not rewritten (it is reached through a link)".to_owned(),
            );
            return Vec::new();
        }
        let mut written = Vec::new();
        for path in crate::bindings::panel_files(&ui).unwrap_or_default() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                notes.push(format!(
                    "ui/{name}: its bindings were not rewritten (it is reached through a link)"
                ));
                continue;
            }
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    notes.push(format!(
                        "ui/{name}: its bindings were not rewritten ({error})"
                    ));
                    continue;
                }
            };
            let Some(new) = crate::bindings::rewrite_bindings(&text, &renames) else {
                continue;
            };
            match transaction::replace_unchanged(&path, Some(text.as_bytes()), new.as_bytes()) {
                Ok(true) => {
                    // As `panel.write` writes a panel: taken as seen, and every
                    // client gets it.
                    self.seen(&Path::new("ui").join(&name));
                    let html = crate::bindings::panel_content(&new)
                        .ok()
                        .and_then(|(start, end)| new.get(start..end))
                        .map(str::to_owned);
                    if let Some(html) = html {
                        self.broadcast(&json!({
                            "type": "panel.written",
                            "file": name,
                            "base": sha256_hex(new.as_bytes()),
                            "plate": crate::plate::panel_plate(&new),
                            "html": html,
                        }));
                    }
                    written.push(format!("ui/{name}"));
                }
                Ok(false) => notes.push(format!(
                    "ui/{name}: its bindings were not rewritten (another writer changed it meanwhile)"
                )),
                Err(error) => notes.push(format!(
                    "ui/{name}: its bindings were not rewritten ({error})"
                )),
            }
        }
        written
    }

    /// ADR 0046 §7: the parts of the slots whose interface ports `renamed`
    /// (`slot.old` → `slot.new` at `level`) rewritten, `.old` → `.new`, as
    /// the host panels' bindings are ([`Host::rebind`]); the parts written.
    fn rebind_parts(
        &mut self,
        level: &str,
        renamed: &[(String, String)],
        notes: &mut Vec<String>,
    ) -> Vec<String> {
        let mut by_part: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (old, new) in renamed {
            let (Some((slot, from)), Some((_, to))) = (old.split_once('.'), new.split_once('.'))
            else {
                continue;
            };
            by_part
                .entry(signal_project::patch::part_path(level, slot))
                .or_default()
                .push((format!(".{from}"), format!(".{to}")));
        }
        let mut written = Vec::new();
        for (path, renames) in by_part {
            if self.linked_file(&path) {
                if self.opening.dir.join(&path).exists() {
                    notes.push(format!(
                        "{path}: its bindings were not rewritten (it is reached through a link)"
                    ));
                }
                continue;
            }
            let full = self.opening.dir.join(&path);
            let Ok(text) = fs::read_to_string(&full) else {
                continue;
            };
            let Some(new) = crate::bindings::rewrite_bindings(&text, &renames) else {
                continue;
            };
            match transaction::replace_unchanged(&full, Some(text.as_bytes()), new.as_bytes()) {
                Ok(true) => {
                    self.seen(Path::new(&path));
                    let html = panel_content(&new)
                        .ok()
                        .and_then(|(start, end)| new.get(start..end))
                        .map(str::to_owned);
                    self.broadcast(&json!({
                        "type": "panel.written",
                        "file": path.trim_start_matches("ui/"),
                        "base": sha256_hex(new.as_bytes()),
                        "plate": crate::plate::panel_plate(&new),
                        "html": html,
                    }));
                    written.push(path);
                }
                Ok(false) => notes.push(format!(
                    "{path}: its bindings were not rewritten (another writer changed it meanwhile)"
                )),
                Err(error) => {
                    notes.push(format!("{path}: its bindings were not rewritten ({error})"))
                }
            }
        }
        written
    }

    /// ADR 0041 with ADR 0037 §2: in `snapshots.toml` (when the project has
    /// one), every snapshot's value (the undo snapshot's too) of a top-level
    /// slot port renamed or removed by the patch follows it, in the order of
    /// `follow` (`slot.old` → `Some(slot.new)`, or removed: `None`), written
    /// under the held `lock` in the file's canonical text; nothing is
    /// written when no value moves. Whether it wrote; a failure is a note
    /// (the level's file holds the edit).
    fn resnap(
        &self,
        lock: &ProjectLock,
        follow: &[(String, Option<String>)],
        notes: &mut Vec<String>,
    ) -> bool {
        if follow.is_empty() || !self.opening.dir.join(SNAPSHOTS_FILE).exists() {
            return false;
        }
        let apply = |values: &mut snapshots::Values| {
            let mut moved = false;
            for (old, new) in follow {
                if let Some(value) = values.remove(&format!("/{old}")) {
                    moved = true;
                    if let Some(new) = new {
                        values.insert(format!("/{new}"), value);
                    }
                }
            }
            moved
        };
        let edited = snapshots::edit_locked(
            &self.opening.dir,
            lock,
            |_| Ok(()),
            |all| {
                let mut moved = false;
                for values in all.snapshots.values_mut() {
                    moved |= apply(values);
                }
                if let Some(undo) = &mut all.undo {
                    moved |= apply(undo);
                }
                // Nothing to move: the file is left as it is, byte for byte.
                if moved {
                    Ok(())
                } else {
                    Err(SnapshotError::Changed(String::new()))
                }
            },
        );
        match edited {
            Ok(edited) => edited.wrote,
            Err(SnapshotError::Changed(why)) if why.is_empty() => false,
            Err(error) => {
                notes.push(format!(
                    "{SNAPSHOTS_FILE}: its values were not renamed ({error})"
                ));
                false
            }
        }
    }

    /// Whether `file` (relative to the project) is reached through a link:
    /// the file itself, or for a composite the `composites/` folder. The
    /// project's files are written only in its own folder (`patch`,
    /// `file.write`, `vendor`).
    pub(super) fn linked_file(&self, file: &str) -> bool {
        let link =
            |path: &Path| fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        link(&self.opening.dir.join(file))
            || (file.starts_with("composites/") && link(&self.opening.dir.join("composites")))
            || (file.starts_with("interfaces/") && link(&self.opening.dir.join("interfaces")))
            || (file.starts_with("ui/") && link(&project_ui(&self.opening.dir)))
            || (file.starts_with("ui/parts/") && link(&self.opening.dir.join("ui/parts")))
            || (file.starts_with("ui/") && !crate::own_file(&self.opening.dir, file))
    }

    /// ADR 0046: a presentation file of the project (`ui/main.html`,
    /// `ui/windows.toml`, `ui/parts/<key>.html`) as it is now; `None` when
    /// it is absent, unreadable or reached through a link (never written).
    pub(super) fn ui_text(&self, file: &str) -> Option<String> {
        if self.linked_file(file) {
            return None;
        }
        crate::read_own_file(&self.opening.dir, file).ok().flatten()
    }

    /// ADR 0046 §7–8: the panels' side of a patch, added to `patched` as
    /// more of its files with their `panel.restore` inverses (the undo puts
    /// them back first): Group into slot at the top level moves the main
    /// panel's controls on the instance into its new part, the open slot
    /// card in their place, and `ui/windows.toml` follows the moved cards;
    /// an interface edit that removes a port a part binds is `in-use`,
    /// unless `removeControls`, which removes those controls from the part.
    fn part_edits(
        &self,
        level: &str,
        ops: &[Op],
        before: &str,
        templates: &BTreeMap<String, signal_project::Interface>,
        patched: &mut patch::Patched,
    ) -> Result<(), Failure> {
        // The presentation files as the patch leaves them: (read, now).
        let mut files: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
        let now = |files: &BTreeMap<String, (Option<String>, Option<String>)>,
                   patched: &patch::Patched,
                   path: &str| {
            if let Some((_, now)) = files.get(path) {
                return now.clone();
            }
            match patched.files.iter().find(|c| c.path == path) {
                Some(change) => change.after.clone(),
                None => self.ui_text(path),
            }
        };
        let set = |files: &mut BTreeMap<String, (Option<String>, Option<String>)>,
                   was: Option<String>,
                   path: &str,
                   text: Option<String>| {
            files.entry(path.to_owned()).or_insert((was, None)).1 = text;
        };
        // The slots whose interface an op edits, and those whose part's
        // controls on dropped names go with them (`removeControls`).
        let mut touched: Vec<String> = Vec::new();
        let mut strip: Vec<String> = Vec::new();
        for op in ops {
            match op {
                Op::SlotGroup { instance, controls } if level.is_empty() => {
                    let main = format!("ui/{MAIN_PANEL}");
                    // Without a main panel the default one shows the slot's card.
                    let Some(text) = now(&files, patched, &main) else {
                        continue;
                    };
                    let grouped = panel_content(&text).and_then(|(start, end)| {
                        crate::parts::group_content(
                            text.get(start..end).unwrap_or(""),
                            instance,
                            controls.as_deref(),
                        )
                    });
                    let grouped = match grouped {
                        Ok(grouped) => grouped,
                        Err(why) => {
                            patched.notes.push(format!(
                                "ui/{MAIN_PANEL} was not changed: its controls on {instance} still bind it from the host's panel ({why})"
                            ));
                            continue;
                        }
                    };
                    let Some(new) = crate::parts::with_content(&text, &grouped.host) else {
                        continue;
                    };
                    set(&mut files, Some(text.clone()), &main, Some(new));
                    if let Some(content) = &grouped.part {
                        let path = format!("ui/{}", crate::parts::file_of(instance));
                        if let Some(was) = now(&files, patched, &path) {
                            return Err(fail_with(
                                "bad-value",
                                format!(
                                    "{path} exists already: move or remove it before grouping {instance} (ADR 0046)"
                                ),
                                json!({"file": path, "base": sha256_hex(was.as_bytes())}),
                            ));
                        }
                        set(
                            &mut files,
                            None,
                            &path,
                            Some(crate::parts::part_text(instance, content)),
                        );
                        patched.notes.push(format!(
                            "moved {} control{} on {instance} into {path}",
                            grouped.moved,
                            if grouped.moved == 1 { "" } else { "s" }
                        ));
                    }
                    let windows = crate::WINDOWS_FILE;
                    if let Some(text) = now(&files, patched, windows)
                        && let Some(new) = crate::parts::rename_cards(&text, &grouped.cards, Some(instance))
                    {
                        set(&mut files, Some(text), windows, Some(new));
                    }
                }
                Op::SlotGroup { instance, .. } => patched.notes.push(format!(
                    "{instance} is in the composite {level}: no control moved (a part here would be every instance's; add one with Edit layout)"
                )),
                Op::SlotInterface {
                    slot,
                    rename,
                    remove_controls,
                    ..
                } => {
                    if *remove_controls {
                        strip.push(slot.clone());
                    }
                    touched.push(slot.clone());
                    // A rename onto a name the part binds already would
                    // merge two binds, and its undo could not part them.
                    let path = signal_project::patch::part_path(level, slot);
                    if let Some(text) = now(&files, patched, &path) {
                        for new in rename.values() {
                            let on = crate::parts::controls_on(&text, std::slice::from_ref(new));
                            if !on.is_empty() {
                                return Err(fail(
                                    "bad-value",
                                    format!(
                                        "not renamed: {path} binds .{new} already ({}); rename or remove those controls first",
                                        on.join(", ")
                                    ),
                                ));
                            }
                        }
                    }
                }
                Op::SlotRestore { slot, .. } => touched.push(slot.clone()),
                _ => {}
            }
        }
        // Every interface edit that drops a name the part binds (a remove,
        // a template set or detached, an undo's restore): `in-use`, unless
        // its controls go too. A renamed name is not dropped: its binds follow.
        touched.sort();
        touched.dedup();
        for slot in &touched {
            let path = signal_project::patch::part_path(level, slot);
            let Some(text) = now(&files, patched, &path) else {
                continue;
            };
            let Some(was) = signal_project::patch::interface_names(before, slot, templates) else {
                continue;
            };
            let is = signal_project::patch::interface_names(&patched.text, slot, templates)
                .unwrap_or_default();
            let renamed: Vec<&str> = patched
                .renamed
                .iter()
                .filter_map(|(old, _)| old.strip_prefix(&format!("{slot}.")))
                .collect();
            let dropped: Vec<String> = was
                .into_iter()
                .filter(|n| !is.contains(n) && !renamed.contains(&n.as_str()))
                .collect();
            let on = crate::parts::controls_on(&text, &dropped);
            if on.is_empty() {
                continue;
            }
            if !strip.contains(slot) {
                let references: Vec<Value> = on
                    .iter()
                    .map(|c| json!({"file": path, "text": c}))
                    .collect();
                return Err(fail_with(
                    "in-use",
                    format!(
                        "not removed: {path} binds it ({}); remove its controls too with removeControls",
                        on.join(", ")
                    ),
                    json!({"references": references}),
                ));
            }
            match crate::parts::without_controls(&text, &dropped) {
                Ok(Some(new)) => {
                    set(&mut files, Some(text), &path, Some(new));
                    patched
                        .notes
                        .push(format!("removed {} from {path}", on.join(", ")));
                }
                Ok(None) => {}
                Err(why) => {
                    return Err(fail(
                        "bad-value",
                        format!(
                            "{path}'s controls were not removed: it is not a panel the editor writes ({why}); edit it by hand"
                        ),
                    ));
                }
            }
        }
        let mut undo = Vec::new();
        for (path, (before, after)) in files {
            if before == after {
                continue;
            }
            if patched.files.iter().any(|c| c.path == path) {
                return Err(fail(
                    "bad-value",
                    format!(
                        "{path}: a patch writes it once (a panel.restore and a slot op of the same patch)"
                    ),
                ));
            }
            undo.push(Op::PanelRestore {
                file: path.trim_start_matches("ui/").to_owned(),
                text: before.clone(),
                was: after.clone(),
            });
            patched.files.push(patch::FileChange {
                path,
                before,
                after,
            });
        }
        if !undo.is_empty() {
            undo.append(&mut patched.inverse);
            patched.inverse = undo;
        }
        Ok(())
    }

    /// ADR 0046 §8: an untrusted `panel.restore` (an inverse a client sends
    /// back). A text a patch of this host wrote or replaced (its SHA-256 in
    /// `restorable`) is taken whole: the undo and redo of what the host
    /// did, a hand-written part removed with its slot included. Any other
    /// text holds only what the panel editor writes: an HTML file's
    /// `<signal-panel>` content in its canonical form (the whitelist, no
    /// comment) and, outside that element, what the file holds now; a part
    /// that does not exist exactly as a new part is written
    /// ([`crate::parts::part_text`]). `ui/windows.toml` must read as TOML.
    fn panel_restore_allowed(&self, file: &str, text: &str) -> Result<(), Failure> {
        let refused = |why: String| fail("bad-value", format!("panel.restore of ui/{file}: {why}"));
        // What this host itself wrote or replaced (an undo, a redo): whole.
        if self
            .opening
            .restorable
            .contains(&sha256_hex(text.as_bytes()))
        {
            return Ok(());
        }
        if file == "windows.toml" {
            return text
                .parse::<toml_edit::DocumentMut>()
                .map(|_| ())
                .map_err(|e| refused(format!("not TOML ({})", e.to_string().trim())));
        }
        let (start, end) = panel_content(text).map_err(refused)?;
        let content = text.get(start..end).unwrap_or("");
        let tree = crate::parts::parse(content)
            .map_err(|why| refused(format!("its panel is not one the editor reads ({why})")))?;
        // The canonical text only: no comment can hide anything.
        if crate::parts::serialize(&tree) != content {
            return Err(refused(
                "its panel is not in the form the editor writes (canonical, no comment)".to_owned(),
            ));
        }
        let outside = |t: &str, s: usize, e: usize| {
            format!("{}{}", t.get(..s).unwrap_or(""), t.get(e..).unwrap_or(""))
        };
        match self.ui_text(&format!("ui/{file}")) {
            Some(now) => {
                let (s, e) = panel_content(&now).map_err(refused)?;
                if outside(&now, s, e) != outside(text, start, end) {
                    return Err(refused(
                        "it changes the file outside its <signal-panel>".to_owned(),
                    ));
                }
            }
            None => {
                // A new part exactly as one is written: its comment and its
                // `<signal-panel part>`, nothing else.
                let key = crate::parts::key_of(file)
                    .ok_or_else(|| refused("only a slot's part is made by a restore".to_owned()))?;
                if crate::parts::part_text(key, content) != text {
                    return Err(refused(
                        "a new part is written as the editor writes one: its comment and its <signal-panel part> alone"
                            .to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// The edited project `new` (its files as an edit would leave them),
    /// planned as a reload would: it must have its audio sink and, while
    /// playing, the running project must be able to take it. Every edit of
    /// the project's text checks this before it writes (`patch`,
    /// `pattern.write`, `file.write`, `vendor`).
    pub(super) fn loads_as_edit(
        &self,
        new: Result<Project, Vec<Diagnostic>>,
    ) -> Result<(), Vec<Diagnostic>> {
        let new = new?;
        let planned = new.plan(&self.registry)?.plan;
        sink_of(&new, &planned)?;
        if let Some(run) = &self.run {
            // `self.plan` is the running project's plan, kept with it.
            signal_project::live::check_edit(
                &run.current,
                self.plan.as_ref(),
                &new,
                planned,
                &self.registry,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR 0037, editable insides: a failed put-back is said, never "nothing changed".
    #[test]
    fn a_side_file_not_put_back_is_said() {
        assert_eq!(
            unrestored(&[("composites/a.toml".to_owned(), Ok(true))]),
            ""
        );
        let said = unrestored(&[
            ("composites/a.toml".to_owned(), Ok(false)),
            (
                "composites/b.toml".to_owned(),
                Err(std::io::Error::other("disk full")),
            ),
        ]);
        assert_eq!(
            said,
            "; composites/a.toml could not be put back (another writer changed it meanwhile; its edit is kept); composites/b.toml could not be put back (disk full): it holds the patch"
        );
    }
}
