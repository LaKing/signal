//! ADR 0051 §5: Make instrument (`slot.make`), ADR 0046's Group into slot
//! extended to several instances.
//!
//! At the top level, the named instances move into a new slot's own inside
//! (`composites/<slot>_inside.toml`, `inside_of = "<slot>"`), with their
//! entries as written and the connections among them. Every connection
//! that crosses the boundary goes through a port of the slot's interface
//! (auto, `{}`) and an export of the inside of the same name: into the
//! slot, one per inner target (`<instance>_<port>`, a parameter's as a
//! parameter export); out of it, one per inner source, wired to each of its
//! outside targets. The slot is written where the first moved instance was,
//! with `seed = "run"` (ADR 0051 §5), so every moved instance keeps its
//! random stream at its new address (`/kick/noise` → `/rhythm/kick/noise`).
//! Addresses and the log change; the audio does not, since every input has
//! one connection (ADR 0054) and nothing sums in address order.
//!
//! Its inverse is the level's text and the inside's file, byte for byte
//! (`level.restore`, `composite.file`). The panels' side (the moved
//! instances' controls into the slot's part) is the host's.

use std::collections::{BTreeMap, BTreeSet};

use toml_edit::{Array, DocumentMut, InlineTable, Item as TomlItem, Value};

use super::{Op, PatchError, Patching, bad, refused, valid_name};
use crate::slot::Role;

/// A connection of the level as written.
struct Wire {
    from: String,
    to: String,
    scale: Option<Value>,
    /// Its inline table as written (kept for a connection that stays).
    item: Value,
}

fn split(endpoint: &str) -> Option<(&str, &str)> {
    endpoint.split_once('.')
}

impl Patching<'_> {
    /// `slot.make`: instances `instances` of the top level moved into slot
    /// `slot`'s new own inside (the module docs).
    pub(super) fn slot_make(
        &mut self,
        slot: &str,
        instances: &[String],
    ) -> Result<Vec<Op>, PatchError> {
        if !self.ctx.level.is_empty() {
            return Err(bad(
                "Make instrument is a top-level edit (an instrument is a top-level slot)",
            ));
        }
        if !valid_name(slot) {
            return Err(bad(format!("`{slot}`: a slot's name is [a-z][a-z0-9_]*")));
        }
        let m = self.level_map()?;
        if m.entries.iter().any(|e| e.name == slot) && !instances.iter().any(|i| i == slot) {
            return Err(bad(format!("`{slot}` is taken at this level")));
        }
        let moved: BTreeSet<&str> = instances.iter().map(String::as_str).collect();
        if moved.is_empty() || moved.len() != instances.len() {
            return Err(bad("name each instance once, at least one"));
        }
        let mut doc: DocumentMut = self
            .text
            .parse()
            .map_err(|_| refused("does-not-load", "the file does not parse"))?;
        let table = doc
            .get("instances")
            .and_then(TomlItem::as_table_like)
            .ok_or_else(|| bad("the level has no [instances]"))?;
        // Each moved entry, as written, and the module whose ports it has.
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut roles: BTreeMap<(String, String), Role> = BTreeMap::new();
        for name in instances {
            let item = table.get(name).ok_or_else(|| {
                refused(
                    "unknown-instance",
                    format!("no instance `{name}` at this level"),
                )
            })?;
            let value = item
                .as_value()
                .and_then(Value::as_inline_table)
                .ok_or_else(|| {
                    bad(format!(
                        "`{name}` is not written as an inline table: move it by hand"
                    ))
                })?;
            // A pattern names the project's file from a composite as from
            // the top (it moves along); a GPU pin stays at the top.
            for key in ["slot", "project", "device", "gpu"] {
                if value.get(key).is_some() {
                    return Err(bad(format!(
                        "`{name}` has `{key}`: Make instrument moves module instances only"
                    )));
                }
            }
            let module = value
                .get("module")
                .and_then(Value::as_str)
                .ok_or_else(|| bad(format!("`{name}` has no `module`")))?;
            let names = (self.ctx.content)(module)
                .ok_or_else(|| refused("unknown-module", format!("`{module}` does not resolve")))?;
            for port in names.ports {
                roles.insert((name.clone(), port.name), port.role);
            }
            // Its comments go with it: those above its key, and its own.
            let above = table
                .key(name)
                .and_then(|k| k.leaf_decor().prefix())
                .and_then(|p| p.as_str())
                .map(str::trim)
                .unwrap_or("")
                .to_owned();
            let after = value
                .decor()
                .suffix()
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .trim_end()
                .to_owned();
            let mut text = value.clone();
            text.decor_mut().clear();
            let mut line = String::new();
            if !above.is_empty() && entries.is_empty() {
                // The first one's stay above the slot it becomes.
            } else if !above.is_empty() {
                line.push_str(&above);
                line.push('\n');
            }
            line.push_str(&format!(
                "{} = {}{after}",
                key(name),
                text.to_string().trim()
            ));
            entries.push((name.clone(), line));
        }
        if let Some(exports) = doc.get("exports").and_then(TomlItem::as_table_like) {
            let named = exports.iter().any(|(_, v)| {
                v.to_string()
                    .split('"')
                    .any(|s| split(s).is_some_and(|(i, _)| moved.contains(i)))
            });
            if named {
                return Err(bad(
                    "an export of the level names a moved instance: move it by hand",
                ));
            }
        }
        let wires = read_wires(&doc)?;

        // The connections: inside, kept, or across the boundary.
        let mut inner: Vec<&Wire> = Vec::new();
        let mut kept: Vec<Value> = Vec::new();
        let mut taken: BTreeSet<String> = BTreeSet::new();
        let name_for = |instance: &str, port: &str, taken: &mut BTreeSet<String>| {
            let base = format!("{instance}_{port}");
            let name = std::iter::once(base.clone())
                .chain((2..).map(|n| format!("{base}_{n}")))
                .find(|n| !taken.contains(n) && n != "voices" && n != "alloc")
                .unwrap_or(base);
            taken.insert(name.clone());
            name
        };
        let mut inputs: Vec<(String, String)> = Vec::new();
        let mut parameters: Vec<(String, String)> = Vec::new();
        let mut outputs: BTreeMap<String, String> = BTreeMap::new();
        // Each inner input or parameter given a port, by its endpoint.
        let mut into: BTreeMap<String, String> = BTreeMap::new();
        let mut host: Vec<(String, String, Option<Value>)> = Vec::new();
        for wire in &wires {
            let (from_i, from_p) =
                split(&wire.from).ok_or_else(|| bad(format!("`{}`: not inst.port", wire.from)))?;
            let (to_i, to_p) =
                split(&wire.to).ok_or_else(|| bad(format!("`{}`: not inst.port", wire.to)))?;
            match (moved.contains(from_i), moved.contains(to_i)) {
                (true, true) => inner.push(wire),
                (false, false) => kept.push(wire.item.clone()),
                (false, true) => {
                    let port = name_for(to_i, to_p, &mut taken);
                    let parameter =
                        roles.get(&(to_i.to_owned(), to_p.to_owned())) == Some(&Role::Parameter);
                    if parameter {
                        parameters.push((port.clone(), wire.to.clone()));
                    } else {
                        inputs.push((port.clone(), wire.to.clone()));
                    }
                    into.insert(wire.to.clone(), port.clone());
                    host.push((
                        wire.from.clone(),
                        format!("{slot}.{port}"),
                        wire.scale.clone(),
                    ));
                }
                (true, false) => {
                    let port = match outputs.get(&wire.from) {
                        Some(port) => port.clone(),
                        None => {
                            let port = name_for(from_i, from_p, &mut taken);
                            outputs.insert(wire.from.clone(), port.clone());
                            port
                        }
                    };
                    host.push((
                        format!("{slot}.{port}"),
                        wire.to.clone(),
                        wire.scale.clone(),
                    ));
                }
            }
        }

        // What the panels bind on a moved instance becomes a port of the
        // slot too, so its controls bind the slot (a parameter inside a
        // composite is no control); not an input or parameter an inner
        // connection drives (an export onto it would be a second source).
        let driven: BTreeSet<&str> = inner.iter().map(|w| w.to.as_str()).collect();
        for instance in instances {
            for name in (self.ctx.bound)(instance) {
                let endpoint = format!("{instance}.{name}");
                match roles.get(&(instance.clone(), name.clone())) {
                    Some(Role::Output) => {
                        if let std::collections::btree_map::Entry::Vacant(slot) =
                            outputs.entry(endpoint)
                        {
                            slot.insert(name_for(instance, &name, &mut taken));
                        }
                    }
                    Some(role)
                        if !into.contains_key(&endpoint) && !driven.contains(endpoint.as_str()) =>
                    {
                        let port = name_for(instance, &name, &mut taken);
                        if *role == Role::Parameter {
                            parameters.push((port.clone(), endpoint.clone()));
                        } else {
                            inputs.push((port.clone(), endpoint.clone()));
                        }
                        into.insert(endpoint, port);
                    }
                    _ => {}
                }
            }
        }
        self.made.extend(
            into.iter()
                .chain(outputs.iter())
                .map(|(endpoint, port)| (endpoint.clone(), format!("{slot}.{port}"))),
        );

        // The inside.
        let exists = |n: &str| {
            self.file_now(&format!("{}/{n}.toml", crate::COMPOSITES))
                .is_some()
                || crate::Library::builtin().get(n).is_some()
        };
        let name = crate::inside::default_name("", slot, &exists);
        let owner = crate::inside::owner("", slot);
        let mut text = format!(
            "schema_version = 1\n\n[composite]\nname = {}\ninside_of = {}\n\n[instances]\n",
            Value::from(name.as_str()),
            Value::from(owner.as_str())
        );
        for (_, line) in &entries {
            text.push_str(line);
            text.push('\n');
        }
        if !inner.is_empty() {
            text.push_str("\n[wiring]\nconnections = [\n");
            for wire in &inner {
                let mut item = wire.item.clone();
                item.decor_mut().clear();
                text.push_str(&format!("  {item},\n"));
            }
            text.push_str("]\n");
        }
        let mut outputs: Vec<(String, String)> = outputs
            .into_iter()
            .map(|(from, port)| (port, from))
            .collect();
        outputs.sort();
        let sections = [
            ("inputs", &inputs),
            ("outputs", &outputs),
            ("parameters", &parameters),
        ];
        if sections.iter().any(|(_, list)| !list.is_empty()) {
            text.push_str("\n[exports]\n");
            for (section, list) in sections {
                if list.is_empty() {
                    continue;
                }
                let items: Vec<String> = list
                    .iter()
                    .map(|(port, at)| format!("{} = {}", key(port), Value::from(at.as_str())))
                    .collect();
                text.push_str(&format!("{section} = {{ {} }}\n", items.join(", ")));
            }
        }

        // The level: the slot in the first moved entry's place, the moved
        // entries gone, the connections rewritten.
        let interface = {
            let part = |list: &[(String, String)]| {
                list.iter()
                    .map(|(port, _)| format!("{} = {{}}", key(port)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let mut tables = Vec::new();
            for (section, list) in [
                ("inputs", &inputs),
                ("outputs", &outputs),
                ("parameters", &parameters),
            ] {
                if !list.is_empty() {
                    tables.push(format!("{section} = {{ {} }}", part(list)));
                }
            }
            if tables.is_empty() {
                "{}".to_owned()
            } else {
                format!("{{ {} }}", tables.join(", "))
            }
        };
        let entry: Value = format!(
            "{{ slot = {interface}, load = {}, seed = \"run\" }}",
            Value::from(name.as_str())
        )
        .parse()
        .map_err(|e| bad(format!("the slot's entry: {e}")))?;
        let table = doc
            .get_mut("instances")
            .and_then(TomlItem::as_table_like_mut)
            .ok_or_else(|| bad("the level has no [instances]"))?;
        let order: Vec<String> = table.iter().map(|(k, _)| k.to_owned()).collect();
        let mut rebuilt: Vec<(String, TomlItem)> = Vec::new();
        let mut placed = false;
        let mut first: Option<String> = None;
        for k in order {
            let Some(item) = table.get(&k).cloned() else {
                continue;
            };
            if moved.contains(k.as_str()) {
                if !placed {
                    let mut value = entry.clone();
                    value.decor_mut().set_prefix(" ");
                    rebuilt.push((slot.to_owned(), TomlItem::Value(value)));
                    first = Some(k.clone());
                    placed = true;
                }
                continue;
            }
            rebuilt.push((k, item));
        }
        let keys: Vec<String> = table.iter().map(|(k, _)| k.to_owned()).collect();
        let mut decors: BTreeMap<String, toml_edit::Decor> = keys
            .iter()
            .filter_map(|k| {
                table
                    .key(k)
                    .map(|key| (k.clone(), key.leaf_decor().clone()))
            })
            .collect();
        // The slot's key takes the first moved instance's place, and the
        // comments above it.
        if let Some(decor) = first.as_ref().and_then(|f| decors.get(f).cloned()) {
            decors.insert(slot.to_owned(), decor);
        }
        for k in &keys {
            table.remove(k);
        }
        for (k, item) in rebuilt {
            table.insert(&k, item);
            if let (Some(decor), Some(mut key)) = (decors.get(&k), table.key_mut(&k)) {
                *key.leaf_decor_mut() = decor.clone();
            }
        }
        write_wires(&mut doc, kept, &host)?;

        let before = self.text.clone();
        let after = doc.to_string();
        let inverse_file = self.write_inside(&name, Some(text))?;
        self.text = after.clone();
        self.notes.push(format!(
            "{slot}: an instrument of {} (composites/{name}.toml, seed = \"run\")",
            instances.join(", ")
        ));
        Ok(vec![
            Op::LevelRestore {
                text: before,
                was: after,
            },
            inverse_file,
        ])
    }

    /// `level.restore` (untrusted): the level's text set to `text` while it
    /// is `was`, byte for byte: the inverse of `slot.make`.
    pub(super) fn level_restore(&mut self, text: &str, was: &str) -> Result<Op, PatchError> {
        if self.text != was {
            return Err(bad("level.restore: the level is not the text the op names"));
        }
        let now = std::mem::replace(&mut self.text, text.to_owned());
        Ok(Op::LevelRestore {
            text: now,
            was: text.to_owned(),
        })
    }
}

/// The level's `[wiring] connections`, in order.
fn read_wires(doc: &DocumentMut) -> Result<Vec<Wire>, PatchError> {
    let Some(array) = doc
        .get("wiring")
        .and_then(|w| w.get("connections"))
        .and_then(TomlItem::as_array)
    else {
        return Ok(Vec::new());
    };
    array
        .iter()
        .map(|value| {
            let table = value
                .as_inline_table()
                .ok_or_else(|| bad("a connection is not an inline table: move it by hand"))?;
            let text = |k: &str| table.get(k).and_then(Value::as_str).map(str::to_owned);
            Ok(Wire {
                from: text("from").ok_or_else(|| bad("a connection without `from`"))?,
                to: text("to").ok_or_else(|| bad("a connection without `to`"))?,
                scale: table.get("scale").cloned(),
                item: value.clone(),
            })
        })
        .collect()
}

/// The level's connections set to `kept`, then `host`'s new ones.
fn write_wires(
    doc: &mut DocumentMut,
    kept: Vec<Value>,
    host: &[(String, String, Option<Value>)],
) -> Result<(), PatchError> {
    let Some(array) = doc
        .get_mut("wiring")
        .and_then(|w| w.get_mut("connections"))
        .and_then(TomlItem::as_array_mut)
    else {
        if host.is_empty() {
            return Ok(());
        }
        return Err(bad("the level has no [wiring] connections to rewrite"));
    };
    let trailing = array.trailing().clone();
    let comma = array.trailing_comma();
    let mut rebuilt = Array::new();
    for value in kept {
        rebuilt.push_formatted(value);
    }
    for (from, to, scale) in host {
        let mut table = InlineTable::new();
        table.insert("from", Value::from(from.as_str()));
        table.insert("to", Value::from(to.as_str()));
        if let Some(scale) = scale {
            let mut scale = scale.clone();
            scale.decor_mut().clear();
            table.insert("scale", scale);
        }
        let mut value = Value::InlineTable(table);
        value.decor_mut().set_prefix("\n  ");
        rebuilt.push_formatted(value);
    }
    rebuilt.set_trailing(trailing);
    rebuilt.set_trailing_comma(comma);
    *array = rebuilt;
    Ok(())
}

/// `name` as a TOML key.
fn key(name: &str) -> String {
    if valid_name(name) {
        name.to_owned()
    } else {
        Value::from(name).to_string()
    }
}
