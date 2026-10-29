//! ADR 0046 §7–8: Group into slot (`slot.group`), a slot's part removed
//! with the slot, and the presentation files' inverse (`panel.restore`).
//!
//! `slot.group` rewrites one instance entry: its `module` (or `project`)
//! key becomes `slot = { … }` in its place, followed by `load`, every other
//! key kept where it is. The interface names what the level already names
//! on the instance, by the same names, so no wire is rewritten and no `map`
//! is needed: a loaded slot plans as the explicit instance (ADR 0037 §5),
//! and so the plan is byte-identical. Its inverse is a `slot.restore` that
//! writes the `module` or `project` key back.
//!
//! A slot's part (`ui/parts/<key>.html`) is the slot's, not the content's:
//! an `instance.remove` of a slot removes it as one more of the patch's
//! files, and `panel.restore` gives it back byte for byte. The text edits
//! of the panels themselves (moving controls into a part) are the host's
//! (`signal-ui`): this module only carries their files.

use std::collections::BTreeMap;

use toml_edit::{DocumentMut, Item as TomlItem};

use super::slot::{Change, entry, is_slot, json_text};
use super::{Op, PatchError, Patching, bad, refused, valid_name};
use crate::slot::Role;

/// The key of the part of slot `slot` at `level` (ADR 0046 §2): `slot` at
/// the top level, `<composite>.<slot>` in a composite's file.
pub fn part_key(level: &str, slot: &str) -> String {
    if level.is_empty() {
        slot.to_owned()
    } else {
        format!("{level}.{slot}")
    }
}

/// The part's file, relative to the project folder: `ui/parts/<key>.html`.
pub fn part_path(level: &str, slot: &str) -> String {
    format!("ui/parts/{}.html", part_key(level, slot))
}

/// Whether `file` (relative to `ui/`) is a presentation file a
/// `panel.restore` may write: `main.html`, `windows.toml` or a part,
/// `parts/<slot>.html` or `parts/<composite>.<slot>.html`.
pub fn restorable(file: &str) -> bool {
    if file == "main.html" || file == "windows.toml" {
        return true;
    }
    let Some(key) = file
        .strip_prefix("parts/")
        .and_then(|f| f.strip_suffix(".html"))
    else {
        return false;
    };
    match key.split_once('.') {
        Some((composite, slot)) => valid_name(composite) && valid_name(slot),
        None => valid_name(key),
    }
}

/// The keys of an instance entry that are not parameter values.
const RESERVED: [&str; 10] = [
    "module",
    "project",
    "slot",
    "load",
    "map",
    "seed",
    "set",
    "gpu",
    "pattern",
    "snapshot_exclude",
];

/// The ports of `instance` the level's `[wiring]` and `[exports]` name
/// (`instance.<port>` strings anywhere under them), in text order.
fn named_on(text: &str, instance: &str) -> Vec<String> {
    fn walk(value: &toml::Value, prefix: &str, found: &mut Vec<String>) {
        match value {
            toml::Value::String(s) => {
                if let Some(port) = s.strip_prefix(prefix)
                    && !port.is_empty()
                    && !found.iter().any(|f| f == port)
                {
                    found.push(port.to_owned());
                }
            }
            toml::Value::Array(items) => items.iter().for_each(|v| walk(v, prefix, found)),
            toml::Value::Table(t) => t.values().for_each(|v| walk(v, prefix, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    let prefix = format!("{instance}.");
    if let Ok(table) = toml::from_str::<toml::Table>(text) {
        for key in ["wiring", "exports"] {
            if let Some(value) = table.get(key) {
                walk(value, &prefix, &mut found);
            }
        }
    }
    found
}

impl Patching<'_> {
    /// `slot.group`: instance `instance` made a slot that loads what it
    /// was. Refused for an instance that is already a slot, or has
    /// `pattern` or `gpu` (no slot key: wrapping it would move its
    /// addresses a level down).
    pub(super) fn slot_group(&mut self, instance: &str) -> Result<Vec<Op>, PatchError> {
        let m = self.level_map()?;
        if !m.entries.iter().any(|e| e.name == instance) {
            return Err(refused(
                "unknown-instance",
                format!("no instance `{instance}` at this level"),
            ));
        }
        if is_slot(&self.text, instance) {
            return Err(bad(format!("`{instance}` is a slot already")));
        }
        let mut doc: DocumentMut = self
            .text
            .parse()
            .map_err(|_| refused("does-not-load", "the file does not parse"))?;
        let tab = entry(&mut doc, instance).ok_or_else(|| {
            bad(format!(
                "`{instance}` is not written as a table: group it by hand"
            ))
        })?;
        for key in ["pattern", "gpu"] {
            if tab.get(key).is_some() {
                return Err(bad(format!(
                    "`{instance}` has `{key}`, which is no slot key: it is not grouped (wrapping it into an own inside would move its addresses a level down)"
                )));
            }
        }
        let (key, content) = match (
            tab.get("module").and_then(TomlItem::as_str),
            tab.get("project").and_then(TomlItem::as_str),
        ) {
            (Some(module), None) => ("module", module.to_owned()),
            (None, Some(project)) => ("project", project.to_owned()),
            _ => {
                return Err(bad(format!(
                    "`{instance}` has no `module` or `project` to load"
                )));
            }
        };
        // A bare project name is a module reference in `load`: the same
        // folder, spelt as a path.
        let load = if key == "project" && !crate::is_project_path(&content) {
            format!("./{content}")
        } else {
            content.clone()
        };
        let names = (self.ctx.content)(&load).ok_or_else(|| {
            refused(
                "unknown-module",
                format!("`{content}` does not resolve: `{instance}` is not grouped"),
            )
        })?;
        let keys = tab.keys();
        let at = keys.iter().position(|k| k == key).unwrap_or(0);
        let decor = tab.decor(key);
        let values: Vec<String> = keys
            .iter()
            .filter(|k| !RESERVED.contains(&k.as_str()))
            .cloned()
            .collect();
        let excluded: Vec<String> = tab
            .get("snapshot_exclude")
            .and_then(TomlItem::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        // What the level names on the instance: its wires and exports, the
        // panels' bindings, the values it sets.
        let mut wanted: Vec<String> = named_on(&self.text, instance);
        for name in (self.ctx.bound)(instance)
            .into_iter()
            .chain(values)
            .chain(excluded)
        {
            if !wanted.contains(&name) {
                wanted.push(name);
            }
        }
        let mut roles: BTreeMap<Role, Vec<String>> = BTreeMap::new();
        for port in &names.ports {
            if wanted.contains(&port.name) {
                roles.entry(port.role).or_default().push(port.name.clone());
            }
        }
        let mut tables = Vec::new();
        for (role, table) in [
            (Role::Input, "inputs"),
            (Role::Output, "outputs"),
            (Role::Parameter, "parameters"),
        ] {
            let Some(list) = roles.get(&role).filter(|l| !l.is_empty()) else {
                continue;
            };
            let entries: Vec<String> = list
                .iter()
                .map(|n| format!("{} = {{}}", toml_key(n)))
                .collect();
            tables.push(format!("{table} = {{ {} }}", entries.join(", ")));
        }
        let interface = if tables.is_empty() {
            "{}".to_owned()
        } else {
            format!("{{ {} }}", tables.join(", "))
        };
        let changes = vec![
            Change {
                path: vec![key.to_owned()],
                value: None,
                at: None,
                decor: None,
            },
            Change {
                path: vec!["slot".to_owned()],
                value: Some(interface),
                at: Some(at),
                decor,
            },
            Change {
                path: vec!["load".to_owned()],
                value: Some(json_text(&serde_json::json!(load))?),
                at: Some(at + 1),
                decor: None,
            },
        ];
        let (changes, wires) = self.restore(instance, &changes, &BTreeMap::new())?;
        Ok(vec![Op::SlotRestore {
            slot: instance.to_owned(),
            changes,
            wires,
        }])
    }

    /// ADR 0046 §7: slot `slot`'s part removed with it; the `panel.restore`
    /// that gives it back, when it had one.
    pub(super) fn remove_part(&mut self, slot: &str) -> Result<Option<Op>, PatchError> {
        let path = part_path(self.ctx.level, slot);
        let Some(now) = self.file_now(&path) else {
            return Ok(None);
        };
        self.set_file(&path, None);
        Ok(Some(Op::PanelRestore {
            file: path.trim_start_matches("ui/").to_owned(),
            text: Some(now),
            was: None,
        }))
    }

    /// `panel.restore`: the presentation file `file` (relative to `ui/`)
    /// set to `text` while it holds `was`. What the text may hold is the
    /// host's to check (the panel whitelist is `signal-ui`'s).
    pub(super) fn panel_restore(
        &mut self,
        file: &str,
        text: &Option<String>,
        was: &Option<String>,
    ) -> Result<Op, PatchError> {
        if !restorable(file) {
            return Err(bad(format!(
                "panel.restore writes ui/main.html, ui/windows.toml or a slot's part (ui/parts/<slot>.html), not ui/{file}"
            )));
        }
        let path = format!("ui/{file}");
        let now = self.file_now(&path);
        if now != *was {
            return Err(bad(format!("{path} is not the text the op names")));
        }
        if text.is_none() && now.is_none() {
            return Err(bad(format!("{path}: nothing to do")));
        }
        self.set_file(&path, text.clone());
        Ok(Op::PanelRestore {
            file: file.to_owned(),
            text: now,
            was: text.clone(),
        })
    }

    /// The patch's other file `path` set to `text` (`None`: removed).
    pub(super) fn set_file(&mut self, path: &str, text: Option<String>) {
        let now = self.file_now(path);
        let entry = self
            .files
            .entry(path.to_owned())
            .or_insert_with(|| (now.clone(), now));
        entry.1 = text;
    }
}

/// A TOML key: bare when it can be.
fn toml_key(name: &str) -> String {
    if !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        name.to_owned()
    } else {
        json_text(&serde_json::json!(name)).unwrap_or_else(|_| format!("\"{name}\""))
    }
}
