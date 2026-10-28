//! The slot ops of `patch` (ADR 0037 §2, §7, phase 3): a slot added, its
//! content loaded, replaced or unloaded, its `map` written, its interface
//! edited (ports and parameters added, removed, renamed, retyped; detached
//! from its template or set to one). Each writes only the slot's entry in
//! the level's file (and, for a rename, the level's wires and exports that
//! name the port), key by key with `toml_edit`, so everything else in the
//! text stays as it was: comments, order, formatting.
//!
//! The inverse of an entry edit is `slot.restore`: the keys it changed,
//! each with its old value as written (TOML text) and its old place in its
//! table, and the wires to rename back. An undo puts back what the keys
//! said, byte for byte for the values; a key that a section held
//! (`[instances.x.slot]`) comes back inline. Untrusted, it may touch only
//! the slot's own keys (`slot`, `load`, `map`, `set`, `seed` and its
//! interface parameters' values) and the wires on the slot's ports, and the
//! load that follows checks what it wrote.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, InlineTable, Item as TomlItem, Table, Value};

use super::{PatchError, Patching, Scheme, bad, map, refused, written};
use crate::model::KINDS;
use crate::slot::{Interface, InterfacePort, Role};

/// One key a slot op changed, relative to the slot's entry: its `path`
/// (`["slot", "inputs", "in"]`, `["load"]`), its value as written (TOML
/// text; `None`: absent) and its place among its table's keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// The keys from the entry down.
    pub path: Vec<String>,
    /// The value, as TOML text; `None` removes the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Its place among its table's keys (`None`: appended).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<usize>,
    /// In a section (`[instances.x]`), the key's line as written around
    /// the key and the value: `[key prefix, key suffix, value prefix, value
    /// suffix]` (the comment lines above it, its trailing comment), so an
    /// undo puts them back; absent in an inline table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decor: Option<Vec<String>>,
}

/// A port or parameter as `slot.interface` names it (ADR 0037 §3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortJson {
    /// Its name.
    pub name: String,
    /// `input`, `output` or `parameter`; for a retype, absent keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// `"signal"` (default), `"events"` or `{"vector": N}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<serde_json::Value>,
    /// A rate divisor, default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<u32>,
    /// A port kind, default `generic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Presentation only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Presentation only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// Presentation only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Role, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "input" | "in" => Ok(Role::Input),
            "output" | "out" => Ok(Role::Output),
            "parameter" => Ok(Role::Parameter),
            other => Err(serde::de::Error::custom(format!(
                "unknown role `{other}`; roles: input, output, parameter"
            ))),
        }
    }
}

/// The keys of an instance entry a slot op may write: the slot's own, and
/// any other plain name (an interface parameter's value).
fn writable(first: &str) -> bool {
    match first {
        "slot" | "load" | "map" | "set" | "seed" | "snapshot_exclude" => true,
        "module" | "project" | "gpu" | "pattern" => false,
        name => super::valid_name(name),
    }
}

/// The table of a role in a slot's interface.
fn role_key(role: Role) -> &'static str {
    match role {
        Role::Input => "inputs",
        Role::Output => "outputs",
        Role::Parameter => "parameters",
    }
}

// A table of either form, edited in place.

pub(super) enum Tab<'a> {
    Std(&'a mut Table),
    Inl(&'a mut InlineTable),
}

/// The TOML text of `item` as a value (a section as an inline table).
pub(super) fn item_text(item: &TomlItem) -> Option<String> {
    match item {
        TomlItem::Value(value) => {
            let mut value = value.clone();
            value.decor_mut().clear();
            Some(value.to_string())
        }
        TomlItem::Table(table) => {
            let mut inline = table.clone().into_inline_table();
            inline.fmt();
            Some(Value::InlineTable(inline).to_string())
        }
        _ => None,
    }
}

/// `text` read as one TOML value.
pub(super) fn parse_value(text: &str) -> Result<Value, PatchError> {
    let doc: DocumentMut = format!("v = {text}\n")
        .parse()
        .map_err(|_| bad(format!("`{text}` is not a TOML value")))?;
    let mut value = doc
        .get("v")
        .and_then(TomlItem::as_value)
        .cloned()
        .ok_or_else(|| bad(format!("`{text}` is not one TOML value")))?;
    value.decor_mut().clear();
    Ok(value)
}

impl Tab<'_> {
    pub(super) fn keys(&self) -> Vec<String> {
        match self {
            Tab::Std(t) => t.iter().map(|(k, _)| k.to_owned()).collect(),
            Tab::Inl(t) => t.iter().map(|(k, _)| k.to_owned()).collect(),
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<&TomlItem> {
        match self {
            Tab::Std(t) => t.get(key),
            Tab::Inl(t) => toml_edit::TableLike::get(&**t, key),
        }
    }

    /// The key's value as written, and its place.
    fn read(&self, key: &str) -> (Option<String>, Option<usize>) {
        let at = self.keys().iter().position(|k| k == key);
        (self.get(key).and_then(item_text), at)
    }

    /// A section's key line as written around `key` and its value (see
    /// [`Change::decor`]); `None` in an inline table, or for a section.
    fn decor(&self, key: &str) -> Option<Vec<String>> {
        let Tab::Std(t) = self else {
            return None;
        };
        let value = t.get(key)?.as_value()?;
        let text = |raw: Option<&toml_edit::RawString>| {
            raw.and_then(toml_edit::RawString::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let leaf = t.key(key)?.leaf_decor();
        Some(vec![
            text(leaf.prefix()),
            text(leaf.suffix()),
            text(value.decor().prefix()),
            text(value.decor().suffix()),
        ])
    }

    /// Sets (or removes, `None`) `key`; an existing value keeps its spacing
    /// and trailing comment, a new one goes at `at` (else at the end).
    fn write(&mut self, key: &str, value: Option<Value>, at: Option<usize>) {
        self.write_decorated(key, value, at, None);
    }

    /// [`Self::write`], a new key in a section written with `decor` (its
    /// line as it was, [`Change::decor`]).
    fn write_decorated(
        &mut self,
        key: &str,
        value: Option<Value>,
        at: Option<usize>,
        decor: Option<&[String]>,
    ) {
        match (self, value) {
            (Tab::Std(t), None) => {
                t.remove(key);
            }
            (Tab::Inl(t), None) => {
                let last = t.iter().last().map(|(k, _)| k.to_owned());
                if let Some(removed) = t.remove(key)
                    && last.as_deref() == Some(key)
                    && let Some((_, now_last)) = t.iter_mut().last()
                {
                    let suffix = removed.decor().suffix().cloned().unwrap_or_default();
                    now_last.decor_mut().set_suffix(suffix);
                }
            }
            (Tab::Std(t), Some(mut value)) => {
                if let Some(old) = t.get_mut(key).and_then(TomlItem::as_value_mut) {
                    *value.decor_mut() = old.decor().clone();
                    *old = value;
                    return;
                }
                if t.get(key).is_some() {
                    // A section replaced by a value.
                    t.remove(key);
                }
                let keys: Vec<String> = t.iter().map(|(k, _)| k.to_owned()).collect();
                let at = at.unwrap_or(keys.len()).min(keys.len());
                let tail: Vec<_> = keys[at..]
                    .iter()
                    .filter_map(|k| t.remove_entry(k))
                    .collect();
                match decor {
                    Some([kp, ks, vp, vs]) => {
                        let value = value.decorated(vp.as_str(), vs.as_str());
                        let new = toml_edit::Key::new(key)
                            .with_leaf_decor(toml_edit::Decor::new(kp.as_str(), ks.as_str()));
                        t.insert_formatted(&new, TomlItem::Value(value));
                    }
                    _ => {
                        t.insert(key, TomlItem::Value(value));
                    }
                }
                for (key, item) in tail {
                    t.insert_formatted(&key, item);
                }
            }
            (Tab::Inl(t), Some(mut value)) => {
                if let Some(old) = t.get_mut(key) {
                    *value.decor_mut() = old.decor().clone();
                    *old = value;
                    return;
                }
                let keys: Vec<String> = t.iter().map(|(k, _)| k.to_owned()).collect();
                let at = at.unwrap_or(keys.len()).min(keys.len());
                let tail: Vec<_> = keys[at..]
                    .iter()
                    .filter_map(|k| t.remove_entry(k))
                    .collect();
                let mut value = value.decorated(" ", "");
                if tail.is_empty()
                    && let Some((_, before)) = t.iter_mut().last()
                {
                    // Appended: it takes over the spacing before `}`.
                    let suffix = before.decor().suffix().cloned().unwrap_or_default();
                    before.decor_mut().set_suffix("");
                    value.decor_mut().set_suffix(suffix);
                } else if tail.is_empty() {
                    value.decor_mut().set_suffix(" ");
                }
                let new = toml_edit::Key::new(key).with_leaf_decor(toml_edit::Decor::new(" ", " "));
                t.insert_formatted(&new, value);
                for (key, value) in tail {
                    t.insert_formatted(&key, value);
                }
            }
        }
    }
}

impl<'a> Tab<'a> {
    /// The table at `key`, made one when `create`: a missing key becomes an
    /// empty inline table, a template name (`slot = "lib.voice"`) its table
    /// form (`{ from = "lib.voice" }`).
    fn into_child(mut self, key: &str, create: bool) -> Option<Tab<'a>> {
        let is_str = self.get(key).and_then(TomlItem::as_str).map(str::to_owned);
        if let Some(from) = is_str {
            if !create || key != "slot" {
                return None;
            }
            let mut table = InlineTable::new();
            table.insert("from", Value::from(from));
            self.write(key, Some(Value::InlineTable(table)), None);
        } else if self.get(key).is_none() {
            if !create {
                return None;
            }
            self.write(key, Some(Value::InlineTable(InlineTable::new())), None);
        }
        let item = match self {
            Tab::Std(t) => t.get_mut(key)?,
            Tab::Inl(t) => toml_edit::TableLike::get_mut(t, key)?,
        };
        match item {
            TomlItem::Table(t) => Some(Tab::Std(t)),
            TomlItem::Value(Value::InlineTable(t)) => Some(Tab::Inl(t)),
            _ => None,
        }
    }
}

/// The entry of instance `name` in `doc`'s `[instances]`.
fn entry<'d>(doc: &'d mut DocumentMut, name: &str) -> Option<Tab<'d>> {
    let instances = doc.get_mut("instances")?;
    let item = match instances {
        TomlItem::Table(t) => t.get_mut(name)?,
        TomlItem::Value(Value::InlineTable(t)) => toml_edit::TableLike::get_mut(t, name)?,
        _ => return None,
    };
    match item {
        TomlItem::Table(t) => Some(Tab::Std(t)),
        TomlItem::Value(Value::InlineTable(t)) => Some(Tab::Inl(t)),
        _ => None,
    }
}

/// Applies `change` to slot `slot`'s entry in `doc`; returns its inverse,
/// preceded by the removals of the tables it had to make (outermost first),
/// so that the inverse list, applied in reverse, leaves none behind.
fn apply_change(
    doc: &mut DocumentMut,
    slot: &str,
    change: &Change,
) -> Result<Vec<Change>, PatchError> {
    let tab = entry(doc, slot).ok_or_else(|| {
        refused(
            "unknown-instance",
            format!("no instance `{slot}` at this level"),
        )
    })?;
    apply_change_in(tab, change)
}

/// Applies `change` (its path relative to `tab`) in `tab`; returns its
/// inverse as [`apply_change`] does.
pub(super) fn apply_change_in(
    mut tab: Tab<'_>,
    change: &Change,
) -> Result<Vec<Change>, PatchError> {
    let (last, parents) = change
        .path
        .split_last()
        .ok_or_else(|| bad("a change names no key"))?;
    let value = change.value.as_deref().map(parse_value).transpose()?;
    if let Some(decor) = &change.decor
        && !decor_ok(decor)
    {
        return Err(bad(
            "a change's `decor` is spacing and comments only: [key prefix, key suffix, value prefix, value suffix]",
        ));
    }
    // Parents are made only for a value written; a removal under a missing
    // table is already done.
    let mut inverse = Vec::new();
    for (depth, key) in parents.iter().enumerate() {
        let exists = tab.get(key).is_some_and(|i| i.as_str().is_none());
        if !exists && value.is_none() {
            return Ok(vec![change.clone()]);
        }
        if tab.get(key).is_none() {
            inverse.push(Change {
                path: change.path[..=depth].to_vec(),
                value: None,
                at: None,
                decor: None,
            });
        }
        tab = tab
            .into_child(key, value.is_some())
            .ok_or_else(|| bad(format!("`{}` is not a table", change.path.join("."))))?;
    }
    let (old, at) = tab.read(last);
    let decor = tab.decor(last);
    tab.write_decorated(last, value, change.at, change.decor.as_deref());
    inverse.push(Change {
        path: change.path.clone(),
        value: old,
        at,
        decor,
    });
    Ok(inverse)
}

/// Whether `decor` ([`Change::decor`], untrusted) is a key line's spacing
/// and comments only: the prefix whole lines, each blank or a `#` comment;
/// the key's suffix and the value's prefix spaces and tabs; the value's
/// suffix spaces and tabs, then at most a `#` comment, on the line.
fn decor_ok(decor: &[String]) -> bool {
    let plain = |c: char| c == ' ' || c == '\t';
    let comment = |text: &str| text.chars().all(|c| c == '\t' || !c.is_control());
    let [prefix, key_suffix, value_prefix, value_suffix] = decor else {
        return false;
    };
    let prefix_ok = prefix.split('\n').all(|line| {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let rest = line.trim_start_matches(plain);
        rest.is_empty() || (rest.starts_with('#') && comment(rest))
    });
    let suffix_ok = {
        let rest = value_suffix.trim_start_matches(plain);
        rest.is_empty() || (rest.starts_with('#') && comment(rest))
    };
    prefix_ok && key_suffix.chars().all(plain) && value_prefix.chars().all(plain) && suffix_ok
}

/// `doc`'s text without instance `name`'s entry: what an edit of that entry
/// alone leaves unchanged.
fn without_entry(mut doc: DocumentMut, name: &str) -> String {
    match doc.get_mut("instances") {
        Some(TomlItem::Table(t)) => {
            t.remove(name);
        }
        Some(TomlItem::Value(Value::InlineTable(t))) => {
            t.remove(name);
        }
        _ => {}
    }
    doc.to_string()
}

/// Whether the entry of `name` in `text` is a slot (has a `slot` key).
pub(super) fn is_slot(text: &str, name: &str) -> bool {
    let Ok(mut doc) = text.parse::<DocumentMut>() else {
        return false;
    };
    entry(&mut doc, name).is_some_and(|t| t.get("slot").is_some())
}

/// A slot's `slot` key, read: the template it names, the names it removes,
/// its own entries (name → role) and the interface it resolves to.
pub(super) struct Read {
    from: Option<String>,
    remove: Vec<String>,
    own: BTreeMap<String, Role>,
    pub(super) interface: Interface,
    template: Option<Interface>,
    pub(super) load: Option<String>,
    pub(super) map: Vec<(String, Option<String>)>,
    set: Vec<String>,
    seed: bool,
    values: Vec<String>,
    /// `snapshot_exclude` (ADR 0041 §8), names of the interface's parameters.
    exclude: Vec<String>,
}

#[derive(Deserialize)]
struct SlotOnly {
    slot: toml::Spanned<crate::model::SlotSpec>,
}

pub(super) fn read_slot(
    text: &str,
    name: &str,
    templates: &BTreeMap<String, Interface>,
) -> Result<Read, PatchError> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|_| refused("does-not-load", "the file does not parse"))?;
    let tab = entry(&mut doc, name).ok_or_else(|| {
        refused(
            "unknown-instance",
            format!("no instance `{name}` at this level"),
        )
    })?;
    let slot_text = tab
        .get("slot")
        .and_then(item_text)
        .ok_or_else(|| bad(format!("`{name}` is not a slot (it has no `slot` key)")))?;
    let spec: SlotOnly = toml::from_str(&format!("slot = {slot_text}\n"))
        .map_err(|e| bad(format!("{name}: its `slot` does not read: {}", e.message())))?;
    let spec = spec.slot.into_inner();
    let interface = crate::slot::resolve(&spec, 0, templates).map_err(|problems| {
        bad(format!(
            "{name}: its interface does not resolve: {}",
            problems
                .into_iter()
                .map(|(_, m)| m)
                .collect::<Vec<_>>()
                .join("; ")
        ))
    })?;
    let from = spec.from.as_ref().map(|f| f.get_ref().clone());
    let template = from.as_deref().and_then(|f| {
        if f.starts_with(crate::slot::LIBRARY_TEMPLATE) {
            crate::slot::library_template(f)
        } else {
            templates.get(f).cloned()
        }
    });
    let mut own = BTreeMap::new();
    for (role, table) in [
        (
            Role::Input,
            spec.inputs
                .keys()
                .map(|k| k.get_ref().clone())
                .collect::<Vec<_>>(),
        ),
        (
            Role::Output,
            spec.outputs.keys().map(|k| k.get_ref().clone()).collect(),
        ),
        (
            Role::Parameter,
            spec.parameters
                .keys()
                .map(|k| k.get_ref().clone())
                .collect(),
        ),
    ] {
        for key in table {
            own.insert(key, role);
        }
    }
    let load = tab
        .get("load")
        .and_then(TomlItem::as_str)
        .map(str::to_owned);
    let map_entries = |item: Option<&TomlItem>| -> Vec<(String, Option<String>)> {
        item.and_then(TomlItem::as_table_like)
            .map(|t| {
                t.iter()
                    .map(|(k, v)| (k.to_owned(), v.as_str().map(str::to_owned)))
                    .collect()
            })
            .unwrap_or_default()
    };
    let map = map_entries(tab.get("map"));
    let set = map_entries(tab.get("set"))
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let values = tab
        .keys()
        .into_iter()
        .filter(|k| {
            !["slot", "load", "map", "set", "seed", "snapshot_exclude"].contains(&k.as_str())
        })
        .collect();
    let exclude = tab
        .get("snapshot_exclude")
        .and_then(TomlItem::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(Read {
        from,
        remove: spec.remove.iter().map(|r| r.get_ref().clone()).collect(),
        own,
        interface,
        template,
        load,
        map,
        set,
        seed: tab.get("seed").is_some(),
        values,
        exclude,
    })
}

/// A port's declaration as TOML text, in its shortest form: `"audio"`,
/// `"events"` or `{ shape = …, rate = …, kind = …, unit = …, range = […], group = … }`.
pub(crate) fn port_text(port: &InterfacePort) -> String {
    let mut table = InlineTable::new();
    if port.role == Role::Parameter {
        if let Some(unit) = &port.unit {
            table.insert("unit", Value::from(unit.as_str()));
        }
        if let Some(group) = &port.group {
            table.insert("group", Value::from(group.as_str()));
        }
        table.fmt();
        return Value::InlineTable(table).to_string().trim().to_owned();
    }
    let plain =
        port.unit.is_none() && port.range.is_none() && port.group.is_none() && port.rate == 1;
    let kind = crate::slot::kind_name(port.kind);
    match port.shape {
        signal_core::PortShape::Signal if plain => return format!("\"{kind}\""),
        signal_core::PortShape::Events if plain && kind == "generic" => {
            return "\"events\"".to_owned();
        }
        signal_core::PortShape::Events => {
            table.insert("shape", Value::from("events"));
        }
        signal_core::PortShape::Vector(n) => {
            let mut shape = InlineTable::new();
            shape.insert("vector", Value::from(i64::from(n)));
            table.insert("shape", Value::InlineTable(shape));
        }
        _ => {}
    }
    if port.rate != 1 {
        table.insert("rate", Value::from(i64::from(port.rate)));
    }
    if kind != "generic" {
        table.insert("kind", Value::from(kind));
    }
    if let Some(unit) = &port.unit {
        table.insert("unit", Value::from(unit.as_str()));
    }
    if let Some((min, max)) = port.range {
        let mut range = toml_edit::Array::new();
        range.push(crate::write::number(min));
        range.push(crate::write::number(max));
        table.insert("range", Value::Array(range));
    }
    if let Some(group) = &port.group {
        table.insert("group", Value::from(group.as_str()));
    }
    table.fmt();
    Value::InlineTable(table).to_string().trim().to_owned()
}

/// `port` as an interface port, `base` the declaration it retypes (its
/// fields are kept where `port` gives none).
fn interface_port(
    port: &PortJson,
    base: Option<&InterfacePort>,
) -> Result<InterfacePort, PatchError> {
    if let Some(problem) = name_problem(&port.name) {
        return Err(bad(problem));
    }
    let role = port.role.or(base.map(|b| b.role)).ok_or_else(|| {
        bad(format!(
            "`{}` needs a `role`: input, output or parameter",
            port.name
        ))
    })?;
    let shape = match &port.shape {
        None => base
            .filter(|b| b.role == role)
            .map_or(signal_core::PortShape::Signal, |b| b.shape),
        Some(serde_json::Value::String(s)) if s == "signal" => signal_core::PortShape::Signal,
        Some(serde_json::Value::String(s)) if s == "events" => signal_core::PortShape::Events,
        Some(serde_json::Value::Object(o)) => {
            match o.get("vector").and_then(serde_json::Value::as_u64) {
                Some(n) if n >= 1 && n <= u64::from(u32::MAX) && o.len() == 1 => {
                    signal_core::PortShape::Vector(n as u32)
                }
                _ => return Err(bad("a vector shape is {\"vector\": N}, N at least 1")),
            }
        }
        Some(other) => {
            return Err(bad(format!(
                "unknown shape {other}; shapes: \"signal\", \"events\", {{\"vector\": N}}"
            )));
        }
    };
    let kind = match &port.kind {
        Some(kind) if KINDS.contains(&kind.as_str()) => crate::slot::kind_of(kind),
        Some(kind) => {
            return Err(bad(format!(
                "unknown kind `{kind}`; kinds: {}",
                KINDS.join(", ")
            )));
        }
        None => base
            .filter(|b| b.role == role)
            .map_or(signal_core::PortKind::Generic, |b| b.kind),
    };
    let rate = port
        .rate
        .or(base.filter(|b| b.role == role).map(|b| b.rate))
        .unwrap_or(1);
    if rate == 0 {
        return Err(bad("a rate divisor is at least 1"));
    }
    if let Some([min, max]) = port.range
        && !(min.is_finite() && max.is_finite() && min <= max)
    {
        return Err(bad("a range is [min, max], finite, min ≤ max"));
    }
    let keep = |given: &Option<String>, kept: Option<&Option<String>>| {
        given.clone().or_else(|| kept.cloned().flatten())
    };
    let base = base.filter(|b| b.role == role);
    Ok(InterfacePort {
        name: port.name.clone(),
        role,
        shape: if role == Role::Parameter {
            signal_core::PortShape::Signal
        } else {
            shape
        },
        rate: if role == Role::Parameter { 1 } else { rate },
        kind: if role == Role::Parameter {
            signal_core::PortKind::Generic
        } else {
            kind
        },
        unit: keep(&port.unit, base.map(|b| &b.unit)),
        range: if role == Role::Parameter {
            None
        } else {
            port.range
                .map(|[a, b]| (a, b))
                .or(base.and_then(|b| b.range))
        },
        group: keep(&port.group, base.map(|b| &b.group)),
    })
}

fn name_problem(name: &str) -> Option<String> {
    if !super::valid_name(name) {
        Some(format!("`{name}`: a port name is [a-z][a-z0-9_]*"))
    } else if name == "from" || name == "remove" {
        Some(format!(
            "`{name}` is reserved in a slot's interface and cannot name a port"
        ))
    } else {
        None
    }
}

/// A JSON value (a `slot` key or a `map`) as TOML text.
pub(crate) fn json_text(value: &serde_json::Value) -> Result<String, PatchError> {
    fn convert(value: &serde_json::Value) -> Result<Value, PatchError> {
        Ok(match value {
            serde_json::Value::String(s) => Value::from(s.as_str()),
            serde_json::Value::Bool(b) => Value::from(*b),
            serde_json::Value::Number(n) => match n.as_f64() {
                Some(f) if f.is_finite() => crate::write::number(f),
                _ => return Err(bad("numbers must be finite")),
            },
            serde_json::Value::Array(items) => {
                let mut array = toml_edit::Array::new();
                for item in items {
                    array.push(convert(item)?);
                }
                Value::Array(array)
            }
            serde_json::Value::Object(fields) => {
                let mut table = InlineTable::new();
                for (key, item) in fields {
                    table.insert(key, convert(item)?);
                }
                table.fmt();
                Value::InlineTable(table)
            }
            serde_json::Value::Null => return Err(bad("null is no TOML value")),
        })
    }
    Ok(convert(value)?.to_string().trim().to_owned())
}

/// What a slot op asks for, once read.
pub(super) enum SlotEdit<'o> {
    Load {
        load: &'o str,
        map: Option<&'o BTreeMap<String, serde_json::Value>>,
        set: Option<&'o BTreeMap<String, f64>>,
    },
    Unload,
    Map(&'o BTreeMap<String, serde_json::Value>),
    Interface {
        add: &'o [PortJson],
        remove: &'o [String],
        rename: &'o BTreeMap<String, String>,
        retype: &'o [PortJson],
        remove_wires: bool,
        detach: bool,
        template: Option<&'o str>,
    },
}

/// What a slot op did: the inverse changes (in the order to apply them),
/// the connection ops it ran (their inverses), the wires it renamed and the
/// notes for the reply.
pub(super) struct SlotDone {
    pub(super) changes: Vec<Change>,
    pub(super) wires: BTreeMap<String, String>,
    pub(super) before: Vec<super::Op>,
}

impl Patching<'_> {
    pub(super) fn ensure_slot(&self, slot: &str) -> Result<(), PatchError> {
        let m = map(&self.text, &Scheme::LEVEL).map_err(|e| refused("does-not-load", e))?;
        if !m.entries.iter().any(|e| e.name == slot) {
            return Err(refused(
                "unknown-instance",
                format!("no instance `{slot}` at this level"),
            ));
        }
        if !is_slot(&self.text, slot) {
            return Err(bad(format!(
                "`{slot}` is not a slot (it has no `slot` key)"
            )));
        }
        Ok(())
    }

    /// Applies `changes` to slot `slot`'s entry, and renames the wires
    /// `wires` (`port` → `new port`) at the level; returns the inverse
    /// changes, last first.
    pub(super) fn restore(
        &mut self,
        slot: &str,
        changes: &[Change],
        wires: &BTreeMap<String, String>,
    ) -> Result<(Vec<Change>, BTreeMap<String, String>), PatchError> {
        self.ensure_slot(slot)?;
        for change in changes {
            if change.path.is_empty() || !writable(&change.path[0]) {
                return Err(bad(format!(
                    "a slot op writes only the slot's own keys, not `{}`",
                    change.path.join(".")
                )));
            }
        }
        for (old, new) in wires {
            for name in [old, new] {
                if let Some(problem) = name_problem(name) {
                    return Err(bad(problem));
                }
            }
        }
        let mut doc: DocumentMut = self
            .text
            .parse()
            .map_err(|_| refused("does-not-load", "the file does not parse"))?;
        // What the edit may change outside the slot's entry: the wires and
        // exports it renames, and nothing else (checked at the end).
        let mut outside = doc.clone();
        for (old, new) in wires {
            let (from, to) = (format!("{slot}.{old}"), format!("{slot}.{new}"));
            for key in ["wiring", "exports"] {
                if let Some(item) = outside.get_mut(key) {
                    rewrite(item, &from, &to);
                }
            }
        }
        let outside = without_entry(outside, slot);
        let mut inverse = Vec::new();
        // A template name (`slot = "lib.voice"`) edited inside becomes its
        // table form first, so the undo gives the name back.
        let named = entry(&mut doc, slot)
            .and_then(|t| t.get("slot").and_then(TomlItem::as_str).map(str::to_owned));
        if let Some(from) = named
            && changes
                .iter()
                .any(|c| c.path.len() > 1 && c.path[0] == "slot")
        {
            let text = format!("{{ from = {} }}", json_text(&serde_json::json!(from))?);
            let change = Change {
                path: vec!["slot".to_owned()],
                value: Some(text),
                at: None,
                decor: None,
            };
            inverse.extend(apply_change(&mut doc, slot, &change)?);
        }
        for change in changes {
            inverse.extend(apply_change(&mut doc, slot, change)?);
        }
        // A role table, `map` or `set` left empty goes, but `slot = {}` stays.
        for key in ["map", "set"] {
            prune(&mut doc, slot, &[key], &mut inverse);
        }
        for role in ["inputs", "outputs", "parameters"] {
            prune(&mut doc, slot, &["slot", role], &mut inverse);
        }
        prune(&mut doc, slot, &["slot", "remove"], &mut inverse);
        let mut back = BTreeMap::new();
        for (old, new) in wires {
            let (from, to) = (format!("{slot}.{old}"), format!("{slot}.{new}"));
            for key in ["wiring", "exports"] {
                if let Some(item) = doc.get_mut(key) {
                    rewrite(item, &from, &to);
                }
            }
            back.insert(new.clone(), old.clone());
            self.renamed.push((from.clone(), to.clone()));
            self.follow.push((from, Some(to)));
        }
        inverse.reverse();
        let text = doc.to_string();
        let after: DocumentMut = text
            .parse()
            .map_err(|_| bad("the slot op leaves a file that does not parse"))?;
        if without_entry(after, slot) != outside {
            return Err(bad(
                "a slot op writes only the slot's own entry and the wires on its ports",
            ));
        }
        self.text = text;
        // The slot is still a slot, and the file still parses as a level.
        self.ensure_slot(slot)?;
        Ok((inverse, back))
    }

    /// The changes a slot op makes, applied; its inverse ops, in the order
    /// of the forward steps (the patch reverses the whole list).
    pub(super) fn slot_edit(
        &mut self,
        slot: &str,
        edit: SlotEdit<'_>,
    ) -> Result<Vec<super::Op>, PatchError> {
        self.ensure_slot(slot)?;
        let read = read_slot(&self.text, slot, self.ctx.templates)?;
        let done = self.plan_edit(slot, &read, edit)?;
        let mut inverses = done.before;
        let (changes, wires) = self.restore(slot, &done.changes, &done.wires)?;
        inverses.push(super::Op::SlotRestore {
            slot: slot.to_owned(),
            changes,
            wires,
        });
        Ok(inverses)
    }

    fn plan_edit(
        &mut self,
        slot: &str,
        read: &Read,
        edit: SlotEdit<'_>,
    ) -> Result<SlotDone, PatchError> {
        let mut changes = Vec::new();
        let mut wires = BTreeMap::new();
        let mut before = Vec::new();
        let set = |path: &[&str], value: Option<String>| Change {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            value,
            at: None,
            decor: None,
        };
        match edit {
            SlotEdit::Load {
                load,
                map,
                set: values,
            } => {
                let load = load.trim();
                if load.is_empty() || load.len() > 1024 || load.chars().any(char::is_control) {
                    return Err(bad(
                        "`load` is a module reference or a project path, 1 to 1024 characters",
                    ));
                }
                changes.push(set(&["load"], Some(json_text(&serde_json::json!(load))?)));
                let content = (self.ctx.content)(load);
                match map {
                    Some(map) => {
                        for (key, value) in map {
                            if !matches!(
                                value,
                                serde_json::Value::String(_) | serde_json::Value::Bool(false)
                            ) {
                                return Err(bad(format!("`map.{key}`: a content name or false")));
                            }
                        }
                        let text = (!map.is_empty())
                            .then(|| json_text(&serde_json::to_value(map).unwrap_or_default()))
                            .transpose()?;
                        changes.push(set(&["map"], text));
                    }
                    None => {
                        // Replace keeps the `map` entries the new content
                        // accepts and removes the rest, saying so (ADR 0037 §7).
                        if let Some(content) = &content {
                            for (key, value) in &read.map {
                                let shown = value
                                    .as_ref()
                                    .map_or(String::new(), |v| format!(" = \"{v}\""));
                                let why = if read.interface.port(key).is_none() {
                                    Some(format!("`{key}` is no port of the slot's interface"))
                                } else if value
                                    .as_ref()
                                    .is_some_and(|v| !content.ports.iter().any(|p| p.name == *v))
                                {
                                    Some(format!("`{load}` has no such port"))
                                } else {
                                    None
                                };
                                if let Some(why) = why {
                                    changes.push(set(&["map", key], None));
                                    self.notes.push(format!("removed map.{key}{shown}: {why}"));
                                }
                            }
                        }
                    }
                }
                match values {
                    Some(values) => {
                        let text = (!values.is_empty())
                            .then(|| json_text(&serde_json::to_value(values).unwrap_or_default()))
                            .transpose()?;
                        changes.push(set(&["set"], text));
                    }
                    None => {
                        if let Some(content) = &content {
                            for key in &read.set {
                                if !content.settable.contains(key) {
                                    changes.push(set(&["set", key], None));
                                    self.notes.push(format!(
                                        "removed set.{key}: `{load}` has no such parameter"
                                    ));
                                }
                            }
                        }
                    }
                }
                if read.seed && !crate::is_project_path(load) {
                    changes.push(set(&["seed"], None));
                    self.notes
                        .push(format!("removed seed: `{load}` is no project"));
                }
            }
            SlotEdit::Unload => {
                if read.load.is_none() {
                    return Err(bad(format!("`{slot}` is empty already")));
                }
                changes.push(set(&["load"], None));
                if read.seed {
                    changes.push(set(&["seed"], None));
                    self.notes
                        .push("removed seed: the slot is empty".to_owned());
                }
            }
            SlotEdit::Map(map) => {
                for (key, value) in map {
                    if read.interface.port(key).is_none() {
                        return Err(bad(format!(
                            "`map.{key}`: `{key}` is not a port or parameter of the slot's interface"
                        )));
                    }
                    if !matches!(
                        value,
                        serde_json::Value::String(_) | serde_json::Value::Bool(false)
                    ) {
                        return Err(bad(format!("`map.{key}`: a content name or false")));
                    }
                }
                let text = (!map.is_empty())
                    .then(|| json_text(&serde_json::to_value(map).unwrap_or_default()))
                    .transpose()?;
                changes.push(set(&["map"], text));
            }
            SlotEdit::Interface {
                add,
                remove,
                rename,
                retype,
                remove_wires,
                detach,
                template,
            } => {
                // ADR 0037, editable insides: the slot's own inside's exports
                // follow a removal or a rename (content name → new or none).
                let inside = self.own_inside(slot, read.load.as_deref());
                let mut synced: Vec<(String, Option<String>)> = Vec::new();
                let mut ports: Vec<InterfacePort> = read.interface.ports.clone();
                let mut own = read.own.clone();
                let mut removed = read.remove.clone();
                let template_has = |name: &str| {
                    read.template
                        .as_ref()
                        .is_some_and(|t| t.port(name).is_some())
                };
                let conns = map(&self.text, &Scheme::LEVEL)
                    .map_err(|e| refused("does-not-load", e))?
                    .conns;
                let wired = |name: &str| -> Vec<(String, String)> {
                    let end = format!("{slot}.{name}");
                    conns
                        .iter()
                        .filter(|c| c.from == end || c.to == end)
                        .map(|c| (c.from.clone(), c.to.clone()))
                        .collect()
                };
                // The level's `[exports]` naming a port: not removed with it.
                let exports_text = self.text.clone();
                let exports_on = |name: &str| exported(&exports_text, &format!("{slot}.{name}"));
                // A renamed port's wires still name it by its old name here.
                let mut was: BTreeMap<String, String> = BTreeMap::new();
                // Drops `name` from the interface: its own entry, and for a
                // template's port a `remove` entry.
                let forget = |name: &str,
                              own: &mut BTreeMap<String, Role>,
                              removed: &mut Vec<String>,
                              changes: &mut Vec<Change>| {
                    if let Some(role) = own.remove(name) {
                        changes.push(set(&["slot", role_key(role), name], None));
                    }
                    if template_has(name) && !removed.iter().any(|r| r == name) {
                        removed.push(name.to_owned());
                    }
                };
                let mut cut: Vec<(String, String)> = Vec::new();
                for name in remove {
                    if !ports.iter().any(|p| p.name == *name) {
                        return Err(bad(format!(
                            "`{name}` is not a port or parameter of `{slot}`"
                        )));
                    }
                    let named = exports_on(name);
                    if !named.is_empty() {
                        return Err(refused(
                            "in-use",
                            format!(
                                "`{slot}.{name}` is not removed: the level's [exports] name it ({}); edit them first",
                                named.join(", ")
                            ),
                        ));
                    }
                    let on = wired(name);
                    if !on.is_empty() && !remove_wires {
                        let named: Vec<String> =
                            on.iter().map(|(f, t)| format!("{f} → {t}")).collect();
                        return Err(refused(
                            "in-use",
                            format!(
                                "`{slot}.{name}` is not removed: wires name it ({}); remove them too with removeWires",
                                named.join(", ")
                            ),
                        ));
                    }
                    for (from, to) in on {
                        if cut.contains(&(from.clone(), to.clone())) {
                            continue;
                        }
                        cut.push((from.clone(), to.clone()));
                        before.extend(self.op(&super::Op::ConnectionRemove {
                            from,
                            to,
                            edits: None,
                        })?);
                    }
                    forget(name, &mut own, &mut removed, &mut changes);
                    if inside.is_some() {
                        let content = read
                            .map
                            .iter()
                            .find(|(k, _)| k == name)
                            .map_or_else(|| Some(name.clone()), |(_, v)| v.clone());
                        if let Some(content) = content {
                            synced.push((content, None));
                        }
                    }
                    if read.map.iter().any(|(k, _)| k == name) {
                        changes.push(set(&["map", name], None));
                    }
                    if read.values.iter().any(|k| k == name) {
                        changes.push(set(&[name], None));
                    }
                    ports.retain(|p| p.name != *name);
                    self.follow.push((format!("{slot}.{name}"), None));
                }
                // One op renames each port once: a new name that another
                // rename of the op takes from or gives away would chain them.
                for new in rename.values() {
                    if rename.contains_key(new) {
                        return Err(bad(format!(
                            "`{new}` is renamed and renamed to in one op: chain renames as separate ops"
                        )));
                    }
                }
                // Removals come before renames (a rename may take a removed name).
                let mut exclude = read.exclude.clone();
                exclude.retain(|n| !remove.contains(n));
                for (old, new) in rename {
                    let Some(port) = ports.iter().find(|p| p.name == *old).cloned() else {
                        return Err(bad(format!(
                            "`{old}` is not a port or parameter of `{slot}`"
                        )));
                    };
                    if let Some(problem) = name_problem(new) {
                        return Err(bad(problem));
                    }
                    if ports.iter().any(|p| p.name == *new) {
                        return Err(bad(format!("`{slot}` already has `{new}`")));
                    }
                    // The new name takes the old one's place in each table.
                    let place = |path: &[&str]| key_index(&self.text, slot, path);
                    let entry_at = place(&["slot", role_key(port.role), old]);
                    forget(old, &mut own, &mut removed, &mut changes);
                    let renamed = InterfacePort {
                        name: new.clone(),
                        ..port.clone()
                    };
                    removed.retain(|r| r != new);
                    let mut declared = set(
                        &["slot", role_key(port.role), new],
                        Some(port_text(&renamed)),
                    );
                    declared.at = entry_at;
                    changes.push(declared);
                    own.insert(new.clone(), port.role);
                    if let Some((_, value)) = read.map.iter().find(|(k, _)| k == old) {
                        let map_at = place(&["map", old]);
                        changes.push(set(&["map", old], None));
                        // Renamed back to the content's own name: matched by
                        // name again, no entry needed.
                        if value.as_deref() != Some(new.as_str()) {
                            let text = match value {
                                Some(v) => json_text(&serde_json::json!(v))?,
                                None => "false".to_owned(),
                            };
                            let mut moved = set(&["map", new], Some(text));
                            moved.at = map_at;
                            changes.push(moved);
                        }
                    } else if inside.is_some() {
                        // Its own inside: the export is renamed with it.
                        synced.push((old.clone(), Some(new.clone())));
                    } else if let Some(load) = &read.load
                        && (self.ctx.content)(load).is_some_and(|c| {
                            c.ports
                                .iter()
                                .any(|p| p.name == *old && p.role == port.role)
                        })
                    {
                        // Matched by name: the new name keeps taking the content's
                        // old one, so the plan does not change (ADR 0037 test 4).
                        changes.push(set(
                            &["map", new],
                            Some(json_text(&serde_json::json!(old))?),
                        ));
                        self.notes.push(format!(
                            "map.{new} = \"{old}\" written: `{load}` keeps its name"
                        ));
                    }
                    was.insert(new.clone(), old.clone());
                    for name in exclude.iter_mut().filter(|n| *n == old) {
                        name.clone_from(new);
                    }
                    if read.values.iter().any(|k| k == old) {
                        let mut doc: DocumentMut = self
                            .text
                            .parse()
                            .map_err(|_| refused("does-not-load", "the file does not parse"))?;
                        let value = entry(&mut doc, slot).and_then(|t| t.read(old).0);
                        let value_at = place(&[old]);
                        changes.push(set(&[old], None));
                        let mut moved = set(&[new], value);
                        moved.at = value_at;
                        changes.push(moved);
                    }
                    wires.insert(old.clone(), new.clone());
                    ports.retain(|p| p.name != *old);
                    ports.push(renamed);
                }
                for port in add {
                    if ports.iter().any(|p| p.name == port.name) {
                        return Err(bad(format!("`{slot}` already has `{}`", port.name)));
                    }
                    let declared = interface_port(port, None)?;
                    removed.retain(|r| *r != port.name);
                    changes.push(set(
                        &["slot", role_key(declared.role), &declared.name],
                        Some(port_text(&declared)),
                    ));
                    own.insert(declared.name.clone(), declared.role);
                    ports.push(declared);
                }
                for port in retype {
                    let Some(existing) = ports.iter().find(|p| p.name == port.name).cloned() else {
                        return Err(bad(format!(
                            "`{}` is not a port or parameter of `{slot}`",
                            port.name
                        )));
                    };
                    let declared = interface_port(port, Some(&existing))?;
                    if let Some(role) = own.get(&declared.name).copied()
                        && role != declared.role
                    {
                        changes.push(set(&["slot", role_key(role), &declared.name], None));
                    }
                    let original = was.get(&declared.name).unwrap_or(&declared.name);
                    if declared.role != existing.role && !wired(original).is_empty() {
                        return Err(refused(
                            "in-use",
                            format!(
                                "`{slot}.{}` keeps its direction while wires name it; remove them first",
                                declared.name
                            ),
                        ));
                    }
                    changes.push(set(
                        &["slot", role_key(declared.role), &declared.name],
                        Some(port_text(&declared)),
                    ));
                    own.insert(declared.name.clone(), declared.role);
                    ports.retain(|p| p.name != declared.name);
                    ports.push(declared);
                }
                if let Some(name) = &inside
                    && !synced.is_empty()
                {
                    before.extend(self.sync_inside(name, &synced)?);
                }
                if exclude != read.exclude {
                    let text = (!exclude.is_empty())
                        .then(|| json_text(&serde_json::json!(exclude)))
                        .transpose()?;
                    changes.push(set(&["snapshot_exclude"], text));
                }
                if removed != read.remove {
                    let text = (!removed.is_empty())
                        .then(|| json_text(&serde_json::json!(removed)))
                        .transpose()?;
                    changes.push(set(&["slot", "remove"], text));
                }
                if detach || template.is_some() {
                    if !(add.is_empty()
                        && remove.is_empty()
                        && rename.is_empty()
                        && retype.is_empty())
                    {
                        return Err(bad(
                            "`detach` and `template` stand alone in a slot.interface op",
                        ));
                    }
                    let text = match template {
                        Some(name) => {
                            let known = if name.starts_with(crate::slot::LIBRARY_TEMPLATE) {
                                crate::slot::library_template(name).is_some()
                            } else {
                                self.ctx.templates.contains_key(name)
                            };
                            if !known {
                                return Err(bad(format!("no interface `{name}`")));
                            }
                            json_text(&serde_json::json!(name))?
                        }
                        None => {
                            if read.from.is_none() {
                                return Err(bad(format!(
                                    "`{slot}` has no template to detach from"
                                )));
                            }
                            interface_text(&read.interface)
                        }
                    };
                    changes = vec![set(&["slot"], Some(text))];
                }
                if changes.is_empty() && before.is_empty() {
                    return Err(bad("the slot.interface op changes nothing"));
                }
            }
        }
        Ok(SlotDone {
            changes,
            wires,
            before,
        })
    }
}

/// The place of the key at `path` of slot `slot`'s entry in `text` among
/// its table's keys, if it is there.
fn key_index(text: &str, slot: &str, path: &[&str]) -> Option<usize> {
    let mut doc: DocumentMut = text.parse().ok()?;
    let (last, parents) = path.split_last()?;
    let mut tab = entry(&mut doc, slot)?;
    for key in parents {
        tab = tab.into_child(key, false)?;
    }
    tab.keys().iter().position(|k| k == last)
}

/// Every string of `text`'s `[exports]` equal to `end` (`slot.port`), by
/// the export's key path.
fn exported(text: &str, end: &str) -> Vec<String> {
    fn walk(value: &toml::Value, path: &str, end: &str, found: &mut Vec<String>) {
        match value {
            toml::Value::String(s) if s == end => found.push(path.to_owned()),
            toml::Value::Array(items) => items.iter().for_each(|v| walk(v, path, end, found)),
            toml::Value::Table(t) => {
                for (k, v) in t {
                    walk(v, &format!("{path}.{k}"), end, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    if let Ok(table) = toml::from_str::<toml::Table>(text)
        && let Some(exports) = table.get("exports")
    {
        walk(exports, "exports", end, &mut found);
    }
    found
}

/// The interface as a slot's inline `slot` value, every port its own entry.
pub(crate) fn interface_text(interface: &Interface) -> String {
    let mut parts = Vec::new();
    for role in [Role::Input, Role::Output, Role::Parameter] {
        let entries: Vec<String> = interface
            .ports
            .iter()
            .filter(|p| p.role == role)
            .map(|p| format!("{} = {}", p.name, port_text(p)))
            .collect();
        if !entries.is_empty() {
            parts.push(format!("{} = {{ {} }}", role_key(role), entries.join(", ")));
        }
    }
    if parts.is_empty() {
        "{}".to_owned()
    } else {
        format!("{{ {} }}", parts.join(", "))
    }
}

/// Removes the table at `path` of `slot`'s entry when it is empty,
/// recording the inverse.
fn prune(doc: &mut DocumentMut, slot: &str, path: &[&str], inverse: &mut Vec<Change>) {
    if let Some(tab) = entry(doc, slot) {
        prune_in(tab, path, inverse);
    }
}

/// Removes the table at `path` of `tab` when it is empty, recording the
/// inverse (its path relative to `tab`).
pub(super) fn prune_in(mut tab: Tab<'_>, path: &[&str], inverse: &mut Vec<Change>) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    for key in parents {
        match tab.into_child(key, false) {
            Some(child) => tab = child,
            None => return,
        }
    }
    let empty = match tab.get(last) {
        Some(TomlItem::Value(Value::InlineTable(t))) => t.is_empty(),
        Some(TomlItem::Value(Value::Array(a))) => a.is_empty(),
        Some(TomlItem::Table(t)) => t.is_empty(),
        _ => false,
    };
    if empty {
        let (old, at) = tab.read(last);
        let decor = tab.decor(last);
        tab.write(last, None, None);
        inverse.push(Change {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            value: old,
            at,
            decor,
        });
    }
}

/// Every string in `item` equal to `from` (a wire's end, an export's
/// target) becomes `to`, keeping its spacing.
fn rewrite(item: &mut TomlItem, from: &str, to: &str) {
    fn value(v: &mut Value, from: &str, to: &str) {
        match v {
            Value::String(s) if s.value() == from => {
                let decor = s.decor().clone();
                let mut new = toml_edit::Formatted::new(to.to_owned());
                *new.decor_mut() = decor;
                *s = new;
            }
            Value::Array(a) => {
                for v in a.iter_mut() {
                    value(v, from, to);
                }
            }
            Value::InlineTable(t) => {
                for (_, v) in t.iter_mut() {
                    value(v, from, to);
                }
            }
            _ => {}
        }
    }
    match item {
        TomlItem::Value(v) => value(v, from, to),
        TomlItem::Table(t) => {
            for (_, item) in t.iter_mut() {
                rewrite(item, from, to);
            }
        }
        TomlItem::ArrayOfTables(a) => {
            for t in a.iter_mut() {
                for (_, item) in t.iter_mut() {
                    rewrite(item, from, to);
                }
            }
        }
        TomlItem::None => {}
    }
}

/// The `slot.add` entry: `name = { slot = …, load = "…" }`.
pub(super) fn slot_entry(
    name: &str,
    slot: &serde_json::Value,
    load: Option<&str>,
) -> Result<String, PatchError> {
    let slot_text = match slot {
        serde_json::Value::Null => "{}".to_owned(),
        serde_json::Value::String(_) | serde_json::Value::Object(_) => json_text(slot)?,
        _ => return Err(bad("`slot` is a template name or an interface table")),
    };
    let mut text = format!("{name} = {{ slot = {slot_text}");
    if let Some(load) = load {
        let load = load.trim();
        if load.is_empty() || load.len() > 1024 || load.chars().any(char::is_control) {
            return Err(bad(
                "`load` is a module reference or a project path, 1 to 1024 characters",
            ));
        }
        text.push_str(&format!(
            ", load = {}",
            json_text(&serde_json::json!(load))?
        ));
    }
    text.push_str(" }");
    // It must read as one entry.
    let doc: DocumentMut = format!("[instances]\n{text}\n")
        .parse()
        .map_err(|_| written(format!("the slot entry does not render: {text}")))?;
    let _ = doc;
    Ok(text)
}
