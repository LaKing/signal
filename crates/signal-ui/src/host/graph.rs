//! The project's structure: `describe`, `graph`, `modules` and their pushes.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use serde_json::{Value, json};
use signal_core::{Direction as PortDirection, PortAddress, PortShape};
use signal_graph::ExecutionPlan;
use signal_output::{SCOPE_POINTS, VECTOR_MAX_ROWS, VECTOR_SLOTS};
use signal_project::live::FolderState;
use signal_project::schematic::{self, SCHEMATIC_FILE};
use signal_project::{PATTERNS, PROJECT_FILE, Project};

use crate::app;
use crate::panel_write::sha256_hex;

use super::failure::{Failure, fail};
use super::{Host, with};

/// Level `level` of a `graph` message: `""` the top level, else a composite.
pub(super) fn level_of<'a>(graph: &'a Value, level: &str) -> Result<&'a Value, Failure> {
    let found = if level.is_empty() {
        graph.get("top")
    } else {
        graph.get("composites").and_then(|c| c.get(level))
    };
    found.ok_or_else(|| {
        fail(
            "unknown-level",
            format!("the project uses no composite `{level}`"),
        )
    })
}

impl Host {
    /// `describe`: the running plan, the controls, the sinks, meters and
    /// scopes.
    pub(super) fn describe(&self, restarted: bool) -> Value {
        let empty = Vec::new();
        let instances = self.plan.as_ref().map_or(&empty, |p| &p.instances);
        let described: Vec<Value> = instances
            .iter()
            .map(|i| {
                let ports: Vec<Value> = i
                    .descriptor
                    .ports
                    .iter()
                    .filter_map(|p| {
                        let address = PortAddress::new(&i.address, p.name).ok()?;
                        let value = serde_json::to_value(p).ok()?;
                        Some(with(value, "address", json!(address.as_str())))
                    })
                    .collect();
                let parameters: Vec<Value> = i
                    .descriptor
                    .parameters
                    .iter()
                    .filter_map(|p| {
                        let address = PortAddress::new(&i.address, p.name).ok()?;
                        let base = i.parameters.get(p.name).copied().unwrap_or(p.default);
                        let value = serde_json::to_value(p).ok()?;
                        let value = with(value, "address", json!(address.as_str()));
                        Some(with(value, "base", json!(base)))
                    })
                    .collect();
                json!({
                    "address": i.address.as_str(),
                    "module": i.type_name,
                    "sink": i.descriptor.sink,
                    "ports": ports,
                    "parameters": parameters,
                })
            })
            .collect();
        let sinks: Vec<Value> = instances
            .iter()
            .filter(|i| i.descriptor.sink.is_some())
            .enumerate()
            .map(|(index, i)| {
                let inputs = i
                    .descriptor
                    .ports
                    .iter()
                    .filter(|p| p.direction == PortDirection::In && p.shape == PortShape::Signal)
                    .count();
                json!({
                    "index": index,
                    "address": i.address.as_str(),
                    "kind": i.descriptor.sink,
                    "inputs": inputs,
                })
            })
            .collect();
        let combine_of = |port: &PortAddress| {
            instances
                .iter()
                .find(|i| i.address == port.instance())
                .and_then(|i| {
                    i.descriptor
                        .parameters
                        .iter()
                        .find(|p| p.name == port.port())
                })
                .map(|p| json!(p.combine))
        };
        let mut controls = Vec::new();
        for instance in self.panel.view(&self.live.values) {
            for p in &instance.parameters {
                let Some(target) = self.panel.target(&instance.name, &p.name) else {
                    continue;
                };
                let address = format!("/{}.{}", instance.name, p.name);
                let base = self.control_base(&instance.name, p);
                controls.push(json!({
                    "address": address,
                    "instance": instance.name,
                    "parameter": p.name,
                    "module": instance.module,
                    "ports": target.ports.iter().map(PortAddress::as_str).collect::<Vec<_>>(),
                    "unit": p.unit,
                    "min": p.min,
                    "max": p.max,
                    "default": p.default,
                    "base": base,
                    "inFile": p.in_file,
                    "inSnapshots": p.in_snapshots,
                    "modulated": p.modulated,
                    "combine": target.ports.first().and_then(combine_of),
                    // ADR 0043: a discrete control's values, each named.
                    "values": p.values.as_ref().map(|values| {
                        values
                            .iter()
                            .map(|(value, label)| json!({"value": value, "label": label}))
                            .collect::<Vec<_>>()
                    }),
                    // ADR 0043: how a value between them is read (and
                    // snapped): `positive` (anything above 0 is on), else
                    // left out (the nearest, ties upward).
                    "snap": (p.snap == signal_core::Snap::Positive).then_some("positive"),
                }));
            }
        }
        let (output_meters, input_meters): (Vec<Value>, Vec<Value>) = match &self.run {
            Some(run) => (
                (0..run.width)
                    .map(|c| {
                        let device = run.output.target(c);
                        json!({"channel": c, "device": device, "label": app::meter_label(c, device)})
                    })
                    .collect(),
                (0..run.input_channels)
                    .map(|c| json!({"channel": c}))
                    .collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let mut describe = json!({
            "type": "describe",
            "planVersion": self.run.as_ref().map(|r| r.version),
            "generation": self.generation,
            "sampleRate": self.rate,
            "project": self.run.as_ref().map(|r| r.current.name.clone()),
            "playing": self.run.is_some(),
            "instances": described,
            "controls": controls,
            "sinks": sinks,
            "meters": {"output": output_meters, "input": input_meters},
            "scopes": with(
                with(self.scopes_json(), "points", json!(SCOPE_POINTS)),
                "maxRate",
                json!(self.rate),
            ),
            "vectors": {"slots": VECTOR_SLOTS, "maxRows": VECTOR_MAX_ROWS},
            "unsaved": self.unsaved_json(),
            // ADR 0035 §8, ADR 0037 §7: includes (with their rate notes)
            // and slots; null without either.
            "content": self.run.as_ref().map(|r| r.current.content_json(&self.registry, self.rate)),
            // ADR 0046 §11 (additive): the slots' parts, their controls rooted.
            "parts": self.parts_json(),
        });
        if restarted {
            describe = with(describe, "restarted", json!(true));
        }
        describe
    }

    /// A project folder loaded and planned ([`app::load`]), counted for the
    /// test hooks.
    pub(super) fn load(
        &mut self,
        dir: &Path,
    ) -> Result<(Project, ExecutionPlan, usize), Vec<String>> {
        self.counts.0 += 1;
        app::load(dir, &self.registry)
    }

    /// Keeps the structure of `project` (loaded from the folder in `state`)
    /// for `graph` while stopped.
    pub(super) fn remember_stopped(
        &mut self,
        state: Option<FolderState>,
        project: &Project,
        plan: &ExecutionPlan,
    ) {
        self.counts.1 += 1;
        let structure = project.structure(plan, &self.registry);
        let modules = project.modules(&self.registry);
        self.graph_stopped = Some((
            self.dir.clone(),
            state,
            project.name.clone(),
            structure,
            modules,
        ));
    }

    /// `graph`'s project name and structure ([`Project::structure`]),
    /// cached: playing, per plan version; stopped, per folder state, so a
    /// call loads the folder again only after it changed.
    fn structure(&mut self) -> Result<(&str, &Value), Failure> {
        if let Some(run) = &self.run {
            let version = run.version;
            let entry = match self.graph_playing.take() {
                Some(entry) if entry.0 == version => self.graph_playing.insert(entry),
                _ => {
                    let plan = self
                        .plan
                        .as_ref()
                        .ok_or_else(|| fail("does-not-load", "the running project has no plan"))?;
                    self.counts.1 += 1;
                    let structure = run.current.structure(plan, &self.registry);
                    self.graph_playing
                        .insert((version, run.current.name.clone(), structure))
                }
            };
            return Ok((&entry.1, &entry.2));
        }
        if self.app.as_ref().is_some_and(|m| !m.opened) {
            return Err(fail("no-project", "open a project folder first"));
        }
        let state = FolderState::read(&self.dir).ok();
        let cached = self
            .graph_stopped
            .as_ref()
            .is_some_and(|(dir, seen, ..)| *dir == self.dir && seen.is_some() && *seen == state);
        if !cached {
            let dir = self.dir.clone();
            let (project, plan, _) = self
                .load(&dir)
                .map_err(|diagnostics| fail("does-not-load", diagnostics.join("\n")))?;
            self.remember_stopped(state, &project, &plan);
        }
        match &self.graph_stopped {
            Some((_, _, name, structure, _)) => Ok((name, structure)),
            None => Err(fail(
                "does-not-load",
                "the project's structure is not known",
            )),
        }
    }

    /// `graph` (ADR 0026 §1): the project's structure as written
    /// ([`Host::structure`]), and its schematic positions from
    /// `ui/schematic.toml`, read now. Playing, of the running project (the
    /// last good version, as `describe`); stopped (app mode), of the folder.
    pub(super) fn graph(&mut self) -> Result<Value, Failure> {
        let (name, top, composites, insides) = {
            let (name, structure) = self.structure()?;
            (
                name.to_owned(),
                structure.get("top").cloned(),
                structure.get("composites").cloned(),
                // ADR 0037, editable insides (additive; absent without one).
                structure.get("insides").cloned(),
            )
        };
        let (positions, error) = match fs::read_to_string(self.dir.join(SCHEMATIC_FILE)) {
            Ok(text) => match schematic::read_positions(&text) {
                Ok(positions) => (positions, None),
                Err(error) => (Default::default(), Some(error)),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Default::default(), None)
            }
            Err(error) => (
                Default::default(),
                Some(format!("{SCHEMATIC_FILE}: {error}")),
            ),
        };
        let positions: serde_json::Map<String, Value> = positions
            .into_iter()
            .map(|(level, entries)| {
                let entries: serde_json::Map<String, Value> = entries
                    .into_iter()
                    .map(|(instance, (x, y))| (instance, json!({"x": x, "y": y})))
                    .collect();
                (level, Value::Object(entries))
            })
            .collect();
        let mut graph = json!({
            "type": "graph",
            "project": name,
            "folder": self.folder_name(),
            "projectId": self.project_id(),
            "generation": self.generation,
            "planVersion": self.run.as_ref().map(|r| r.version),
            "top": top,
            "composites": composites,
            "layout": {"file": SCHEMATIC_FILE, "positions": positions, "error": error},
            "files": self.files_json(),
            "interfaces": self.interfaces_json(),
        });
        if let (Some(insides), Value::Object(fields)) = (insides, &mut graph) {
            fields.insert("insides".to_owned(), insides);
        }
        // ADR 0046 §6: each slot box's part, its controls rooted.
        if let Some(top) = graph.get_mut("top") {
            self.add_parts(top, "");
        }
        if let Some(Value::Object(levels)) = graph.get_mut("composites") {
            for (name, level) in levels.iter_mut() {
                self.add_parts(level, name);
            }
        }
        Ok(graph)
    }

    /// ADR 0046 §2: the part of slot `slot` at `level` rooted at `root`, as
    /// `graph` and `describe` give it: `{file, base, controls, error}`,
    /// `null` when the slot has none.
    fn part_json(&self, level: &str, slot: &str, root: &str) -> Value {
        let path = signal_project::patch::part_path(level, slot);
        let Some(text) = self.ui_text(&path) else {
            return Value::Null;
        };
        let key = signal_project::patch::part_key(level, slot);
        let wanted = crate::parts::marker(&key);
        let error = match crate::parts::marker_in(&text) {
            Some((found, _)) if found == wanted => Value::Null,
            Some((found, _)) => json!(format!(
                "its <signal-panel> names `{found}`, not its slot `{wanted}` (part=\"{wanted}\")"
            )),
            None => json!(format!("no <signal-panel part=\"{wanted}\">")),
        };
        json!({
            "file": path,
            "base": sha256_hex(text.as_bytes()),
            "root": root,
            "controls": crate::parts::rooted_controls(&text, root),
            "error": error,
        })
    }

    /// `level`'s slot boxes (a `graph` level: the top level, or composite
    /// `composite`'s) given their `slot.part`.
    fn add_parts(&self, level: &mut Value, composite: &str) {
        let Some(Value::Array(instances)) = level.get_mut("instances") else {
            return;
        };
        for instance in instances {
            let name = instance
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let root = instance
                .get("address")
                .and_then(Value::as_str)
                .map_or_else(|| format!("/{name}"), str::to_owned);
            let Some(Value::Object(slot)) = instance.get_mut("slot") else {
                continue;
            };
            slot.insert("part".to_owned(), self.part_json(composite, &name, &root));
        }
    }

    /// `describe.parts` (ADR 0046 §11): every slot's part that exists, its
    /// file, the roots it is rendered at and its controls rooted at the
    /// first, in key order.
    fn parts_json(&self) -> Value {
        let mut list = Vec::new();
        for key in self.panel.part_keys() {
            let (level, slot) = key.split_once('.').unwrap_or(("", key));
            let roots = self.panel.part_roots(key).unwrap_or_default();
            let root = roots
                .first()
                .cloned()
                .unwrap_or_else(|| format!("/{}", crate::parts::marker(key)));
            let part = self.part_json(level, slot, &root);
            if part.is_null() {
                continue;
            }
            list.push(with(
                with(part, "slot", json!(crate::parts::slot_of(key))),
                "roots",
                json!(roots),
            ));
        }
        Value::Array(list)
    }

    /// `graph.interfaces` (ADR 0037 §3, phase 3, additive): the interface
    /// templates a slot may start from, the library's (`lib.voice`, …) then
    /// the project's `interfaces/<name>.toml` that read, each in name order:
    /// `{name, library, description, ports, base}` (`ports` as `describe`'s
    /// `content.slots` interface ports; `base`: the file's SHA-256, null for a
    /// library template), read with every `graph`.
    fn interfaces_json(&self) -> Value {
        let mut list: Vec<Value> = signal_project::library_interfaces()
            .into_iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "library": true,
                    "description": t.interface.description,
                    "ports": t.interface.ports,
                    "base": null,
                })
            })
            .collect();
        let sources = signal_project::read_sources(&self.dir).unwrap_or_default();
        for (name, interface) in signal_project::interface_templates(&sources) {
            let base = sources
                .iter()
                .find(|s| s.path == format!("{}/{name}.toml", signal_project::INTERFACES))
                .map(|s| sha256_hex(s.text.as_bytes()));
            list.push(json!({
                "name": name,
                "library": false,
                "description": interface.description,
                "ports": interface.ports,
                "base": base,
            }));
        }
        Value::Array(list)
    }

    /// `graph.projectId` (M2.9-3): which project folder this is, opaque (the
    /// SHA-256 of its canonical path; the path itself is not sent), so a
    /// page's patch history stays with the project it was written in even
    /// when two have the same folder name and project name.
    pub(super) fn project_id(&self) -> String {
        let dir = fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone());
        sha256_hex(dir.as_os_str().as_encoded_bytes())
    }

    /// `graph.files` (M2.9-3): the SHA-256 of `project.toml` (`""`) and of
    /// each `composites/<name>.toml` (by name) as they are now, the bases a
    /// `patch` names.
    fn files_json(&self) -> Value {
        let mut files = serde_json::Map::new();
        let sources = signal_project::read_sources(&self.dir).unwrap_or_default();
        // A pattern's base is `pattern.read`'s, not a patch's.
        let patterns = format!("{PATTERNS}/");
        for source in sources.iter().filter(|s| !s.path.starts_with(&patterns)) {
            let key = match source.path.as_str() {
                PROJECT_FILE => String::new(),
                path => path
                    .strip_prefix("composites/")
                    .and_then(|p| p.strip_suffix(".toml"))
                    .unwrap_or(path)
                    .to_owned(),
            };
            files.insert(key, json!(sha256_hex(source.text.as_bytes())));
        }
        Value::Object(files)
    }

    /// `modules` (M2.9-3): the module types an instance can be added from,
    /// of the project `graph` describes (playing, the running one; stopped,
    /// the folder's, cached with its structure). A composite's `inside`
    /// only when the message asks for it (`inside: true`, the Add module
    /// browser's schematic preview).
    pub(super) fn modules(&mut self, message: &Value) -> Result<Value, Failure> {
        let inside = match message.get("inside") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(inside)) => *inside,
            Some(_) => return Err(fail("bad-message", "`inside` must be true or false")),
        };
        let modules = match &self.run {
            Some(run) => run.current.modules(&self.registry),
            None => {
                self.structure()?;
                match &self.graph_stopped {
                    Some((.., modules)) => modules.clone(),
                    None => {
                        return Err(fail("does-not-load", "the project's modules are not known"));
                    }
                }
            }
        };
        let modules = if inside {
            modules
        } else {
            signal_project::modules_without_inside(modules)
        };
        Ok(with(modules, "type", json!("modules")))
    }

    /// Pushes `graph` to every client when it differs from the last one
    /// pushed (compared by the hash of its text), or (`force`) whatever it
    /// is: after a swap and a schematic write. A project that does not load
    /// pushes `graph.error` with its diagnostics, and clients keep their
    /// last drawing, marked stale.
    pub(super) fn push_graph(&mut self, force: bool) {
        let message = match self.graph() {
            Ok(graph) => graph,
            Err(failure) => json!({
                "type": "graph.error",
                "folder": self.folder_name(),
                "code": failure.code,
                "message": failure.message,
            }),
        };
        let text = message.to_string();
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();
        if force || self.graph_pushed != Some(hash) {
            self.broadcast_text(&text);
            self.graph_pushed = Some(hash);
        }
    }

    /// What a removal of `names` at `level` would leave dangling: the
    /// composite's `[exports]` naming them and the panel bindings in `ui/*.html`
    /// naming them (through every instance of the composite).
    pub(super) fn references(
        &self,
        level: &str,
        names: &[&str],
        project: Option<&Project>,
    ) -> Vec<Value> {
        let mut found = Vec::new();
        if names.is_empty() {
            return found;
        }
        let instance = |target: &str| target.split('.').next().unwrap_or_default().to_owned();
        if !level.is_empty()
            && let Some(def) = project.and_then(|p| p.composites.iter().find(|c| c.name == level))
        {
            let file = format!("composites/{level}.toml");
            let mut export = |kind: &str, name: &str, target: &str| {
                if names.contains(&instance(target).as_str()) {
                    found.push(json!({"file": file, "text": format!("[exports] {kind}.{name} = {target}")}));
                }
            };
            for e in &def.exports.inputs {
                e.to.iter().for_each(|t| export("inputs", &e.name, t));
            }
            for e in &def.exports.outputs {
                export("outputs", &e.name, &e.from);
            }
            for e in &def.exports.parameters {
                e.to.iter().for_each(|t| export("parameters", &e.name, t));
            }
        }
        let paths: Vec<String> = if level.is_empty() {
            vec![String::new()]
        } else {
            project.map(|p| p.instances_of(level)).unwrap_or_default()
        };
        let prefixes: Vec<String> = paths
            .iter()
            .flat_map(|path| names.iter().map(move |name| format!("{path}/{name}")))
            .collect();
        for (file, line, address) in crate::bindings::references(&self.dir.join("ui"), &prefixes) {
            found.push(json!({"file": format!("ui/{file}"), "line": line, "text": address}));
        }
        found
    }
}
