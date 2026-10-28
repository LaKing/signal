//! ADR 0037, editable insides (2026-10-27): a slot's **own inside**, a
//! project composite `composites/<name>.toml` that the slot loads and whose
//! `[composite] inside_of` names the slot. The owner: "I think the slot
//! should behave like a composite … step into it with a double-click and
//! edit its inside", every slot's inside its own ("Saját példány").
//!
//! This module makes its text (from nothing, from a project or library
//! composite, or wrapping a primitive), names it, reads its marker, and
//! keeps its `[exports]` in step with the slot's interface. Its exports are
//! named after the slot's ports, so the slot matches them by name with no
//! `map`, and the plan is the one the slot had before (ADR 0037 §5: a
//! composite's exports are flattened away). Text only: the plan never
//! reads the marker, and nothing here reaches `signal-core`,
//! `signal-graph` or `signal-engine` (hard rule 1).

use std::collections::BTreeSet;

use signal_core::{Direction, LIBRARY_PREFIX, Registry};
use toml_edit::{DocumentMut, InlineTable, Item, Table, TableLike, Value};

use crate::slot::{Interface, Role};
use crate::{Library, SourceFile, is_project_path};

/// The `[composite]` key that names the slot a composite is the own inside of.
pub const INSIDE_OF: &str = "inside_of";

/// The parameter names no composite may export (`lib.rs`'s reserved keys),
/// left out when a primitive is wrapped.
const RESERVED: [&str; 10] = [
    "module",
    "project",
    "seed",
    "set",
    "slot",
    "load",
    "map",
    "snapshot_exclude",
    "gpu",
    "pattern",
];

fn plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `text` is an `inside_of` value: `<slot>` or `<composite>/<slot>`.
pub fn valid_owner(text: &str) -> bool {
    match text.split_once('/') {
        Some((level, slot)) => plain_name(level) && plain_name(slot),
        None => plain_name(text),
    }
}

/// The `inside_of` value of slot `slot` at `level` (`""`: the top level,
/// else a composite's name): `"fx"`, `"rack/fx"`.
pub fn owner(level: &str, slot: &str) -> String {
    if level.is_empty() {
        slot.to_owned()
    } else {
        format!("{level}/{slot}")
    }
}

/// The key `graph` gives a slot's inside that is not its own yet (its
/// [`crate::SlotResolution::slot`]): `"/fx"`, `"rack/fx"`.
pub fn preview_key(level: &str, slot: &str) -> String {
    format!("{level}/{slot}")
}

/// The `inside_of` of the composite file `text`, if it names one.
pub fn owner_of(text: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(text).ok()?;
    table
        .get("composite")?
        .get(INSIDE_OF)?
        .as_str()
        .map(str::to_owned)
}

/// Whether the composite file `composites/<name>.toml` is among `files`.
pub fn composite_file(files: &[SourceFile], name: &str) -> Option<String> {
    let path = format!("{}/{name}.toml", crate::COMPOSITES);
    files
        .iter()
        .find(|f| f.path == path)
        .map(|f| f.text.clone())
}

/// Whether a new project composite may not be called `name`: a file of the
/// project's has it, or the library has a composite of that name (a direct
/// `lib.<name>` reference beside it would be an error).
pub fn taken(files: &[SourceFile], name: &str) -> bool {
    composite_file(files, name).is_some() || Library::builtin().get(name).is_some()
}

/// The name of the own inside made for slot `slot` at `level`: `fx_inside`,
/// `rack_fx_inside` in composite `rack`, then `_2`, `_3`, … while
/// `taken`. Deterministic for the same files.
pub fn default_name(level: &str, slot: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let base = if level.is_empty() {
        format!("{slot}_inside")
    } else {
        format!("{level}_{slot}_inside")
    };
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|name| !taken(name))
        .unwrap_or(base)
}

/// What a slot's own inside is made from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// An empty slot: an empty inside.
    Empty,
    /// A project composite or a library composite: its file's text,
    /// copied.
    Composite(String),
    /// A primitive type, wrapped: a composite holding one instance of it.
    Primitive {
        /// Its type name (`std.gain`).
        type_name: String,
        /// Its ports and parameters, in descriptor order.
        ports: Vec<(String, Role)>,
    },
}

/// What slot content `load` (as written) is copied from: a project or
/// library composite's text, a primitive's ports; `None` for a project
/// (`./x`, `../x`, `.signal`: not copied, ADR 0037 "Editable insides") and
/// for a reference that names nothing.
pub fn source(files: &[SourceFile], load: &str, registry: &Registry) -> Option<Source> {
    if is_project_path(load) {
        return None;
    }
    if let Some(bare) = load.strip_prefix(LIBRARY_PREFIX) {
        return Library::builtin()
            .get(bare)
            .map(|text| Source::Composite(text.to_owned()));
    }
    if let Some(text) = composite_file(files, load) {
        return Some(Source::Composite(text));
    }
    let module = registry.get(load)?;
    let descriptor = &module.descriptor;
    let mut ports: Vec<(String, Role)> = descriptor
        .ports
        .iter()
        .map(|p| {
            let role = if p.direction == Direction::Out {
                Role::Output
            } else {
                Role::Input
            };
            (p.name.to_owned(), role)
        })
        .collect();
    ports.extend(
        descriptor
            .parameters
            .iter()
            .map(|p| (p.name.to_owned(), Role::Parameter)),
    );
    Some(Source::Primitive {
        type_name: descriptor.type_name.to_owned(),
        ports,
    })
}

/// One slot port and the content name it matches (ADR 0037 §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matched {
    /// The slot's port or parameter.
    pub port: String,
    /// Its role.
    pub role: Role,
    /// The content's name it matches.
    pub content: String,
}

/// The slot ports of `interface` that find a content name of their role in
/// `content` (through `map`: `Some(name)`, `None` for `false`), by §4's
/// candidate rule. Types are not checked here: the load check judges them.
pub fn matches(
    interface: &Interface,
    map: &[(String, Option<String>)],
    content: &[(String, Role)],
) -> Vec<Matched> {
    interface
        .ports
        .iter()
        .filter_map(|p| {
            let candidate = match map.iter().find(|(k, _)| *k == p.name) {
                Some((_, Some(name))) => name.clone(),
                Some((_, None)) => return None,
                None => p.name.clone(),
            };
            content
                .iter()
                .any(|(name, role)| *name == candidate && *role == p.role)
                .then(|| Matched {
                    port: p.name.clone(),
                    role: p.role,
                    content: candidate,
                })
        })
        .collect()
}

fn role_table(role: Role) -> &'static str {
    match role {
        Role::Input => "inputs",
        Role::Output => "outputs",
        Role::Parameter => "parameters",
    }
}

const ROLES: [Role; 3] = [Role::Input, Role::Output, Role::Parameter];

/// The exports a content name keeps in the inside: each slot port matched
/// to `content` of `role`, by the port's name; else `content` itself unless
/// the interface has that name (it is then the slot's port, unmatched or
/// mapped elsewhere, and must stay so).
fn renamed(
    content: &str,
    role: Role,
    matched: &[Matched],
    interface: &BTreeSet<String>,
) -> Vec<String> {
    let ports: Vec<String> = matched
        .iter()
        .filter(|m| m.role == role && m.content == content)
        .map(|m| m.port.clone())
        .collect();
    if !ports.is_empty() {
        ports
    } else if interface.contains(content) {
        Vec::new()
    } else {
        vec![content.to_owned()]
    }
}

/// The text of the own inside `name` of slot `owner` made from `source`,
/// its exports named after the slot's ports (`matched`, `interface` the
/// slot's port and parameter names), so the slot matches them by name
/// with no `map`. A copied file keeps every other byte it can (comments,
/// order, ranges, groups); its `[composite] name` becomes `name` and
/// `inside_of` is set.
pub fn text(
    name: &str,
    owner: &str,
    source: &Source,
    matched: &[Matched],
    interface: &BTreeSet<String>,
) -> Result<String, String> {
    match source {
        Source::Empty => Ok(format!(
            "schema_version = 1\n\n[composite]\nname = {}\n{INSIDE_OF} = {}\n\n[instances]\n",
            Value::from(name),
            Value::from(owner)
        )),
        Source::Composite(text) => copied(text, name, owner, matched, interface),
        Source::Primitive { type_name, ports } => {
            Ok(wrapped(name, owner, type_name, ports, matched, interface))
        }
    }
}

fn copied(
    text: &str,
    name: &str,
    owner: &str,
    matched: &[Matched],
    interface: &BTreeSet<String>,
) -> Result<String, String> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("the content's file does not parse: {e}"))?;
    let composite = doc
        .get_mut("composite")
        .and_then(Item::as_table_like_mut)
        .ok_or("the content's file has no [composite]")?;
    set_keeping(composite, "name", Value::from(name));
    set_keeping(composite, INSIDE_OF, Value::from(owner));
    if let Some(exports) = doc.get_mut("exports").and_then(Item::as_table_like_mut) {
        for role in ROLES {
            let Some(table) = exports
                .get_mut(role_table(role))
                .and_then(Item::as_table_like_mut)
            else {
                continue;
            };
            let old: Vec<(String, Item)> = table
                .iter()
                .map(|(k, v)| (k.to_owned(), v.clone()))
                .collect();
            let new: Vec<(String, Item)> = old
                .iter()
                .flat_map(|(content, item)| {
                    renamed(content, role, matched, interface)
                        .into_iter()
                        .map(move |port| (port, item.clone()))
                })
                .collect();
            if new.iter().map(|(k, _)| k).eq(old.iter().map(|(k, _)| k)) {
                continue;
            }
            rebuild(table, new);
        }
    }
    Ok(doc.to_string())
}

/// `key` set to `value` in `table`, an existing value's spacing and comment kept.
fn set_keeping(table: &mut dyn TableLike, key: &str, mut value: Value) {
    if let Some(old) = table.get_mut(key).and_then(Item::as_value_mut) {
        *value.decor_mut() = old.decor().clone();
        *old = value;
    } else {
        table.insert(key, Item::Value(value));
    }
}

/// `table`'s entries replaced by `entries`, in order; an inline table formatted.
fn rebuild(table: &mut dyn TableLike, entries: Vec<(String, Item)>) {
    table.clear();
    for (key, item) in entries {
        table.insert(&key, item);
    }
    table.fmt();
}

fn wrapped(
    name: &str,
    owner: &str,
    type_name: &str,
    ports: &[(String, Role)],
    matched: &[Matched],
    interface: &BTreeSet<String>,
) -> String {
    let inner = inner_name(type_name);
    let mut doc = DocumentMut::new();
    doc.insert("schema_version", Item::Value(Value::from(1i64)));
    let mut composite = Table::new();
    composite.insert("name", Item::Value(Value::from(name)));
    composite.insert(INSIDE_OF, Item::Value(Value::from(owner)));
    doc.insert("composite", Item::Table(composite));
    let mut instances = Table::new();
    let mut entry = InlineTable::new();
    entry.insert("module", Value::from(type_name));
    entry.fmt();
    instances.insert(&inner, Item::Value(Value::InlineTable(entry)));
    doc.insert("instances", Item::Table(instances));
    let mut exports = Table::new();
    for role in ROLES {
        let mut table = InlineTable::new();
        for (content, _) in ports.iter().filter(|(_, r)| *r == role) {
            if role == Role::Parameter && RESERVED.contains(&content.as_str()) {
                continue;
            }
            for port in renamed(content, role, matched, interface) {
                table.insert(&port, Value::from(format!("{inner}.{content}")));
            }
        }
        if !table.is_empty() {
            table.fmt();
            exports.insert(role_table(role), Item::Value(Value::InlineTable(table)));
        }
    }
    if !exports.is_empty() {
        doc.insert("exports", Item::Table(exports));
    }
    doc.to_string()
}

/// The instance name a wrapped primitive gets: its type's last segment
/// (`std.sine_osc` → `sine_osc`), made a valid name.
pub fn inner_name(type_name: &str) -> String {
    let last = type_name.rsplit('.').next().unwrap_or(type_name);
    let mut name: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        name.insert_str(0, "m_");
    }
    name
}

/// The export `old` (of any role) of composite file `text` renamed `new`,
/// in its place, everything else kept; `None` when it has none.
pub fn rename_export(text: &str, old: &str, new: &str) -> Option<String> {
    // Exports share one namespace: never onto one that is there.
    if has_export(text, new) {
        return None;
    }
    let mut doc: DocumentMut = text.parse().ok()?;
    let exports = doc.get_mut("exports")?.as_table_like_mut()?;
    for role in ROLES {
        let Some(table) = exports
            .get_mut(role_table(role))
            .and_then(Item::as_table_like_mut)
        else {
            continue;
        };
        if table.get(old).is_none() {
            continue;
        }
        let entries: Vec<(String, Item)> = table
            .iter()
            .map(|(k, v)| {
                let key = if k == old { new } else { k };
                (key.to_owned(), v.clone())
            })
            .collect();
        rebuild(table, entries);
        return Some(doc.to_string());
    }
    None
}

/// Whether composite file `text` exports `name` (of any role).
pub fn has_export(text: &str, name: &str) -> bool {
    let Ok(table) = toml::from_str::<toml::Table>(text) else {
        return false;
    };
    let exports = table.get("exports").and_then(toml::Value::as_table);
    ["inputs", "outputs", "parameters"].iter().any(|role| {
        exports
            .and_then(|e| e.get(*role))
            .and_then(toml::Value::as_table)
            .is_some_and(|t| t.contains_key(name))
    })
}

/// The export `name` (of any role) of composite file `text` removed,
/// everything else kept; `None` when it has none.
pub fn remove_export(text: &str, name: &str) -> Option<String> {
    let mut doc: DocumentMut = text.parse().ok()?;
    let exports = doc.get_mut("exports")?.as_table_like_mut()?;
    for role in ROLES {
        let Some(table) = exports
            .get_mut(role_table(role))
            .and_then(Item::as_table_like_mut)
        else {
            continue;
        };
        if table.remove(name).is_some() {
            table.fmt();
            if table.is_empty() {
                exports.remove(role_table(role));
            }
            return Some(doc.to_string());
        }
    }
    None
}
