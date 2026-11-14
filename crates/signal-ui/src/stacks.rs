//! ADR 0051 §10: a Stack's export and import. No new format: an exported
//! Stack is a project.
//!
//! - **Export:** the Stack's instruments as they are (each a top-level slot:
//!   a card instrument is made one first, Make Instrument), their own
//!   insides with the composites and patterns those use, their parts, the
//!   connections among them, and each port a connection joins to
//!   something outside the Stack as an export (`<slot>_<port>`, the edge
//!   pins); a `ui/main.html` of their slot cards and a one-Stack
//!   `ui/plane.toml`. Role endpoints stay behind. A slot linking a project
//!   keeps the link, written relative to the new folder.
//! - **Import:** the project's slots copied in as a new Stack: a name the
//!   host has gets `_2`, `_3`, …, its own inside renamed with it and every
//!   connection among them rewritten; another composite or pattern the
//!   host has with another text is copied as `<stack>_<name>`; the edge
//!   pins stay unconnected, to be cabled. The new Stack goes to the right
//!   of the others in `ui/plane.toml` (a plane that is not written is left
//!   derived). Written by `signal vendor`'s all-or-nothing writer.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use toml_edit::{DocumentMut, Item, Value};

/// A project's files by path, read from `dir` (`project.toml`,
/// `composites/`, `patterns/`, `ui/parts/`, `ui/plane.toml`).
fn read(dir: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut take = |path: String| {
        if let Ok(text) = signal_project::read_regular_to_string(dir.join(&path)) {
            files.insert(path, text);
        }
    };
    take("project.toml".to_owned());
    take(format!("ui/{}", crate::plane::PLANE_FILE));
    for folder in ["composites", "patterns", "ui/parts"] {
        let ext = if folder == "ui/parts" {
            ".html"
        } else {
            ".toml"
        };
        let mut names: Vec<String> = std::fs::read_dir(dir.join(folder))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(ext) && !n.starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            take(format!("{folder}/{name}"));
        }
    }
    files
}

fn parse(path: &str, text: &str) -> Result<DocumentMut, String> {
    text.parse()
        .map_err(|e| format!("{path} does not parse: {e}"))
}

/// The inline table of top-level instance `name`.
fn entry<'d>(doc: &'d DocumentMut, name: &str) -> Option<&'d toml_edit::InlineTable> {
    doc.get("instances")?
        .get(name)?
        .as_value()?
        .as_inline_table()
}

/// The `inside_of` of a composite file's text.
fn inside_of(text: &str) -> Option<String> {
    signal_project::inside::owner_of(text)
}

/// The project composites and patterns `texts` use, transitively, from
/// `files`.
fn uses(
    files: &BTreeMap<String, String>,
    texts: &[String],
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut composites = BTreeSet::new();
    let mut patterns = BTreeSet::new();
    let mut queue: Vec<String> = texts.to_vec();
    while let Some(text) = queue.pop() {
        let Ok(table) = text.parse::<toml::Table>() else {
            continue;
        };
        let Some(instances) = table.get("instances").and_then(toml::Value::as_table) else {
            continue;
        };
        for entry in instances.values() {
            if let Some(p) = entry.get("pattern").and_then(toml::Value::as_str) {
                patterns.insert(p.to_owned());
            }
            if let Some(m) = entry.get("module").and_then(toml::Value::as_str)
                && let Some(text) = files.get(&format!("composites/{m}.toml"))
                && composites.insert(m.to_owned())
            {
                queue.push(text.clone());
            }
        }
    }
    (composites, patterns)
}

/// The Stack `stack`'s instruments, from a plane file's text.
fn instruments(plane: &str, stack: &str) -> Option<(Vec<String>, i64)> {
    let table: toml::Table = plane.parse().ok()?;
    let found = table
        .get("stack")?
        .as_array()?
        .iter()
        .find(|s| s.get("name").and_then(toml::Value::as_str) == Some(stack))?;
    let list = found
        .get("instruments")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let width = found
        .get("width")
        .and_then(toml::Value::as_integer)
        .unwrap_or(160);
    Some((list, width))
}

/// Stack `stack` of the project in `dir` as a project of its own, to be
/// written in the folder `out`: its files by path.
pub fn export_stack(dir: &Path, stack: &str, out: &Path) -> Result<Vec<(String, String)>, String> {
    let files = read(dir);
    let plane = files
        .get(&format!("ui/{}", crate::plane::PLANE_FILE))
        .ok_or(
            "the project's plane is not written (ui/plane.toml): arrange it once on the Deck first",
        )?;
    let (list, width) =
        instruments(plane, stack).ok_or_else(|| format!("no Stack `{stack}` in ui/plane.toml"))?;
    if let Some(card) = list
        .iter()
        .find(|e| e.starts_with(crate::plane::CARD_PREFIX))
    {
        return Err(format!(
            "`{card}` is a card: make it an instrument first (Make Instrument), then save the Stack"
        ));
    }
    if list.is_empty() {
        return Err(format!("the Stack `{stack}` is empty"));
    }
    let host_text = files.get("project.toml").ok_or("no project.toml")?;
    let host = parse("project.toml", host_text)?;
    let name_of = |table: &toml::Table| -> (String, u32, u64) {
        let project = table.get("project");
        (
            project
                .and_then(|p| p.get("name"))
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_owned(),
            project
                .and_then(|p| p.get("sample_rate"))
                .and_then(toml::Value::as_integer)
                .and_then(|r| u32::try_from(r).ok())
                .unwrap_or(48_000),
            project
                .and_then(|p| p.get("seed"))
                .and_then(toml::Value::as_integer)
                .and_then(|s| u64::try_from(s).ok())
                .unwrap_or(0),
        )
    };
    let (host_name, rate, seed) = name_of(&host_text.parse::<toml::Table>().unwrap_or_default());
    let members: BTreeSet<&str> = list.iter().map(String::as_str).collect();
    let mut out_files: Vec<(String, String)> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut insides: Vec<String> = Vec::new();
    let canonical_out = out
        .parent()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.join(out.file_name().unwrap_or_default()));
    for slot in &list {
        let value = entry(&host, slot)
            .ok_or_else(|| format!("`{slot}` is not a top-level slot written inline"))?;
        if value.get("slot").is_none() {
            return Err(format!("`{slot}` is not a slot"));
        }
        let mut value = value.clone();
        value.decor_mut().clear();
        if let Some(load) = value.get("load").and_then(Value::as_str).map(str::to_owned) {
            if signal_project::is_project_path(&load) {
                // The link, written from the new folder.
                let target = std::fs::canonicalize(dir.join(&load))
                    .map_err(|e| format!("`{slot}` links `{load}`, which cannot be read: {e}"))?;
                let from = canonical_out
                    .clone()
                    .ok_or("the new folder's place cannot be read")?;
                let relative = relative_folder(&from, &target).ok_or_else(|| {
                    format!("`{slot}`'s link cannot be written from the new folder")
                })?;
                value.insert("load", Value::from(relative));
            } else if let Some(text) = files.get(&format!("composites/{load}.toml"))
                && inside_of(text).as_deref() == Some(slot.as_str())
            {
                insides.push(text.clone());
                out_files.push((format!("composites/{load}.toml"), text.clone()));
            }
        }
        lines.push(format!("{slot} = {}", value.to_string().trim()));
        if let Some(part) = files.get(&format!("ui/parts/{slot}.html")) {
            out_files.push((format!("ui/parts/{slot}.html"), part.clone()));
        }
    }
    let (composites, patterns) = uses(&files, &insides);
    for name in composites {
        let path = format!("composites/{name}.toml");
        if !out_files.iter().any(|(p, _)| *p == path)
            && let Some(text) = files.get(&path)
        {
            out_files.push((path, text.clone()));
        }
    }
    for name in patterns {
        let path = format!("patterns/{name}.toml");
        let text = files
            .get(&path)
            .ok_or_else(|| format!("{path} is missing"))?;
        out_files.push((path, text.clone()));
    }

    // The connections among them, and the edge pins.
    let mut wires: Vec<String> = Vec::new();
    let mut inputs: BTreeMap<String, String> = BTreeMap::new();
    let mut outputs: BTreeMap<String, String> = BTreeMap::new();
    if let Some(array) = host
        .get("wiring")
        .and_then(|w| w.get("connections"))
        .and_then(Item::as_array)
    {
        for value in array.iter() {
            let Some(table) = value.as_inline_table() else {
                continue;
            };
            let end = |k: &str| {
                table
                    .get(k)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            let (from, to) = (end("from"), end("to"));
            let inside = |e: &str| e.split_once('.').is_some_and(|(i, _)| members.contains(i));
            match (inside(&from), inside(&to)) {
                (true, true) => {
                    let mut item = value.clone();
                    item.decor_mut().clear();
                    wires.push(format!("  {item},"));
                }
                (false, true) => {
                    inputs.insert(to.replace('.', "_"), to.clone());
                }
                (true, false) => {
                    outputs.insert(from.replace('.', "_"), from.clone());
                }
                (false, false) => {}
            }
        }
    }
    let mut text = format!(
        "schema_version = 1\n\n# Saved from Stack `{stack}` of `{host_name}` (ADR 0051 §10).\n[project]\nname = {}\nsample_rate = {rate}\nseed = {seed}\n\n[instances]\n{}\n",
        Value::from(stack),
        lines.join("\n")
    );
    if !wires.is_empty() {
        text.push_str(&format!(
            "\n[wiring]\nconnections = [\n{}\n]\n",
            wires.join("\n")
        ));
    }
    if !inputs.is_empty() || !outputs.is_empty() {
        text.push_str("\n[exports]\n");
        for (role, list) in [("inputs", &inputs), ("outputs", &outputs)] {
            if list.is_empty() {
                continue;
            }
            let items: Vec<String> = list
                .iter()
                .map(|(name, at)| format!("{name} = {}", Value::from(at.as_str())))
                .collect();
            text.push_str(&format!("{role} = {{ {} }}\n", items.join(", ")));
        }
    }
    out_files.insert(0, ("project.toml".to_owned(), text));
    let cards: Vec<String> = list
        .iter()
        .map(|s| format!("    <signal-slot bind=\"/{s}\"></signal-slot>"))
        .collect();
    out_files.push((
        "ui/main.html".to_owned(),
        format!(
            "<!doctype html>\n<signal-panel>\n  <div class=\"signal-cards\">\n{}\n  </div>\n</signal-panel>\n",
            cards.join("\n")
        ),
    ));
    let listed: Vec<String> = list
        .iter()
        .map(|s| Value::from(s.as_str()).to_string())
        .collect();
    out_files.push((
        format!("ui/{}", crate::plane::PLANE_FILE),
        format!(
            "schema_version = 1\n\n[[stack]]\nname = {}\nat = [0, 0]\nwidth = {width}\ninstruments = [{}]\n",
            Value::from(stack),
            listed.join(", ")
        ),
    ));
    Ok(out_files)
}

/// What importing the Stack project in `source` into the project in `dir`
/// writes: new files, replaced files (by path), and the new Stack's name.
pub type Imported = (Vec<(String, String)>, Vec<(String, String)>, String);

/// The Stack project in `source` copied into the project in `dir` as a new
/// Stack (the module docs).
pub fn import_stack(dir: &Path, source: &Path) -> Result<Imported, String> {
    let host_files = read(dir);
    let from = read(source);
    let src_text = from
        .get("project.toml")
        .ok_or("the folder has no project.toml")?;
    let src = parse("project.toml", src_text)?;
    let src_table: toml::Table = src_text.parse().unwrap_or_default();
    let src_name = src_table
        .get("project")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .unwrap_or("stack")
        .to_owned();
    let slots: Vec<String> = match from.get(&format!("ui/{}", crate::plane::PLANE_FILE)) {
        Some(plane) => plane
            .parse::<toml::Table>()
            .ok()
            .and_then(|t| {
                t.get("stack")?
                    .as_array()?
                    .first()?
                    .get("instruments")?
                    .as_array()
                    .cloned()
            })
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        None => src_table
            .get("instances")
            .and_then(toml::Value::as_table)
            .map(|t| {
                t.iter()
                    .filter(|(_, v)| v.get("slot").is_some())
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default(),
    };
    if slots.is_empty() {
        return Err("the project has no slots to import as a Stack".to_owned());
    }
    let host_text = host_files.get("project.toml").ok_or("no project.toml")?;
    let mut host = parse("project.toml", host_text)?;
    let host_names: BTreeSet<String> = host
        .get("instances")
        .and_then(Item::as_table_like)
        .map(|t| t.iter().map(|(k, _)| k.to_owned()).collect())
        .unwrap_or_default();
    let mut taken = host_names.clone();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    for slot in &slots {
        let new = std::iter::once(slot.clone())
            .chain((2..).map(|n| format!("{slot}_{n}")))
            .find(|n| !taken.contains(n))
            .unwrap_or_else(|| slot.clone());
        taken.insert(new.clone());
        renamed.insert(slot.clone(), new);
    }
    let plane_path = format!("ui/{}", crate::plane::PLANE_FILE);
    let host_plane = host_files.get(&plane_path);
    let stack_names: BTreeSet<String> = host_plane
        .and_then(|p| p.parse::<toml::Table>().ok())
        .and_then(|t| t.get("stack").and_then(toml::Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            s.get("name")
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let stack = std::iter::once(src_name.clone())
        .chain((2..).map(|n| format!("{src_name}_{n}")))
        .find(|n| !stack_names.contains(n))
        .unwrap_or(src_name);

    let has = |path: &str| host_files.contains_key(path) || dir.join(path).exists();
    let mut copies: Vec<(String, String)> = Vec::new();
    // Other composites and patterns: kept, reused, or copied as `<stack>_<name>`.
    let mut modules: BTreeMap<String, String> = BTreeMap::new();
    let mut pattern_names: BTreeMap<String, String> = BTreeMap::new();
    let insides: BTreeMap<String, String> = slots
        .iter()
        .filter_map(|slot| {
            let load = entry(&src, slot)?.get("load")?.as_str()?.to_owned();
            let text = from.get(&format!("composites/{load}.toml"))?;
            (inside_of(text).as_deref() == Some(slot.as_str())).then(|| (load, slot.clone()))
        })
        .collect();
    for (path, text) in &from {
        for (prefix, names) in [
            ("composites/", &mut modules),
            ("patterns/", &mut pattern_names),
        ] {
            let Some(name) = path
                .strip_prefix(prefix)
                .and_then(|n| n.strip_suffix(".toml"))
            else {
                continue;
            };
            if prefix == "composites/" && insides.contains_key(name) {
                continue;
            }
            let new = match host_files.get(path) {
                Some(existing) if existing == text => name.to_owned(),
                Some(_) => format!("{stack}_{name}"),
                None => name.to_owned(),
            };
            names.insert(name.to_owned(), new);
        }
    }
    let mut inside_names: BTreeMap<String, String> = BTreeMap::new();
    for (load, slot) in &insides {
        let new_slot = lookup(&renamed, slot)?;
        let name = signal_project::inside::default_name("", new_slot, &|n| {
            has(&format!("composites/{n}.toml")) || inside_names.values().any(|v| v == n)
        });
        inside_names.insert(load.clone(), name);
    }
    let retarget = |doc: &mut DocumentMut| {
        if let Some(instances) = doc.get_mut("instances").and_then(Item::as_table_like_mut) {
            for (_, e) in instances.iter_mut() {
                let Some(e) = e.as_table_like_mut() else {
                    continue;
                };
                for (key, names) in [("module", &modules), ("pattern", &pattern_names)] {
                    if let Some(item) = e.get_mut(key)
                        && let Some(new) = item.as_str().and_then(|v| names.get(v)).cloned()
                        && let Some(value) = item.as_value_mut()
                    {
                        let decor = value.decor().clone();
                        *value = Value::from(new);
                        *value.decor_mut() = decor;
                    }
                }
            }
        }
    };
    for (path, text) in &from {
        for (prefix, names) in [("composites/", &modules), ("patterns/", &pattern_names)] {
            let Some(name) = path
                .strip_prefix(prefix)
                .and_then(|n| n.strip_suffix(".toml"))
            else {
                continue;
            };
            let Some(new) = names.get(name) else { continue };
            let target = format!("{prefix}{new}.toml");
            if host_files.get(&target) == Some(text) {
                continue;
            }
            let mut doc = parse(path, text)?;
            if prefix == "composites/" {
                if let Some(item) = doc.get_mut("composite").and_then(|c| c.get_mut("name"))
                    && let Some(value) = item.as_value_mut()
                {
                    *value = Value::from(new.as_str());
                }
                retarget(&mut doc);
            }
            copies.push((target, doc.to_string()));
        }
    }
    for (load, new) in &inside_names {
        let text = lookup(&from, &format!("composites/{load}.toml"))?;
        let mut doc = parse(load, text)?;
        if let Some(composite) = doc.get_mut("composite").and_then(Item::as_table_like_mut) {
            composite.insert("name", toml_edit::value(new.as_str()));
            composite.insert(
                "inside_of",
                toml_edit::value(lookup(&renamed, lookup(&insides, load)?)?.as_str()),
            );
        }
        retarget(&mut doc);
        copies.push((format!("composites/{new}.toml"), doc.to_string()));
    }

    // The slots, their parts and the connections among them.
    let canonical_dir = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
    let instances = host
        .get_mut("instances")
        .and_then(Item::as_table_like_mut)
        .ok_or("the project has no [instances]")?;
    for slot in &slots {
        let new = lookup(&renamed, slot)?;
        let mut value = entry(&src, slot)
            .ok_or_else(|| format!("`{slot}` is not written inline in the Stack's project"))?
            .clone();
        value.decor_mut().clear();
        if let Some(load) = value.get("load").and_then(Value::as_str).map(str::to_owned) {
            if let Some(inside) = inside_names.get(&load) {
                value.insert("load", Value::from(inside.as_str()));
            } else if signal_project::is_project_path(&load) {
                let target = std::fs::canonicalize(source.join(&load))
                    .map_err(|e| format!("`{slot}` links `{load}`, which cannot be read: {e}"))?;
                let relative = relative_folder(&canonical_dir, &target).ok_or_else(|| {
                    format!("`{slot}`'s link cannot be written from this project")
                })?;
                value.insert("load", Value::from(relative));
            } else if let Some(m) = modules.get(&load) {
                value.insert("load", Value::from(m.as_str()));
            }
        }
        instances.insert(new, Item::Value(Value::InlineTable(value)));
        if let Some(part) = from.get(&format!("ui/parts/{slot}.html")) {
            let target = format!("ui/parts/{new}.html");
            if has(&target) {
                return Err(format!("{target} exists"));
            }
            copies.push((
                target,
                part.replace(&format!("part=\"{slot}\""), &format!("part=\"{new}\"")),
            ));
        }
    }
    let rename_end = |end: &str| -> String {
        match end.split_once('.') {
            Some((i, p)) => format!("{}.{p}", renamed.get(i).map_or(i, String::as_str)),
            None => end.to_owned(),
        }
    };
    let mut wires: Vec<Value> = Vec::new();
    if let Some(array) = src
        .get("wiring")
        .and_then(|w| w.get("connections"))
        .and_then(Item::as_array)
    {
        for value in array.iter() {
            let Some(table) = value.as_inline_table() else {
                continue;
            };
            let mut table = table.clone();
            for key in ["from", "to"] {
                if let Some(end) = table.get(key).and_then(Value::as_str).map(rename_end) {
                    table.insert(key, Value::from(end));
                }
            }
            let mut value = Value::InlineTable(table);
            value.decor_mut().set_prefix("\n  ");
            wires.push(value);
        }
    }
    if !wires.is_empty() {
        let wiring = host
            .entry("wiring")
            .or_insert(toml_edit::table())
            .as_table_like_mut()
            .ok_or("[wiring] is not a table")?;
        let array = wiring
            .entry("connections")
            .or_insert(toml_edit::value(toml_edit::Array::new()))
            .as_array_mut()
            .ok_or("`connections` is not an array")?;
        let trailing = array.trailing().clone();
        for wire in wires {
            array.push_formatted(wire);
        }
        array.set_trailing(trailing);
        array.set_trailing_comma(true);
    }
    let mut edits = vec![("project.toml".to_owned(), host.to_string())];

    // The new Stack, to the right of the others.
    if let Some(plane) = host_plane {
        let mut doc = parse(&plane_path, plane)?;
        let right = doc
            .get("stack")
            .and_then(Item::as_array_of_tables)
            .map(|stacks| {
                stacks
                    .iter()
                    .map(|s| {
                        let x = s
                            .get("at")
                            .and_then(Item::as_array)
                            .and_then(|a| a.get(0))
                            .and_then(Value::as_integer)
                            .unwrap_or(0);
                        x + s.get("width").and_then(Item::as_integer).unwrap_or(160)
                    })
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        let width = from
            .get(&plane_path)
            .and_then(|p| {
                instruments(p, &stack).or_else(|| {
                    p.parse::<toml::Table>().ok().and_then(|t| {
                        t.get("stack")?
                            .as_array()?
                            .first()?
                            .get("width")?
                            .as_integer()
                            .map(|w| (Vec::new(), w))
                    })
                })
            })
            .map_or(160, |(_, w)| w);
        let mut table = toml_edit::Table::new();
        table.insert("name", toml_edit::value(stack.as_str()));
        let mut at = toml_edit::Array::new();
        at.push(right + 40);
        at.push(0);
        table.insert("at", toml_edit::value(at));
        table.insert("width", toml_edit::value(width));
        let mut list = toml_edit::Array::new();
        for slot in &slots {
            list.push(lookup(&renamed, slot)?.as_str());
        }
        table.insert("instruments", toml_edit::value(list));
        match doc.get_mut("stack").and_then(Item::as_array_of_tables_mut) {
            Some(stacks) => stacks.push(table),
            None => {
                let mut stacks = toml_edit::ArrayOfTables::new();
                stacks.push(table);
                doc.insert("stack", Item::ArrayOfTables(stacks));
            }
        }
        edits.push((plane_path, doc.to_string()));
    }
    Ok((copies, edits, stack))
}

/// `to` relative to the folder `from` (both canonical): `./x`, `../x`,
/// `../../a/b`; `None` when they share no root.
pub fn relative_folder(from: &Path, to: &Path) -> Option<String> {
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..a.len() {
        parts.push("..".to_owned());
    }
    for c in b.get(common..).unwrap_or(&[]) {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    let text = parts.join("/");
    Some(if text.starts_with("..") {
        text
    } else {
        format!("./{text}")
    })
}

/// `map`'s value at `key`, or the error that it is missing.
fn lookup<'m>(map: &'m BTreeMap<String, String>, key: &str) -> Result<&'m String, String> {
    map.get(key).ok_or_else(|| format!("`{key}` is missing"))
}
