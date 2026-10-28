//! ADR 0037, editable insides (2026-10-27): the ops that make a slot's own
//! inside (`slot.inside`), write its file (`composite.file`, the inverse),
//! and wire a composite level's frame pins to its inner ports
//! (`export.connect`, `export.disconnect`, their inverse `export.restore`).
//! A slot op on a slot that loads its own inside keeps the inside's exports
//! in step (`Patching::sync_inside`).
//!
//! The level's file is the patch's as always; the inside's file is another
//! file of the patch ([`FileChange`]), written by the caller under the same
//! writer lock (ADR 0040), and undone by a `composite.file` holding its
//! whole text, so an undo gives back every byte.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use toml_edit::{DocumentMut, Item as TomlItem, Table, TableLike};

use super::slot::{Change, Tab, apply_change_in, item_text, json_text};
use super::{Op, PatchError, Patching, Scheme, bad, map, refused, valid_name};
use crate::inside::{self, Source};
use crate::slot::Role;

/// A file other than the level's that a patch writes: its path, its text
/// before (`None`: absent) and after (`None`: removed).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FileChange {
    /// `composites/<name>.toml`.
    pub path: String,
    /// The text it held.
    pub before: Option<String>,
    /// The text it holds now.
    pub after: Option<String>,
}

/// The files of `files` the patch changed, by path.
pub(super) fn changed(
    files: BTreeMap<String, (Option<String>, Option<String>)>,
) -> Vec<FileChange> {
    files
        .into_iter()
        .filter(|(_, (before, after))| before != after)
        .map(|(path, (before, after))| FileChange {
            path,
            before,
            after,
        })
        .collect()
}

fn composite_path(name: &str) -> String {
    format!("{}/{name}.toml", crate::COMPOSITES)
}

fn role_table(role: Role) -> &'static str {
    match role {
        Role::Input => "inputs",
        Role::Output => "outputs",
        Role::Parameter => "parameters",
    }
}

/// An export's targets as written: a port, a list, or the table form's
/// `to` (`from` for an output).
fn targets(item: &TomlItem, role: Role) -> Vec<String> {
    let strings = |item: &TomlItem| -> Vec<String> {
        match item {
            TomlItem::Value(v) => match v {
                toml_edit::Value::String(s) => vec![s.value().clone()],
                toml_edit::Value::Array(a) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    };
    match item.as_table_like() {
        Some(t) => {
            let key = if role == Role::Output { "from" } else { "to" };
            t.get(key).map(strings).unwrap_or_default()
        }
        None => strings(item),
    }
}

/// `ports` as an export's port value: one port as a string, several a list.
fn port_value(ports: &[String]) -> toml_edit::Value {
    if let [one] = ports {
        toml_edit::Value::from(one.as_str())
    } else {
        let mut array = toml_edit::Array::new();
        for port in ports {
            array.push(port.as_str());
        }
        toml_edit::Value::Array(array)
    }
}

/// The text of `item` (an export's entry) with its targets `ports`: the
/// plain form replaced, the table form's `to` (or `from`) set.
fn with_targets(item: &TomlItem, role: Role, ports: &[String]) -> Option<String> {
    match item.as_table_like() {
        Some(_) => {
            let mut item = item.clone();
            let key = if role == Role::Output { "from" } else { "to" };
            let table = item.as_table_like_mut()?;
            table.insert(key, TomlItem::Value(port_value(ports)));
            item_text(&item)
        }
        None => item_text(&TomlItem::Value(port_value(ports))),
    }
}

impl Patching<'_> {
    /// The project's file at `path` as the patch leaves it so far.
    fn file_now(&self, path: &str) -> Option<String> {
        match self.files.get(path) {
            Some((_, now)) => now.clone(),
            None => (self.ctx.file)(path),
        }
    }

    /// The composite slot `slot` (loading `load`) holds as its own inside:
    /// `load` is a project composite whose `inside_of` names the slot.
    pub(super) fn own_inside(&self, slot: &str, load: Option<&str>) -> Option<String> {
        let load = load?;
        if crate::is_project_path(load) || !valid_name(load) {
            return None;
        }
        let text = self.file_now(&composite_path(load))?;
        (inside::owner_of(&text)? == inside::owner(self.ctx.level, slot)).then(|| load.to_owned())
    }

    /// `composites/<name>.toml` set to `text` (`None`: removed); returns the
    /// `composite.file` that undoes it.
    fn write_inside(&mut self, name: &str, text: Option<String>) -> Result<Op, PatchError> {
        if name == self.ctx.level {
            return Err(bad(format!(
                "composites/{name}.toml is this patch's level: its own ops edit it"
            )));
        }
        let path = composite_path(name);
        let now = self.file_now(&path);
        let entry = self
            .files
            .entry(path)
            .or_insert_with(|| (now.clone(), now.clone()));
        entry.1.clone_from(&text);
        Ok(Op::CompositeFile {
            name: name.to_owned(),
            text: now,
            was: text,
        })
    }

    /// `slot.inside`: slot `slot`'s own inside made from what it holds, and
    /// loaded. A project as content is refused: it is edited in its own
    /// folder (not copied in v1).
    pub(super) fn slot_inside(
        &mut self,
        slot: &str,
        name: Option<&str>,
    ) -> Result<Vec<Op>, PatchError> {
        self.ensure_slot(slot)?;
        let read = super::slot::read_slot(&self.text, slot, self.ctx.templates)?;
        let owner = inside::owner(self.ctx.level, slot);
        let source = match &read.load {
            None => Source::Empty,
            Some(load) if crate::is_project_path(load) => {
                return Err(bad(format!(
                    "`{slot}` holds the project `{load}`: a project is edited in its own folder, and a slot's own copy of one is not made (ADR 0037, editable insides)"
                )));
            }
            Some(load) => {
                if self.own_inside(slot, Some(load)).is_some() {
                    return Err(bad(format!(
                        "`{slot}` holds its own inside already: composites/{load}.toml"
                    )));
                }
                (self.ctx.source)(load).ok_or_else(|| {
                    refused(
                        "unknown-module",
                        format!("`{slot}`'s content `{load}` names no module to copy"),
                    )
                })?
            }
        };
        let taken = |n: &str| {
            self.file_now(&composite_path(n)).is_some()
                || crate::Library::builtin().get(n).is_some()
        };
        let name = match name {
            Some(name) => {
                if !valid_name(name) {
                    return Err(bad(format!(
                        "`{name}`: a composite's name is [a-z][a-z0-9_]*"
                    )));
                }
                if taken(name) {
                    return Err(bad(format!(
                        "`{name}` is taken: composites/{name}.toml exists, or the library has `lib.{name}`"
                    )));
                }
                name.to_owned()
            }
            None => inside::default_name(self.ctx.level, slot, &taken),
        };
        let content: Vec<(String, Role)> = match &source {
            Source::Empty => Vec::new(),
            Source::Primitive { ports, .. } => ports.clone(),
            Source::Composite(_) => read
                .load
                .as_deref()
                .and_then(|load| (self.ctx.content)(load))
                .map(|c| c.ports.iter().map(|p| (p.name.clone(), p.role)).collect())
                .unwrap_or_default(),
        };
        let matched = inside::matches(&read.interface, &read.map, &content);
        let names: BTreeSet<String> = read
            .interface
            .ports
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let text = inside::text(&name, &owner, &source, &matched, &names).map_err(bad)?;
        let mut inverses = vec![self.write_inside(&name, Some(text))?];
        let mut changes = vec![Change {
            path: vec!["load".to_owned()],
            value: Some(json_text(&serde_json::json!(name))?),
            at: None,
            decor: None,
        }];
        if !read.map.is_empty() {
            // Its exports are named after the slot's ports: matched by name.
            changes.push(Change {
                path: vec!["map".to_owned()],
                value: None,
                at: None,
                decor: None,
            });
        }
        let (changes, wires) = self.restore(slot, &changes, &BTreeMap::new())?;
        inverses.push(Op::SlotRestore {
            slot: slot.to_owned(),
            changes,
            wires,
        });
        let from = match &read.load {
            None => "empty".to_owned(),
            Some(load) => format!("a copy of {load}"),
        };
        self.notes.push(format!(
            "{slot}'s own inside: composites/{name}.toml ({from})"
        ));
        Ok(inverses)
    }

    /// `composite.file` (untrusted): only a slot's own inside (a text with
    /// `inside_of`, before and after) is made, replaced or removed, and
    /// only while it holds `was`.
    pub(super) fn composite_file(
        &mut self,
        name: &str,
        text: &Option<String>,
        was: &Option<String>,
    ) -> Result<Op, PatchError> {
        if !valid_name(name) {
            return Err(bad(format!(
                "`{name}`: a composite's name is [a-z][a-z0-9_]*"
            )));
        }
        let now = self.file_now(&composite_path(name));
        if now != *was {
            return Err(bad(format!(
                "composites/{name}.toml is not the text the op names"
            )));
        }
        if text.is_none() && now.is_none() {
            return Err(bad(format!("composites/{name}.toml: nothing to do")));
        }
        // Only a slot's own inside, of a slot at this patch's level, and it
        // stays that slot's: what an undo of `slot.inside` or of an
        // interface edit here means, nothing more.
        let mut owners = Vec::new();
        for side in [&now, text].into_iter().flatten() {
            let Some(owner) = inside::owner_of(side) else {
                return Err(bad(format!(
                    "composite.file writes only a slot's own inside (a composite with `inside_of`), not composites/{name}.toml"
                )));
            };
            owners.push(owner);
        }
        if owners.windows(2).any(|w| w[0] != w[1]) {
            return Err(bad(format!(
                "composite.file keeps composites/{name}.toml the inside of the slot it names"
            )));
        }
        let owner = owners.first().cloned().unwrap_or_default();
        let (level, slot) = owner.rsplit_once('/').unwrap_or(("", owner.as_str()));
        if level != self.ctx.level || !super::slot::is_slot(&self.text, slot) {
            return Err(bad(format!(
                "composite.file writes the inside of a slot at this patch's level, not of `{owner}`"
            )));
        }
        if now.is_none() && crate::Library::builtin().get(name).is_some() {
            return Err(bad(format!(
                "`{name}` is the library's `lib.{name}`: no project composite takes its name here"
            )));
        }
        self.write_inside(name, text.clone())
    }

    /// The exports of own inside `name` renamed or removed as `synced`
    /// says (content name → new name, or `None`: removed); the
    /// `composite.file` that undoes it, if anything changed.
    pub(super) fn sync_inside(
        &mut self,
        name: &str,
        synced: &[(String, Option<String>)],
    ) -> Result<Vec<Op>, PatchError> {
        let Some(mut text) = self.file_now(&composite_path(name)) else {
            return Ok(Vec::new());
        };
        let before = text.clone();
        for (old, new) in synced {
            // A rename onto an export the inside has already would overwrite it.
            if let Some(new) = new
                && inside::has_export(&text, new)
            {
                return Err(bad(format!(
                    "`{new}` is an export of composites/{name}.toml already: rename it, or remove it, first"
                )));
            }
            let edited = match new {
                Some(new) => inside::rename_export(&text, old, new),
                None => inside::remove_export(&text, old),
            };
            if let Some(edited) = edited {
                text = edited;
            }
        }
        if text == before {
            return Ok(Vec::new());
        }
        Ok(vec![self.write_inside(name, Some(text))?])
    }

    /// `export.connect` and `export.disconnect` at a composite level.
    pub(super) fn export_edit(
        &mut self,
        connect: bool,
        role: Role,
        name: &str,
        port: &str,
    ) -> Result<Op, PatchError> {
        if self.ctx.level.is_empty() {
            return Err(bad(
                "`[exports]` are a composite's: an export op patches a composite level",
            ));
        }
        if !valid_name(name) {
            return Err(bad(format!(
                "`{name}`: an export's name is [a-z][a-z0-9_]*"
            )));
        }
        let Some((instance, inner)) = port.split_once('.') else {
            return Err(bad(format!("`{port}`: an inner port is `instance.port`")));
        };
        if !valid_name(instance) || !valid_name(inner) {
            return Err(bad(format!("`{port}`: an inner port is `instance.port`")));
        }
        let m = map(&self.text, &Scheme::LEVEL).map_err(|e| refused("does-not-load", e))?;
        if !m.entries.iter().any(|e| e.name == instance) {
            return Err(refused(
                "unknown-instance",
                format!("no instance `{instance}` at this level"),
            ));
        }
        let doc: DocumentMut = self
            .text
            .parse()
            .map_err(|_| refused("does-not-load", "the file does not parse"))?;
        let table = role_table(role);
        let entry = doc
            .get("exports")
            .and_then(TomlItem::as_table_like)
            .and_then(|t| t.get(table))
            .and_then(TomlItem::as_table_like)
            .and_then(|t| t.get(name))
            .cloned();
        let mut ports = entry.as_ref().map(|e| targets(e, role)).unwrap_or_default();
        let value = if connect {
            if ports.iter().any(|p| p == port) {
                return Err(bad(format!("the export `{name}` reaches `{port}` already")));
            }
            if role == Role::Output && !ports.is_empty() {
                return Err(bad(format!(
                    "the output `{name}` is exported from `{}`: an exported output names one port; disconnect it first",
                    ports.join(", ")
                )));
            }
            ports.push(port.to_owned());
            match &entry {
                Some(item) => with_targets(item, role, &ports),
                None => item_text(&TomlItem::Value(port_value(&ports))),
            }
        } else {
            if !ports.iter().any(|p| p == port) {
                return Err(bad(format!("the export `{name}` does not reach `{port}`")));
            }
            ports.retain(|p| p != port);
            match (&entry, ports.is_empty()) {
                (_, true) | (None, _) => None,
                (Some(item), false) => with_targets(item, role, &ports),
            }
        };
        let change = Change {
            path: vec!["exports".to_owned(), table.to_owned(), name.to_owned()],
            value,
            at: None,
            decor: None,
        };
        let changes = self.exports_restore(&[change])?;
        Ok(Op::ExportRestore { changes })
    }

    /// `export.restore` (untrusted): only keys of the level's `[exports]`.
    pub(super) fn export_restore(&mut self, changes: &[Change]) -> Result<Op, PatchError> {
        let changes = self.exports_restore(changes)?;
        Ok(Op::ExportRestore { changes })
    }

    /// Applies `changes` to the level's `[exports]` (paths from the file's
    /// root); returns their inverse, last first. Nothing outside
    /// `[exports]` may change (checked).
    fn exports_restore(&mut self, changes: &[Change]) -> Result<Vec<Change>, PatchError> {
        if self.ctx.level.is_empty() {
            return Err(bad(
                "`[exports]` are a composite's: an export op patches a composite level",
            ));
        }
        let mut doc: DocumentMut = self
            .text
            .parse()
            .map_err(|_| refused("does-not-load", "the file does not parse"))?;
        let outside = |mut doc: DocumentMut| {
            doc.remove("exports");
            doc.to_string()
        };
        let before = outside(doc.clone());
        let mut inverse = Vec::new();
        for change in changes {
            match change.path.as_slice() {
                [only] if only == "exports" => {
                    let exists = doc.get("exports").is_some();
                    match &change.value {
                        Some(_) if !exists => {
                            doc.insert("exports", TomlItem::Table(Table::new()));
                            inverse.push(Change {
                                path: change.path.clone(),
                                value: None,
                                at: None,
                                decor: None,
                            });
                        }
                        None if exists => {
                            let empty = doc
                                .get("exports")
                                .and_then(TomlItem::as_table_like)
                                .is_some_and(TableLike::is_empty);
                            if !empty {
                                return Err(bad("`[exports]` is removed only when it is empty"));
                            }
                            doc.remove("exports");
                            inverse.push(Change {
                                path: change.path.clone(),
                                value: Some("{}".to_owned()),
                                at: None,
                                decor: None,
                            });
                        }
                        _ => {}
                    }
                }
                [first, table, rest @ ..]
                    if first == "exports"
                        && ["inputs", "outputs", "parameters"].contains(&table.as_str())
                        && rest.len() <= 1
                        && rest.iter().all(|name| valid_name(name)) =>
                {
                    if doc.get("exports").is_none() {
                        if change.value.is_none() {
                            continue;
                        }
                        // A `[exports]` section of its own, at the file's end.
                        doc.insert("exports", TomlItem::Table(Table::new()));
                        inverse.push(Change {
                            path: vec!["exports".to_owned()],
                            value: None,
                            at: None,
                            decor: None,
                        });
                    }
                    let tab = match doc.get_mut("exports") {
                        Some(TomlItem::Table(t)) => Tab::Std(t),
                        Some(TomlItem::Value(toml_edit::Value::InlineTable(t))) => Tab::Inl(t),
                        _ => return Err(bad("`exports` is not a table")),
                    };
                    let relative = Change {
                        path: change.path[1..].to_vec(),
                        ..change.clone()
                    };
                    let undone = apply_change_in(tab, &relative)?;
                    inverse.extend(undone.into_iter().map(|c| {
                        Change {
                            path: std::iter::once("exports".to_owned())
                                .chain(c.path)
                                .collect(),
                            ..c
                        }
                    }));
                    // A role table left empty stays (`inputs = {}`): a section
                    // removed would come back inline, not byte for byte.
                }
                _ => {
                    return Err(bad(format!(
                        "an export op writes only the level's [exports], not `{}`",
                        change.path.join(".")
                    )));
                }
            }
        }
        inverse.reverse();
        let text = doc.to_string();
        let after: DocumentMut = text
            .parse()
            .map_err(|_| bad("the export op leaves a file that does not parse"))?;
        if outside(after) != before {
            return Err(bad("an export op writes only the level's [exports]"));
        }
        self.text = text;
        Ok(inverse)
    }
}
