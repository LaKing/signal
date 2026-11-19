//! ADR 0046 §9, ADR 0051 §10: "Save slot as project…": a top-level slot
//! written out as a project of its own, no new format (ADR 0035).
//!
//! - **The content becomes the project.** A slot holding a project
//!   composite (its own inside, Make instrument's or Group's) gives its
//!   instances, connections and exports as `project.toml`'s; a slot holding
//!   a primitive or a library module gives one instance of it, named after
//!   the slot, with every interface port exported under its name. The
//!   project composites it uses are copied with it, and so are the
//!   patterns its instances name, with the WAVs sample patterns read
//!   ([`sample_files`], ADR 0056 §1).
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
//! inside, a content that includes a project.
//!
//! [`export_project`] is what Signal.app and `signal slot-export` write:
//! the files, the samples and the panel's assets, and only once the
//! project they make loads and plans on its own (the review of 99b764b).

use std::collections::{BTreeMap, BTreeSet};

use toml_edit::{DocumentMut, Item, Value};

use crate::{Library, Match, Project, Role, SourceFile, is_project_path};

/// ADR 0056 §1: the WAV files the sample patterns among `files` (path,
/// text) read, from the project folder `dir`, by path: an exported project
/// carries its samples beside its text, or it does not load. One that
/// cannot be read is refused with its path.
pub fn sample_files<'a>(
    dir: &std::path::Path,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut samples: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, text) in files {
        let Some(file) = path
            .strip_prefix("patterns/")
            .filter(|p| p.ends_with(".toml"))
            .and_then(|_| crate::sample_file(text))
        else {
            continue;
        };
        if samples.iter().any(|(p, _)| *p == file) {
            continue;
        }
        let bytes = crate::read_regular(dir.join(&file))
            .map_err(|e| format!("{file}, the sample {path} reads: {e}"))?;
        samples.push((file, bytes));
    }
    Ok(samples)
}

/// The files of slot `slot` of `project` as a project of its own, with
/// `part`, the text of its part (`ui/parts/<slot>.html`) if it has one.
/// [`export_project`] also carries the samples and assets, exports only
/// the slot ports its content matches, and checks the result.
pub fn export_slot(
    project: &Project,
    slot: &str,
    part: Option<&str>,
) -> Result<Vec<SourceFile>, String> {
    export_matched(project, slot, part, None)
}

/// ADR 0046 §9: everything "Save slot as project…" writes for slot `slot`
/// of `project`, read from the folder `dir`, by path: [`export_slot`]'s
/// files (its part read from `ui/parts/<slot>.html`), the WAVs its sample
/// patterns read ([`sample_files`]) and the `ui/assets/` files its panel
/// names. Refused unless that project loads and plans with `registry` on
/// its own, so an export that reports success is one that opens.
pub fn export_project(
    dir: &std::path::Path,
    project: &Project,
    slot: &str,
    registry: &signal_core::Registry,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let planned = project
        .plan(registry)
        .map_err(|d| format!("the project does not plan: {}", lines(&d)))?;
    let matches = planned
        .slots
        .iter()
        .find(|r| r.slot == format!("/{slot}"))
        .map(|r| r.matches.as_slice());
    let part =
        crate::read_regular_to_string(dir.join("ui/parts").join(format!("{slot}.html"))).ok();
    let files = export_matched(project, slot, part.as_deref(), matches)?;
    let samples = sample_files(
        dir,
        files.iter().map(|f| (f.path.as_str(), f.text.as_str())),
    )?;
    let mut assets: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(main) = files.iter().find(|f| f.path == "ui/main.html") {
        for name in asset_names(&main.text) {
            let path = format!("ui/assets/{name}");
            let bytes = crate::read_regular(dir.join(&path))
                .map_err(|e| format!("{path}, which the part names: {e}"))?;
            assets.push((path, bytes));
        }
    }
    // Loaded as the folder it becomes: nothing outside it (an empty
    // folder of its own as its place), its samples from the export.
    let place = std::env::temp_dir();
    // Its render files: the panel is not planned (`ui check` reads it).
    let render_files: Vec<SourceFile> = files
        .iter()
        .filter(|f| !f.path.starts_with("ui/"))
        .cloned()
        .collect();
    let alone = Project::from_sources_planned(render_files, Library::builtin(), &place, &samples)
        .map_err(|d| format!("the exported project does not load: {}", lines(&d)))?;
    alone
        .plan(registry)
        .map_err(|d| format!("the exported project does not plan: {}", lines(&d)))?;
    Ok(files
        .into_iter()
        .map(|f| (f.path, f.text.into_bytes()))
        .chain(samples)
        .chain(assets)
        .collect())
}

/// Diagnostics on one line each, joined.
fn lines(diagnostics: &[crate::Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// The files under `ui/assets/` a panel's text names (`assets/dial.svg`),
/// each once, in order: a name with no `..`, `.` or empty part.
fn asset_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("assets/") {
        let before = rest[..at].chars().last();
        let after = &rest[at + "assets/".len()..];
        let end = after
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '?' | '#' | '<' | '>')
            })
            .unwrap_or(after.len());
        let name = &after[..end];
        let bounded = before.is_none_or(|c| matches!(c, '"' | '\'' | '(' | '/' | ' ' | '='));
        let safe = !name.is_empty()
            && name
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != "..");
        if bounded && safe && !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
        rest = &after[end..];
    }
    names
}

/// [`export_slot`], the wrapper of a module's content exporting only the
/// slot ports `matches` (the host plan's match table) says the content
/// matched; all of them without one.
fn export_matched(
    project: &Project,
    slot: &str,
    part: Option<&str>,
    matches: Option<&[Match]>,
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
    // A module's content is one instance named after the slot.
    let module = composite(load).is_none();
    let mut doc: DocumentMut = match composite(load) {
        Some(text) => text
            .parse()
            .map_err(|e| format!("composites/{load}.toml does not parse: {e}"))?,
        None => wrapper(slot, load, interface, matches)?
            .parse()
            .map_err(|e| format!("the wrapper does not parse: {e}"))?,
    };
    doc.remove("schema_version");
    doc.remove("composite");

    // The slot's values on the instances its exports reach: an export is
    // `"g.db"`, a list of them, or a table whose `to` is either (ADR 0042:
    // `{ to = "g.db", min = -40, max = 0 }`).
    let exported = |name: &str| -> Vec<String> {
        let targets = |value: &Value| -> Vec<String> {
            match value {
                Value::String(s) => vec![s.value().clone()],
                Value::Array(a) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
                _ => Vec::new(),
            }
        };
        match doc
            .get("exports")
            .and_then(|e| e.get("parameters"))
            .and_then(|p| p.get(name))
        {
            Some(Item::Value(Value::InlineTable(t))) => {
                t.get("to").map(targets).unwrap_or_default()
            }
            Some(Item::Table(t)) => t
                .get("to")
                .and_then(Item::as_value)
                .map(targets)
                .unwrap_or_default(),
            Some(Item::Value(value)) => targets(value),
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
                // A module's own parameter outside the interface (ADR 0037
                // §5): on its one instance, as the host applied it.
                let targets = exported(key);
                if targets.is_empty() && module {
                    values.push((format!("{slot}.{key}"), x));
                } else {
                    values.extend(targets.into_iter().map(|t| (t, x)));
                }
            }
        }
    }
    for (target, x) in values {
        let Some((instance, parameter)) = target.split_once('.') else {
            continue;
        };
        // Inline or a `[instances.<name>]` table (the review of 99b764b).
        match doc.get_mut("instances").and_then(|i| i.get_mut(instance)) {
            Some(Item::Value(Value::InlineTable(table))) => {
                table.insert(parameter, crate::write::number(x));
            }
            Some(Item::Table(table)) => {
                table.insert(parameter, Item::Value(crate::write::number(x)));
            }
            _ => {}
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
            if let Some(path) = entry.get("project").and_then(toml::Value::as_str) {
                return Err(format!(
                    "its content includes the project `{path}` (`{name}`): saving it with its includes is not built"
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

    // Its random streams: the slot's own seed when it has one (it drew from
    // it in the host), else the host's (the audit of 2026-11-14).
    let seed = entry
        .get("seed")
        .and_then(toml::Value::as_integer)
        .and_then(|s| u64::try_from(s).ok())
        .unwrap_or(project.seed);
    let header = format!(
        "schema_version = 1\n\n# Saved from slot `{slot}` of `{}` (ADR 0046 §9).\n[project]\nname = {}\nsample_rate = {}\nseed = {seed}\n\n",
        project.name,
        Value::from(slot),
        project.sample_rate,
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
/// the slot's written interface exported under its name, except one
/// `matches` says the module does not have (an unmatched spare port).
fn wrapper(
    slot: &str,
    load: &str,
    interface: &toml::Value,
    matches: Option<&[Match]>,
) -> Result<String, String> {
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
        let kind = match role {
            "inputs" => Role::Input,
            "outputs" => Role::Output,
            _ => Role::Parameter,
        };
        let matched = |port: &str| {
            matches.is_none_or(|all| {
                all.iter()
                    .any(|m| m.port == port && m.role == kind && m.content.is_some())
            })
        };
        let items: Vec<String> = ports
            .keys()
            .filter(|p| matched(p))
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
                    // ADR 0042: `{ to = "c.value", min = 0, max = 1 }`.
                    Item::Value(Value::InlineTable(t)) => t.get("to").and_then(|to| match to {
                        Value::String(s) => Some(s.value().clone()),
                        Value::Array(a) => a.iter().find_map(|v| v.as_str().map(str::to_owned)),
                        _ => None,
                    }),
                    Item::Table(t) => {
                        t.get("to")
                            .and_then(Item::as_value)
                            .and_then(|to| match to {
                                Value::String(s) => Some(s.value().clone()),
                                Value::Array(a) => {
                                    a.iter().find_map(|v| v.as_str().map(str::to_owned))
                                }
                                _ => None,
                            })
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
    // Every binding attribute's addresses, `.p` rooted, in either quote.
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    loop {
        let next = ["bind=\"", "bind='"]
            .iter()
            .filter_map(|open| rest.find(open).map(|at| (at, *open)))
            .min();
        let Some((at, open)) = next else {
            break;
        };
        let quote = open.chars().last().unwrap_or('"');
        let (before, after) = rest.split_at(at + open.len());
        out.push_str(before);
        let end = after.find(quote).unwrap_or(after.len());
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
