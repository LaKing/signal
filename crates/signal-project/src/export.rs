//! ADR 0046 §9, ADR 0051 §10: "Save slot as project…": a top-level slot
//! written out as a project of its own, no new format (ADR 0035).
//!
//! - **The content becomes the project.** A slot holding a project
//!   composite (its own inside, Make instrument's or Group's) gives its
//!   instances, connections and exports as `project.toml`'s; a slot holding
//!   a primitive or a library module gives one instance of it, named after
//!   the slot, with every interface port exported under its name. The
//!   project composites it uses are copied with it, and so are the
//!   patterns its instances name.
//! - **The slot's values travel:** a value set on the slot under an
//!   interface parameter's name, and a `set` key, are written on the
//!   instances the export reaches.
//! - **The seed** is the host's: under a slot with `seed = "run"` every
//!   instance then draws, alone, the stream it drew inside the host (its
//!   address relative to the slot is its address in the project). A slot
//!   without it keyed its streams by its own address, which the project
//!   alone cannot repeat.
//! - **The part** (`ui/parts/<slot>.html`) becomes `ui/main.html`, each
//!   `.p` binding rooted on the export it names (`.kick_tune` →
//!   `/kick.tune`), the content paths (`/kick.tune`) as they are.
//!
//! Refused, with the reason: a slot holding a project (that folder is the
//! project already), an empty slot, a `map`, `voices`, a content with slots
//! inside.

use std::collections::{BTreeMap, BTreeSet};

use toml_edit::{DocumentMut, Item, Value};

use crate::{Project, SourceFile, is_project_path};

/// The files of slot `slot` of `project` as a project of its own, with
/// `part`, the text of its part (`ui/parts/<slot>.html`) if it has one.
pub fn export_slot(
    project: &Project,
    slot: &str,
    part: Option<&str>,
) -> Result<Vec<SourceFile>, String> {
    let file = |path: &str| {
        project
            .files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.text.clone())
    };
    let host: toml::Table = file("project.toml")
        .ok_or("the project has no project.toml")?
        .parse()
        .map_err(|e| format!("project.toml does not parse: {e}"))?;
    let entry = host
        .get("instances")
        .and_then(|i| i.get(slot))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("no instance `{slot}` at the top level"))?;
    let interface = entry
        .get("slot")
        .ok_or_else(|| format!("`{slot}` is not a slot"))?;
    let load = entry
        .get("load")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("`{slot}` is empty: there is nothing to save"))?;
    if is_project_path(load) {
        return Err(format!(
            "`{slot}` holds the project `{load}`: that folder is the project already"
        ));
    }
    for key in ["map", "voices"] {
        if entry.get(key).is_some() {
            return Err(format!("`{slot}` has `{key}`: save it after removing it"));
        }
    }

    let composite = |name: &str| file(&format!("{}/{name}.toml", crate::COMPOSITES));
    let mut doc: DocumentMut = match composite(load) {
        Some(text) => text
            .parse()
            .map_err(|e| format!("composites/{load}.toml does not parse: {e}"))?,
        None => wrapper(slot, load, interface)?
            .parse()
            .map_err(|e| format!("the wrapper does not parse: {e}"))?,
    };
    doc.remove("schema_version");
    doc.remove("composite");

    // The slot's values on the instances its exports reach.
    let exported = |name: &str| -> Vec<String> {
        match doc
            .get("exports")
            .and_then(|e| e.get("parameters"))
            .and_then(|p| p.get(name))
        {
            Some(Item::Value(Value::String(s))) => vec![s.value().clone()],
            Some(Item::Value(Value::Array(a))) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        }
    };
    let reserved = [
        "slot",
        "load",
        "map",
        "seed",
        "set",
        "voices",
        "snapshot_exclude",
    ];
    let mut values: Vec<(String, f64)> = Vec::new();
    for (key, value) in entry {
        if reserved.contains(&key.as_str()) {
            continue;
        }
        if let Some(x) = value
            .as_float()
            .or_else(|| value.as_integer().map(|i| i as f64))
        {
            for target in exported(key) {
                values.push((target, x));
            }
        }
    }
    if let Some(set) = entry.get("set").and_then(toml::Value::as_table) {
        for (key, value) in set {
            if let Some(x) = value
                .as_float()
                .or_else(|| value.as_integer().map(|i| i as f64))
            {
                values.extend(exported(key).into_iter().map(|t| (t, x)));
            }
        }
    }
    for (target, x) in values {
        let Some((instance, parameter)) = target.split_once('.') else {
            continue;
        };
        if let Some(table) = doc
            .get_mut("instances")
            .and_then(|i| i.get_mut(instance))
            .and_then(Item::as_value_mut)
            .and_then(Value::as_inline_table_mut)
        {
            table.insert(parameter, crate::write::number(x));
        }
    }

    // What it uses: project composites (transitively) and patterns.
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut patterns: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<toml::Table> = vec![doc.to_string().parse().map_err(|e| format!("{e}"))?];
    while let Some(table) = queue.pop() {
        let Some(instances) = table.get("instances").and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, entry) in instances {
            if entry.get("slot").is_some() {
                return Err(format!(
                    "its content has a slot inside (`{name}`): saving nested slots is not built"
                ));
            }
            if let Some(pattern) = entry.get("pattern").and_then(toml::Value::as_str) {
                patterns.insert(pattern.to_owned());
            }
            if let Some(module) = entry.get("module").and_then(toml::Value::as_str)
                && module != load
                && !used.contains(module)
                && let Some(text) = composite(module)
            {
                let parsed: toml::Table = text
                    .parse()
                    .map_err(|e| format!("composites/{module}.toml does not parse: {e}"))?;
                used.insert(module.to_owned());
                queue.push(parsed);
            }
        }
    }

    let header = format!(
        "schema_version = 1\n\n# Saved from slot `{slot}` of `{}` (ADR 0046 §9).\n[project]\nname = {}\nsample_rate = {}\nseed = {}\n\n",
        project.name,
        Value::from(slot),
        project.sample_rate,
        project.seed,
    );
    let mut files = vec![SourceFile {
        path: "project.toml".to_owned(),
        text: format!("{header}{}", doc.to_string().trim_start()),
    }];
    for name in &used {
        let path = format!("{}/{name}.toml", crate::COMPOSITES);
        if let Some(text) = composite(name) {
            files.push(SourceFile { path, text });
        }
    }
    for name in &patterns {
        let path = format!("patterns/{name}.toml");
        let text = file(&path).ok_or_else(|| format!("{path} is missing"))?;
        files.push(SourceFile { path, text });
    }
    if let Some(part) = part {
        files.push(SourceFile {
            path: "ui/main.html".to_owned(),
            text: rooted_part(part, slot, &doc),
        });
    }
    Ok(files)
}

/// The project text of one instance `slot` of module `load`, every port of
/// the slot's written interface exported under its name.
fn wrapper(slot: &str, load: &str, interface: &toml::Value) -> Result<String, String> {
    let table = interface
        .as_table()
        .ok_or("its interface is a template's: save its content after detaching it")?;
    let mut text = format!(
        "[instances]\n{slot} = {{ module = {} }}\n",
        Value::from(load)
    );
    let mut sections = Vec::new();
    for role in ["inputs", "outputs", "parameters"] {
        let Some(ports) = table.get(role).and_then(toml::Value::as_table) else {
            continue;
        };
        let items: Vec<String> = ports
            .keys()
            .map(|p| format!("{p} = {}", Value::from(format!("{slot}.{p}"))))
            .collect();
        if !items.is_empty() {
            sections.push(format!("{role} = {{ {} }}", items.join(", ")));
        }
    }
    if !sections.is_empty() {
        text.push_str(&format!("\n[exports]\n{}\n", sections.join("\n")));
    }
    Ok(text)
}

/// The part's text as the project's own panel: the marker dropped, each
/// `.p` binding rooted on the first port export `p` names.
fn rooted_part(part: &str, slot: &str, doc: &DocumentMut) -> String {
    let mut targets: BTreeMap<String, String> = BTreeMap::new();
    if let Some(exports) = doc.get("exports").and_then(Item::as_table_like) {
        for (_, role) in exports.iter() {
            let Some(role) = role.as_table_like() else {
                continue;
            };
            for (name, value) in role.iter() {
                let first = match value {
                    Item::Value(Value::String(s)) => Some(s.value().clone()),
                    Item::Value(Value::Array(a)) => {
                        a.iter().find_map(|v| v.as_str().map(str::to_owned))
                    }
                    _ => None,
                };
                if let Some(first) = first {
                    targets.insert(name.to_owned(), format!("/{first}"));
                }
            }
        }
    }
    let text = part
        .replace(&format!(" part=\"{slot}\""), "")
        .replace(&format!(" part='{slot}'"), "");
    // Every binding attribute's addresses, `.p` rooted.
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find("bind=\"") {
        let (before, after) = rest.split_at(at + "bind=\"".len());
        out.push_str(before);
        let end = after.find('"').unwrap_or(after.len());
        let value = &after[..end];
        let rooted: Vec<String> = value
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|a| !a.is_empty())
            .map(|a| {
                a.strip_prefix('.')
                    .and_then(|p| targets.get(p).cloned())
                    .unwrap_or_else(|| a.to_owned())
            })
            .collect();
        out.push_str(&rooted.join(" "));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}
