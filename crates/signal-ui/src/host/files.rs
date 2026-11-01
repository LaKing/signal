//! Reading and writing the project's text files: `file.*`, `panel.*`,
//! `pattern.*`, `slot.projects` and `vendor`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use signal_project::transaction::{self, ProjectLock};
use signal_project::vendor::{self, VendorError};
use signal_project::{Diagnostic, PATTERNS, PROJECT_FILE, Project, located};

use crate::bindings::{MAIN_PANEL, Targets, check_file, panel_content};
use crate::panel_write::{check_content, first_file, replace, sha256_hex};

use super::failure::{Failure, fail, fail_with};
use super::{Host, project_ui, with};

/// ADR 0037 §7: how many project folders `slot.projects` lists of each
/// place (the project's subfolders, its siblings).
const SLOT_PROJECTS: usize = 64;

impl Host {
    /// The file a `file.read` or `file.write` names: `project.toml`,
    /// `composites/<name>.toml` or (ADR 0037 §3) `interfaces/<name>.toml`,
    /// never any other path (`bad-value`).
    fn project_text_file(message: &Value) -> Result<String, Failure> {
        let file = message
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "needs `file`"))?;
        let named = |folder: &str| {
            file.strip_prefix(folder)
                .and_then(|f| f.strip_suffix(".toml"))
                .is_some_and(|name| {
                    name.starts_with(|c: char| c.is_ascii_lowercase())
                        && name
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                })
        };
        if file == PROJECT_FILE || named("composites/") || named("interfaces/") {
            Ok(file.to_owned())
        } else {
            Err(fail(
                "bad-value",
                format!(
                    "`{file}` is not project.toml, composites/<name>.toml or interfaces/<name>.toml: patterns are pattern.*, panels panel.*, positions schematic.*"
                ),
            ))
        }
    }

    /// `file.read` (M2.12-1, ADR 0029 §2): the text of `project.toml` or a
    /// composite's file and its SHA-256 (the `base` a `file.write` names);
    /// a missing composite reads as `exists: false`.
    pub(super) fn file_read(&self, message: &Value) -> Result<Value, Failure> {
        let file = Self::project_text_file(message)?;
        if self.linked_file(&file) {
            return Err(fail(
                "bad-value",
                format!("{file} is reached through a link: only the project's own files are read"),
            ));
        }
        match fs::read_to_string(self.dir.join(&file)) {
            Ok(text) => Ok(json!({
                "type": "file.read",
                "file": file,
                "exists": true,
                "base": sha256_hex(text.as_bytes()),
                "text": text,
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({
                "type": "file.read", "file": file, "exists": false, "base": null, "text": null,
            })),
            Err(error) => Err(fail("bad-value", format!("{file}: {error}"))),
        }
    }

    /// `file.write` (M2.12-1, ADR 0029 §2, operator decision of 2026-10-18):
    /// `text` becomes `project.toml` or `composites/<name>.toml` with the
    /// rules of `patch` and `pattern.write`: never through a link, only when
    /// the file is still the version `base` names (`null`: a composite that
    /// does not exist yet; `conflict` otherwise, with its text), only when the
    /// project with it loads and plans as a reload would (`does-not-load`),
    /// written atomically, and not taken as seen, so the watch applies it live.
    pub(super) fn file_write(&mut self, message: &Value) -> Result<Value, Failure> {
        let file = Self::project_text_file(message)?;
        let text = message
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "file.write needs `text`"))?;
        let base = match message.get("base") {
            Some(Value::String(base)) => Some(base.as_str()),
            None | Some(Value::Null) => None,
            Some(_) => return Err(fail("bad-message", "`base` is a string or null")),
        };
        if self.linked_file(&file) {
            return Err(fail(
                "write-failed",
                format!(
                    "{file} was not written: it is reached through a link, and only the project's own files are written"
                ),
            ));
        }
        let path = self.dir.join(&file);
        let current = || -> Result<Option<String>, Failure> {
            match fs::read_to_string(&path) {
                Ok(text) => Ok(Some(text)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(fail("write-failed", format!("{file}: {error}"))),
            }
        };
        let conflict = |before: Option<String>| {
            let now = before.as_deref().map(|t| sha256_hex(t.as_bytes()));
            fail_with(
                "conflict",
                format!("{file} changed since the version the write names"),
                json!({"file": file, "base": now, "text": before}),
            )
        };
        let before = current()?;
        let hash = before.as_deref().map(|t| sha256_hex(t.as_bytes()));
        if hash.as_deref() != base {
            return Err(conflict(before));
        }
        let mut sources = signal_project::read_sources(&self.dir)
            .map_err(|e| fail("does-not-load", e.to_string()))?;
        sources.retain(|s| s.path != file);
        sources.push(signal_project::SourceFile {
            path: file.clone(),
            text: text.to_owned(),
        });
        let does_not_load = |diagnostics: Vec<Diagnostic>| {
            let lines: Vec<String> = located(&self.dir, diagnostics)
                .iter()
                .map(ToString::to_string)
                .collect();
            fail_with(
                "does-not-load",
                format!("not written: {}", lines.join("\n")),
                json!({"file": file, "diagnostics": lines}),
            )
        };
        self.loads_as_edit(Project::from_sources_in(sources, &self.dir))
            .map_err(does_not_load)?;
        // The file as it is now, not as it was read: an edit from elsewhere
        // since then is a conflict, never overwritten. Checked and written
        // under the project's writer lock (ADR 0040).
        let _lock = ProjectLock::acquire(&self.dir)
            .map_err(|e| fail("write-failed", format!("{file} was not written: {e}")))?;
        let now = current()?;
        if now != before {
            return Err(conflict(now));
        }
        let failed =
            |e: std::io::Error| fail("write-failed", format!("{file} was not written: {e}"));
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(failed)?;
        }
        // Compared once more just before the rename (ADR 0040).
        if !transaction::replace_unchanged(
            &path,
            now.as_deref().map(str::as_bytes),
            text.as_bytes(),
        )
        .map_err(failed)?
        {
            return Err(conflict(current()?));
        }
        // Playing, the watch reloads the file and pushes `graph` once it
        // swaps; stopped, at once.
        if self.run.is_none() {
            self.push_graph(false);
        }
        Ok(json!({
            "type": "file.written",
            "file": file,
            "base": sha256_hex(text.as_bytes()),
            "created": before.is_none(),
        }))
    }

    /// `slot.projects` (ADR 0037 §7, the Load dialog): the projects a slot
    /// may load from beside this one: its subfolders (`./bass`) and its
    /// sibling folders (`../studio`) that hold a `project.toml`, at most
    /// [`SLOT_PROJECTS`] of each, in name order, each with its `[project]
    /// name` and what it offers (`ports`: its exports, or its sinks'
    /// channels; `settable`: the `set` keys it takes), or, when it does not
    /// load, `error`. Hidden folders and the project's own are left out;
    /// nothing is written.
    pub(super) fn slot_projects(&self) -> Result<Value, Failure> {
        if self.app.as_ref().is_some_and(|m| !m.opened) {
            return Err(fail("no-project", "open a project folder first"));
        }
        let own = fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone());
        let folders = |dir: &Path, prefix: &str| -> Vec<(String, PathBuf)> {
            let mut found: Vec<(String, PathBuf)> = fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let path = entry.path();
                    let plain = !name.starts_with('.')
                        && !name.contains(['"', '\\', '\n'])
                        && path.join(PROJECT_FILE).is_file();
                    let other = fs::canonicalize(&path).is_ok_and(|p| p != own);
                    (plain && other).then(|| (format!("{prefix}{name}"), path))
                })
                .collect();
            found.sort();
            found.truncate(SLOT_PROJECTS);
            found
        };
        let mut candidates = folders(&self.dir, "./");
        if let Some(parent) = own.parent() {
            candidates.extend(folders(parent, "../"));
        }
        let projects: Vec<Value> = candidates
            .into_iter()
            .map(|(load, path)| match Project::load(&path) {
                Ok(project) => {
                    let names = signal_project::project_content(&project, &self.registry);
                    json!({
                        "load": load,
                        "name": project.name,
                        "ports": names.as_ref().map(|n| json!(n.ports)),
                        "settable": names.as_ref().map(|n| json!(n.settable)),
                    })
                }
                Err(error) => json!({
                    "load": load,
                    "name": null,
                    "error": error.to_string().lines().next().unwrap_or("does not load").to_owned(),
                }),
            })
            .collect();
        Ok(json!({"type": "slot.projects", "projects": projects}))
    }

    /// `vendor` (M2.12-1, ADR 0027 §4, operator decision of 2026-10-18): the
    /// library composite `composite` (`lib.<name>`) copied into the project's
    /// `composites/` as `signal vendor` copies it (`as`: the copy's name), by
    /// its all-or-nothing writer, after the host's own checks: `project.toml`
    /// is still the version `base` names (`conflict`), no file it writes is
    /// reached through a link, and the project as it would be left plans as a
    /// reload would (`does-not-load`).
    pub(super) fn vendor(&mut self, message: &Value) -> Result<Value, Failure> {
        let composite = message
            .get("composite")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "vendor needs `composite`"))?;
        let name = match message.get("as") {
            Some(Value::String(name)) => Some(name.as_str()),
            None | Some(Value::Null) => None,
            Some(_) => return Err(fail("bad-message", "`as` is a string or null")),
        };
        let base = message.get("base").and_then(Value::as_str).ok_or_else(|| {
            fail(
                "bad-message",
                "vendor needs `base`, the SHA-256 of project.toml",
            )
        })?;
        let now = fs::read(self.dir.join(PROJECT_FILE))
            .ok()
            .map(|bytes| sha256_hex(&bytes));
        if now.as_deref() != Some(base) {
            return Err(fail_with(
                "conflict",
                format!("{PROJECT_FILE} changed since the version the vendor names"),
                json!({"file": PROJECT_FILE, "base": now}),
            ));
        }
        let library = signal_project::Library::builtin();
        let refused = |error: VendorError| match error {
            VendorError::NotInLibrary(_) => fail("unknown-module", error.to_string()),
            VendorError::InvalidName(_) | VendorError::Exists(_) => {
                fail("bad-value", error.to_string())
            }
            VendorError::Syntax { .. } | VendorError::Invalid(_) => {
                fail("does-not-load", error.to_string())
            }
            VendorError::Changed(ref file) => fail_with(
                "conflict",
                error.to_string(),
                json!({"file": file, "base": fs::read(self.dir.join(file)).ok().map(|bytes| sha256_hex(&bytes))}),
            ),
            VendorError::Io { .. } => fail("write-failed", error.to_string()),
        };
        let prepared = vendor::prepare(&self.dir, composite, name, library).map_err(refused)?;
        // What was prepared is the version `base` names (the file may have
        // changed since the check above); `vendor::write` then commits it
        // only while the files are still the ones prepared (ADR 0040).
        let prepared_base = prepared
            .before()
            .iter()
            .find(|f| f.path == PROJECT_FILE)
            .map(|f| sha256_hex(f.text.as_bytes()));
        if prepared_base.as_deref() != Some(base) {
            return Err(fail_with(
                "conflict",
                format!("{PROJECT_FILE} changed since the version the vendor names"),
                json!({"file": PROJECT_FILE, "base": prepared_base}),
            ));
        }
        if let Some(file) = prepared.paths().find(|file| self.linked_file(file)) {
            return Err(fail(
                "write-failed",
                format!(
                    "nothing was vendored: {file} is reached through a link, and only the project's own files are written"
                ),
            ));
        }
        let does_not_load = |diagnostics: Vec<Diagnostic>| {
            let lines: Vec<String> = located(&self.dir, diagnostics)
                .iter()
                .map(ToString::to_string)
                .collect();
            fail_with(
                "does-not-load",
                format!("not vendored: {}", lines.join("\n")),
                json!({"diagnostics": lines}),
            )
        };
        self.loads_as_edit(Project::from_sources_in(prepared.after(), &self.dir))
            .map_err(does_not_load)?;
        let vendored = vendor::write(&self.dir, prepared, library).map_err(refused)?;
        if self.run.is_none() {
            self.push_graph(false);
        }
        Ok(json!({
            "type": "vendored",
            "composite": composite,
            "copied": vendored.copied,
            "reused": vendored.reused,
            "rewritten": vendored.rewritten,
        }))
    }

    /// `panel.check`: the binding check of a panel file in the project's
    /// `ui/` against the running plan, as `signal ui check` makes it.
    pub(super) fn panel_check(&self, message: &Value) -> Result<Value, Failure> {
        let name = message.get("file").and_then(Value::as_str).unwrap_or("");
        let unknown = || {
            fail(
                "unknown-panel",
                format!("no panel '{name}' in the project's ui/ folder"),
            )
        };
        if !name.ends_with(".html") {
            return Err(unknown());
        }
        let path = crate::http::file(&project_ui(&self.dir), name).ok_or_else(unknown)?;
        let text = std::fs::read_to_string(path).map_err(|_| unknown())?;
        let plan = self
            .plan
            .as_ref()
            .ok_or_else(|| fail("not-playing", "the project has no plan"))?;
        let targets = Targets {
            plan,
            panel: &self.panel,
        };
        // ADR 0046: a slot's part is checked at its roots.
        let checked = check_file(name, &text, &targets);
        let diagnostics: Vec<Value> = checked
            .findings
            .iter()
            .map(|f| {
                let value = serde_json::to_value(&f.diagnostic).unwrap_or(Value::Null);
                with(value, "control", json!(f.control))
            })
            .collect();
        Ok(json!({
            "type": "panel.check",
            "file": checked.file,
            "controls": checked.controls,
            "bindings": checked.bindings,
            "ok": !checked.failed(),
            "diagnostics": diagnostics,
        }))
    }

    /// The panel file `name` of `panel.read` and `panel.write`: an `.html`
    /// file in the project's `ui/` by the static files' rules
    /// ([`crate::http::file`]: no `..`, no hidden segment, no escape through
    /// a link), or `ui/main.html` when it does not exist yet (the generated
    /// default panel's first write). Never outside the project served.
    fn panel_path(&self, name: &str) -> Result<(PathBuf, bool), Failure> {
        let unknown = || {
            fail(
                "unknown-panel",
                format!("no panel '{name}' in the project's ui/ folder"),
            )
        };
        if !name.ends_with(".html") {
            return Err(unknown());
        }
        let ui = project_ui(&self.dir);
        // Written only where it resolves inside the project served, never
        // through a link (a linked `ui/` or file is served, not edited).
        let link =
            |path: &Path| fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        let part = crate::parts::key_of(name);
        if link(&ui)
            || link(&ui.join(name))
            || (part.is_some() && link(&ui.join(crate::parts::PARTS_DIR)))
        {
            return Err(fail(
                "unknown-panel",
                format!(
                    "ui/{name} is reached through a link: the editor writes only the project's own files"
                ),
            ));
        }
        if let Some(path) = crate::http::file(&ui, name) {
            let project = fs::canonicalize(&self.dir).map_err(|_| unknown())?;
            return if path.starts_with(&project) {
                Ok((path, true))
            } else {
                Err(unknown())
            };
        }
        // ADR 0046 §5: a slot's part, made by its first write.
        if let Some(key) = part
            && self.panel.part_roots(key).is_some()
            && fs::symlink_metadata(ui.join(name)).is_err()
        {
            return Ok((ui.join(name), false));
        }
        let main = ui.join(MAIN_PANEL);
        if name == MAIN_PANEL && fs::symlink_metadata(&main).is_err() {
            return Ok((main, false));
        }
        Err(unknown())
    }

    /// The pattern file of `pattern.read` and `pattern.write` (ADR 0028 §7):
    /// `patterns/<name>.toml` of the project served, from a name (never a
    /// path), and whether it is reached through a link (a linked `patterns/`
    /// or file, never read or written).
    pub(super) fn pattern_path(&self, message: &Value) -> Result<(String, PathBuf, bool), Failure> {
        if self.app.as_ref().is_some_and(|m| !m.opened) {
            return Err(fail("no-project", "open a project folder first"));
        }
        let name = message
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "a pattern message needs `name`"))?;
        let mut chars = name.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid {
            return Err(fail(
                "bad-value",
                format!(
                    "`{name}` is not a pattern name: a-z, 0-9 and _, starting with a-z (the file is patterns/<name>.toml)"
                ),
            ));
        }
        let folder = self.dir.join(PATTERNS);
        let path = folder.join(format!("{name}.toml"));
        let link =
            |path: &Path| fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        let linked = link(&folder) || link(&path);
        Ok((name.to_owned(), path, linked))
    }

    /// `pattern.read` (ADR 0028 §7): a pattern file's text, its SHA-256 (the
    /// `base` a `pattern.write` names) and its kind (`null` for a file that
    /// does not read as a pattern). A missing file reads as `exists: false`.
    pub(super) fn pattern_read(&self, message: &Value) -> Result<Value, Failure> {
        let (name, path, linked) = self.pattern_path(message)?;
        if linked {
            return Err(fail(
                "unknown-pattern",
                format!(
                    "{PATTERNS}/{name}.toml is reached through a link: patterns are read and written only in the project's own {PATTERNS}/"
                ),
            ));
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(
                    json!({"type": "pattern.read", "name": name, "exists": false,
                    "base": null, "text": null, "kind": null}),
                );
            }
            Err(error) => {
                return Err(fail(
                    "unknown-pattern",
                    format!("{PATTERNS}/{name}.toml: {error}"),
                ));
            }
        };
        Ok(json!({
            "type": "pattern.read",
            "name": name,
            "exists": true,
            "base": sha256_hex(text.as_bytes()),
            "kind": signal_project::pattern_kind(&text),
            "text": text,
        }))
    }

    /// `pattern.write` (ADR 0028 §7, as `patch` and `panel.write`): `text`
    /// becomes `patterns/<name>.toml` when the file is still the version
    /// `base` names (`null`: it does not exist yet; `conflict` otherwise,
    /// with its text) and the project, with it, loads and plans as a reload
    /// would (`does-not-load` names every problem); written atomically,
    /// never through a link, and not taken as seen, so the watch applies it
    /// live (a swap at an edit frame). Nothing is written when a step
    /// refuses.
    pub(super) fn pattern_write(&mut self, message: &Value) -> Result<Value, Failure> {
        let (name, path, linked) = self.pattern_path(message)?;
        let file = format!("{PATTERNS}/{name}.toml");
        let text = message
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "pattern.write needs `text`"))?;
        let base = match message.get("base") {
            Some(Value::String(base)) => Some(base.as_str()),
            None | Some(Value::Null) => None,
            Some(_) => return Err(fail("bad-message", "`base` is a string or null")),
        };
        if linked {
            return Err(fail(
                "write-failed",
                format!(
                    "{file} was not written: it is reached through a link, and patterns are written only in the project's own {PATTERNS}/"
                ),
            ));
        }
        let current = |path: &Path| -> Result<Option<String>, Failure> {
            match fs::read_to_string(path) {
                Ok(text) => Ok(Some(text)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(fail("write-failed", format!("{file}: {error}"))),
            }
        };
        let conflict = |before: Option<String>| {
            let now = before.as_deref().map(|t| sha256_hex(t.as_bytes()));
            fail_with(
                "conflict",
                format!("{file} changed since the version the editor started from"),
                json!({"name": name, "file": file, "base": now, "text": before}),
            )
        };
        let before = current(&path)?;
        if before
            .as_deref()
            .map(|t| sha256_hex(t.as_bytes()))
            .as_deref()
            != base
        {
            return Err(conflict(before));
        }
        // The project with the new text, loaded and compiled as a reload
        // would: the pattern's own errors and those of the instances naming it.
        let mut sources = signal_project::read_sources(&self.dir)
            .map_err(|e| fail("does-not-load", e.to_string()))?;
        sources.retain(|s| s.path != file);
        sources.push(signal_project::SourceFile {
            path: file.clone(),
            text: text.to_owned(),
        });
        let does_not_load = |diagnostics: Vec<Diagnostic>| {
            let lines: Vec<String> = located(&self.dir, diagnostics)
                .iter()
                .map(ToString::to_string)
                .collect();
            fail_with(
                "does-not-load",
                format!("not written: {}", lines.join("\n")),
                json!({"name": name, "file": file, "diagnostics": lines}),
            )
        };
        self.loads_as_edit(Project::from_sources_in(sources, &self.dir))
            .map_err(does_not_load)?;
        // The file as it is now, not as it was read: an edit from elsewhere
        // since then is a conflict, never overwritten. Checked and written
        // under the project's writer lock (ADR 0040).
        let _lock = ProjectLock::acquire(&self.dir)
            .map_err(|e| fail("write-failed", format!("{file} was not written: {e}")))?;
        let now = current(&path)?;
        if now != before {
            return Err(conflict(now));
        }
        let failed =
            |e: std::io::Error| fail("write-failed", format!("{file} was not written: {e}"));
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(failed)?;
        }
        // Compared once more just before the rename (ADR 0040).
        if !transaction::replace_unchanged(
            &path,
            now.as_deref().map(str::as_bytes),
            text.as_bytes(),
        )
        .map_err(failed)?
        {
            return Err(conflict(current(&path)?));
        }
        // Playing, the watch reloads the file and swaps it in.
        Ok(json!({
            "type": "pattern.written",
            "name": name,
            "file": file,
            "base": sha256_hex(text.as_bytes()),
            "created": before.is_none(),
        }))
    }

    /// `panel.read` (ADR 0026 §5): a panel file's text and its SHA-256 (the
    /// `base` a `panel.write` names), the content of its `<signal-panel>`
    /// (or why it has no one to edit), and whether a write can be checked
    /// (`planned`: the project has a plan). A missing `ui/main.html` reads
    /// as `exists: false` with `base: null`: the generated panel.
    pub(super) fn panel_read(&self, message: &Value) -> Result<Value, Failure> {
        let name = message.get("file").and_then(Value::as_str).unwrap_or("");
        let (path, exists) = self.panel_path(name)?;
        let planned = self.plan.is_some();
        if !exists {
            return Ok(
                json!({"type": "panel.read", "file": name, "exists": false, "base": null,
                "plate": null, "text": null, "html": null, "editable": true, "reason": null, "planned": planned}),
            );
        }
        let text = fs::read_to_string(&path)
            .map_err(|e| fail("unknown-panel", format!("ui/{name}: {e}")))?;
        let (html, reason) = match panel_content(&text) {
            Ok((start, end)) => (json!(text[start..end]), Value::Null),
            Err(reason) => (Value::Null, json!(reason)),
        };
        Ok(json!({
            "type": "panel.read",
            "file": name,
            "exists": true,
            "base": sha256_hex(text.as_bytes()),
            "plate": crate::plate::panel_plate(&text),
            "text": text,
            "html": html,
            "editable": reason.is_null(),
            "reason": reason,
            "planned": planned,
        }))
    }

    /// `panel.write` (ADR 0026 §5): replaces the content of the file's one
    /// `<signal-panel>` with `html`, keeping every byte outside it, after
    /// checking `html` against the whitelist again, that the file is still
    /// the version `base` names (`conflict` otherwise, with its text), and
    /// the bindings as `panel.check` does; written atomically as `commit`
    /// writes, and taken as seen by the watch. A missing `ui/main.html`
    /// (`base: null`) is created with the standard head.
    pub(super) fn panel_write(&mut self, message: &Value) -> Result<Value, Failure> {
        let name = message.get("file").and_then(Value::as_str).unwrap_or("");
        let html = message
            .get("html")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "panel.write needs `html`"))?;
        let base = match message.get("base") {
            Some(Value::String(base)) => Some(base.as_str()),
            None | Some(Value::Null) => None,
            Some(_) => return Err(fail("bad-message", "`base` is a string or null")),
        };
        // ADR 0047 §2: the plate on <signal-panel> itself, outside its
        // content: a string sets it, null removes it, no field keeps it.
        let plate = match message.get("plate") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(plate)) => Some(Some(plate.as_str())),
            Some(_) => return Err(fail("bad-message", "`plate` is a string or null")),
        };
        if let Some(Some(value)) = plate
            && crate::plate::parse_plate(value).is_none()
        {
            return Err(fail(
                "bad-html",
                format!(
                    "not written: plate=\"{value}\" is not two whole numbers of plate units (1 to {})",
                    crate::plate::MAX_PLATE
                ),
            ));
        }
        let (path, _) = self.panel_path(name)?;
        check_content(html).map_err(|why| fail("bad-html", format!("not written: {why}")))?;
        // The version check and the write under the project's writer lock
        // (ADR 0040), on the file as it is now.
        let _lock = ProjectLock::acquire(&self.dir)
            .map_err(|e| fail("write-failed", format!("ui/{name} was not written: {e}")))?;
        let before = match fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(fail("write-failed", format!("ui/{name}: {e}"))),
        };
        let exists = before.is_some();
        let conflict = |before: Option<&str>| {
            let current = before.map(|t| sha256_hex(t.as_bytes()));
            let html = before.and_then(|t| panel_content(t).ok().map(|(s, e)| t[s..e].to_owned()));
            let plate = before.and_then(crate::plate::panel_plate);
            fail_with(
                "conflict",
                format!("ui/{name} changed since the version the editor started from"),
                json!({"file": name, "base": current, "text": before, "html": html, "plate": plate}),
            )
        };
        let current = before.as_deref().map(|t| sha256_hex(t.as_bytes()));
        if current.as_deref() != base {
            return Err(conflict(before.as_deref()));
        }
        let after = match &before {
            Some(text) => {
                let range = panel_content(text).map_err(|why| fail("not-editable", why))?;
                replace(text, range, html)
            }
            // ADR 0046 §5: a slot's part, made by its first write.
            None => match crate::parts::key_of(name) {
                Some(key) => crate::parts::part_text(key, html),
                None => first_file(&self.project_name(), html),
            },
        };
        let after = match plate {
            Some(plate) => crate::plate::with_plate(&after, plate)
                .map_err(|why| fail("not-editable", format!("not written: {why}")))?,
            None => after,
        };
        let plan = self.plan.as_ref().ok_or_else(|| {
            fail(
                "not-playing",
                "the project has no plan: the bindings cannot be checked",
            )
        })?;
        let targets = Targets {
            plan,
            panel: &self.panel,
        };
        let checked = check_file(name, &after, &targets);
        if checked.failed() {
            let diagnostics: Vec<Value> = checked
                .findings
                .iter()
                .map(|f| {
                    let value = serde_json::to_value(&f.diagnostic).unwrap_or(Value::Null);
                    with(value, "control", json!(f.control))
                })
                .collect();
            let first = checked
                .findings
                .iter()
                .find(|f| f.diagnostic.severity == signal_project::Severity::Error)
                .map(|f| f.diagnostic.message.clone())
                .unwrap_or_default();
            return Err(fail_with(
                "bad-binding",
                format!("not written: {first}"),
                json!({"file": name, "diagnostics": diagnostics}),
            ));
        }
        let failed =
            |e: std::io::Error| fail("write-failed", format!("ui/{name} was not written: {e}"));
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(failed)?;
        }
        // Compared once more just before the rename (ADR 0040).
        if !transaction::replace_unchanged(
            &path,
            before.as_deref().map(str::as_bytes),
            after.as_bytes(),
        )
        .map_err(failed)?
        {
            let now = fs::read_to_string(&path).ok();
            return Err(conflict(now.as_deref()));
        }
        self.seen(&Path::new("ui").join(name));
        Ok(json!({
            "type": "panel.written",
            "file": name,
            "base": sha256_hex(after.as_bytes()),
            "plate": crate::plate::panel_plate(&after),
            "created": !exists,
        }))
    }
}
