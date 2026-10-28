//! The project's structure as written (ADR 0026 §1), for the UI protocol's
//! `graph` message (`docs/ui-protocol.md`): the top level's instances and
//! connections and, for every composite the project uses, the same for its
//! inside with its exports. Nothing is flattened; the converters the
//! compiler inserted are listed on the connections they join, read from the
//! plan's warnings. Control-thread code: it allocates.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use signal_core::{Direction, LIBRARY_PREFIX, ParameterDescriptor, PortDescriptor, Registry, Snap};
use signal_graph::{CompositeDef, ExecutionPlan, GraphWarning, InstanceDef};

use crate::locate::{DefSpans, ExportKind};
use crate::slot::{Role, SlotResolution};
use crate::{Diagnostic, Library, PROJECT_FILE, Project, SourceFile};

/// What a written endpoint (`instance.port`) finally reaches.
enum Endpoint {
    Port(&'static PortDescriptor),
    Parameter(&'static ParameterDescriptor),
}

/// A composite's exports of one name, by (composite, export name): an
/// output's source, or the targets of an input or a parameter, in source order.
type ByExport<'a, T> = BTreeMap<(&'a str, &'a str), Vec<T>>;

struct Walk<'a> {
    project: &'a Project,
    registry: &'a Registry,
    by_name: BTreeMap<&'a str, &'a CompositeDef>,
    /// Each definition's instances by name (the top level's under its own name).
    instances: BTreeMap<&'a str, BTreeMap<&'a str, &'a InstanceDef>>,
    outputs: ByExport<'a, &'a str>,
    inputs: ByExport<'a, &'a [String]>,
    parameters: ByExport<'a, &'a [String]>,
    /// (flattened source port, flattened target) → the converter between.
    converters: BTreeMap<(String, String), Value>,
    /// ADR 0037: what the plan build decided for each slot, by its address
    /// as [`SlotResolution::slot`] writes it (`/fx`, `<composite>/fx`).
    slots: BTreeMap<String, SlotResolution>,
}

/// An entry of a level in the order its file writes them: an instance of
/// the plan, or an empty slot (which plans as nothing, ADR 0037 §5).
enum Written<'a> {
    Instance(&'a InstanceDef),
    EmptySlot(&'a str),
}

impl Project {
    /// The structure as written, of this project and its plan `plan`
    /// (`docs/ui-protocol.md` "graph"): `{"top": level, "composites":
    /// {name: level with "inputs", "outputs", "parameters"}}`. Instances and
    /// connections are in source order, composites in name order. Inside a
    /// composite, addresses are relative to an instance of it (`kick.out`);
    /// its connections' converters are those of its first instance in source
    /// order (a type's rates are the same in every instance).
    pub fn structure(&self, plan: &ExecutionPlan, registry: &Registry) -> Value {
        let type_of: BTreeMap<&str, &str> = plan
            .instances
            .iter()
            .map(|i| (i.address.as_str(), i.type_name))
            .collect();
        let mut converters = BTreeMap::new();
        for warning in &plan.warnings {
            if let GraphWarning::ConverterInserted {
                from,
                to,
                converter,
                conversion,
                from_rate,
                to_rate,
                ..
            } = warning
            {
                let name = converter.segments().last().unwrap_or_default();
                converters.insert(
                    (from.as_str().to_owned(), to.as_str().to_owned()),
                    json!({
                        "name": name,
                        "module": type_of.get(converter.as_str()),
                        "conversion": conversion.to_string(),
                        "fromRate": from_rate.get(),
                        "toRate": to_rate.get(),
                    }),
                );
            }
        }
        let mut walk = self.walk(registry, converters);
        // ADR 0037 §7: the slots' match tables, built only when there are slots.
        if self.has_slots() {
            walk.slots = self
                .plan(registry)
                .map(|p| p.slots)
                .unwrap_or_default()
                .into_iter()
                .map(|r| (r.slot.clone(), r))
                .collect();
        }
        // The first instance (in source order) of every composite reached.
        let mut first = BTreeMap::new();
        walk.reach(&self.root, "", &mut first);
        let top = walk.level(&self.root, "", false);
        let mut composites = Map::new();
        for (name, prefix) in &first {
            // A library composite's instance shows its interface; its inside
            // is not opened in the UI yet (ADR 0027 §6, a later issue).
            if name.starts_with(signal_core::LIBRARY_PREFIX) {
                continue;
            }
            let Some(def) = walk.by_name.get(name.as_str()) else {
                continue;
            };
            composites.insert(name.clone(), self.composite_level(&walk, def, prefix));
        }
        // ADR 0037, editable insides: a slot's own inside shows the slot's
        // interface on its frame, and the inside a slot would get when it
        // is first edited is given for the read-only view (`insides`,
        // additive: absent without slots).
        let insides = walk.insides(&mut composites);
        let mut graph = json!({"top": top, "composites": composites});
        if !insides.is_empty()
            && let Value::Object(fields) = &mut graph
        {
            fields.insert("insides".to_owned(), Value::Object(insides));
        }
        graph
    }

    /// Composite `def`'s level as `graph` gives it in `composites`: its
    /// inside (instances and connections, addresses relative to an instance
    /// of it at `prefix`) with its exported `inputs`, `outputs` and
    /// `parameters`, each carrying its `order` and `group`.
    fn composite_level(&self, walk: &Walk<'_>, def: &CompositeDef, prefix: &str) -> Value {
        let name = def.name.as_str();
        let mut level = walk.level(def, prefix, true);
        if let Value::Object(fields) = &mut level {
            let mut inputs = walk.exported_inputs(def);
            let mut outputs = walk.exported_outputs(def);
            let mut parameters = walk.exported_parameters(def);
            self.presentation(name, ExportKind::Input, &mut inputs);
            self.presentation(name, ExportKind::Output, &mut outputs);
            self.presentation(name, ExportKind::Parameter, &mut parameters);
            fields.insert("inputs".to_owned(), inputs);
            fields.insert("outputs".to_owned(), outputs);
            fields.insert("parameters".to_owned(), parameters);
        }
        level
    }

    /// M2.13-2 (`docs/ui-protocol.md` "graph"): each export of `kind` of
    /// composite `name` in `list` (`graph`'s, in name order) given `order`,
    /// its place among the composite's exports of that kind as the file
    /// writes them, and its `group` when the file names one. Presentation
    /// only, for the schematic; read from the file's spans.
    fn presentation(&self, name: &str, kind: ExportKind, list: &mut Value) {
        let Some(spans) = self.spans.composites.get(name) else {
            return;
        };
        let mut written: Vec<&crate::locate::ExportSpans> =
            spans.exports.iter().filter(|e| e.kind == kind).collect();
        written.sort_by_key(|e| e.key.start);
        let Value::Array(entries) = list else {
            return;
        };
        for entry in entries {
            let Value::Object(fields) = entry else {
                continue;
            };
            let at = written
                .iter()
                .position(|e| fields.get("name").and_then(Value::as_str) == Some(e.name.as_str()));
            if let Some(at) = at {
                fields.insert("order".to_owned(), json!(at));
                if let Some(group) = &written[at].group {
                    fields.insert("group".to_owned(), json!(group));
                }
            }
        }
    }

    /// The module types an instance can be added from (`docs/ui-protocol.md`
    /// "modules"): `registry`'s primitive types, then the project's
    /// composites and the built-in library's ([`library_modules`], with
    /// `library: true` and, on a validation reference, `reference_for`),
    /// each in name order, with their ports and parameters as `graph` gives
    /// an instance's, without `address` (there is no instance).
    pub fn modules(&self, registry: &Registry) -> Value {
        let unaddressed = |mut value: Value| {
            if let Value::Object(fields) = &mut value {
                fields.remove("address");
            }
            value
        };
        let primitives: Vec<Value> = registry
            .iter()
            .map(|t| {
                let d = &t.descriptor;
                let mut primitive = json!({
                    "module": d.type_name,
                    "composite": false,
                    "description": d.doc,
                    "ports": d.ports.iter()
                        .map(|p| unaddressed(port_json(p.name, "", p.direction, Some(Endpoint::Port(p)))))
                        .collect::<Vec<_>>(),
                    "parameters": d.parameters.iter()
                        .map(|p| unaddressed(parameter_json(p, p.name, "")))
                        .collect::<Vec<_>>(),
                });
                // ADR 0027 §1: the palette group, only when the type has one.
                if let (Some(category), Value::Object(fields)) = (d.category, &mut primitive) {
                    fields.insert("category".to_owned(), json!(category));
                }
                primitive
            })
            .collect();
        let walk = self.walk(registry, BTreeMap::new());
        let mut composites: Vec<Value> = self
            .composites
            .iter()
            .map(|def| {
                let (ports, parameters) = walk.interface(def, &|_| String::new());
                let mut entry = json!({
                    "module": def.name,
                    "composite": true,
                    // The file's `[composite] description`, else null.
                    "description": self.description(&def.name),
                    "ports": ports.into_iter().map(unaddressed).collect::<Vec<_>>(),
                    "parameters": parameters.into_iter().map(unaddressed).collect::<Vec<_>>(),
                    // The add browser's schematic preview: its inside as `graph` gives a composite's.
                    "inside": self.composite_level(&walk, def, ""),
                });
                // ADR 0027 §1: its category, only when the file names one.
                if let (Some(category), Value::Object(fields)) =
                    (self.category(&def.name), &mut entry)
                {
                    fields.insert("category".to_owned(), json!(category));
                }
                entry
            })
            .collect();
        // ADR 0027 §6: the palette groups by category and library. The
        // built-in library always loads (signal-lib's tests), so a failure
        // here only leaves it out.
        composites.extend(library_entries(registry, true).unwrap_or_default());
        json!({"primitives": primitives, "composites": composites})
    }

    fn walk<'a>(
        &'a self,
        registry: &'a Registry,
        converters: BTreeMap<(String, String), Value>,
    ) -> Walk<'a> {
        let mut instances = BTreeMap::new();
        let (mut outputs, mut inputs, mut parameters) =
            (ByExport::new(), ByExport::new(), ByExport::new());
        let library = self.library.iter().map(|l| &l.def);
        for def in std::iter::once(&self.root)
            .chain(&self.composites)
            .chain(library.clone())
        {
            let name = def.name.as_str();
            let by_name = def.instances.iter().map(|i| (i.name.as_str(), i));
            instances.insert(name, by_name.collect());
            for e in &def.exports.outputs {
                let all: &mut Vec<_> = outputs.entry((name, e.name.as_str())).or_default();
                all.push(e.from.as_str());
            }
            for e in &def.exports.inputs {
                let all: &mut Vec<_> = inputs.entry((name, e.name.as_str())).or_default();
                all.push(e.to.as_slice());
            }
            for e in &def.exports.parameters {
                let all: &mut Vec<_> = parameters.entry((name, e.name.as_str())).or_default();
                all.push(e.to.as_slice());
            }
        }
        Walk {
            project: self,
            registry,
            by_name: self
                .composites
                .iter()
                .chain(library)
                .map(|c| (c.name.as_str(), c))
                .collect(),
            instances,
            outputs,
            inputs,
            parameters,
            converters,
            slots: BTreeMap::new(),
        }
    }

    /// Whether the project or one of its composites has a slot.
    pub(crate) fn has_slots(&self) -> bool {
        std::iter::once(&self.spans.root)
            .chain(self.spans.composites.values())
            .any(|d| d.contents.values().any(|c| c.slot.is_some()))
    }

    /// The spans of definition `def` (the root's or a composite's).
    fn def_spans(&self, def: &CompositeDef) -> Option<&DefSpans> {
        if def.name.is_empty() {
            Some(&self.spans.root)
        } else {
            self.spans.composites.get(&def.name)
        }
    }

    /// `def`'s entries in source order: its instances, and its empty slots
    /// where the file writes them.
    fn written_order<'a>(&'a self, def: &'a CompositeDef) -> Vec<Written<'a>> {
        let mut entries: Vec<(usize, Written<'a>)> = self
            .in_source_order(def)
            .into_iter()
            .enumerate()
            .map(|(i, instance)| {
                let at = self
                    .def_spans(def)
                    .and_then(|s| s.instances.get(&instance.name))
                    .map_or(usize::MAX - 1_000_000 + i, |s| s.key.start);
                (at, Written::Instance(instance))
            })
            .collect();
        if let Some(spans) = self.def_spans(def) {
            for (name, content) in &spans.contents {
                let planned = def.instances.iter().any(|i| i.name == *name);
                if content.slot.is_some() && !planned {
                    let at = spans
                        .instances
                        .get(name)
                        .map_or(usize::MAX, |s| s.key.start);
                    entries.push((at, Written::EmptySlot(name.as_str())));
                }
            }
        }
        entries.sort_by_key(|(at, _)| *at);
        entries.into_iter().map(|(_, w)| w).collect()
    }

    /// `def`'s instances in source order.
    fn in_source_order<'a>(&self, def: &'a CompositeDef) -> Vec<&'a signal_graph::InstanceDef> {
        let spans = if def.name.is_empty() {
            Some(&self.spans.root)
        } else {
            self.spans.composites.get(&def.name)
        };
        let mut instances: Vec<_> = def.instances.iter().collect();
        instances.sort_by_key(|i| {
            spans
                .and_then(|s| s.instances.get(&i.name))
                .map_or(usize::MAX, |s| s.key.start)
        });
        instances
    }
}

/// The exported parameters, in declared order, of the built-in library
/// composite `module` (`lib.<name>`), or `None` when the library has none
/// of that name: what a patch's `instance.add` needs to write an instance of
/// it (`patch::Context::module`).
pub fn library_parameters(module: &str) -> Option<Vec<String>> {
    let name = module.strip_prefix(LIBRARY_PREFIX)?;
    Library::builtin().get(name)?;
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"library\"\n\n[instances]\nx = {{ module = \"{module}\" }}\n"
    );
    let project = Project::from_sources(vec![SourceFile {
        path: PROJECT_FILE.to_owned(),
        text,
    }])
    .ok()?;
    let found = project.library.iter().find(|l| l.name == module)?;
    Some(
        found
            .def
            .exports
            .parameters
            .iter()
            .map(|e| e.name.clone())
            .collect(),
    )
}

/// The built-in library's composites as `signal modules` lists them (ADR 0027
/// §4), in name order: each one's `module` (`lib.<name>`), `library: true`,
/// its `category`, `description` and content `hash`, and its ports and
/// parameters as an instance of it shows them (the interface `graph` gives an
/// instance), without `address`.
pub fn library_modules(registry: &Registry) -> Result<Vec<Value>, Vec<Diagnostic>> {
    library_entries(registry, false)
}

/// A [`Project::modules`] reply without its composites' `inside`: what
/// `modules` gives unless it is asked for `inside: true` (the Add module
/// browser's schematic preview, `docs/ui-protocol.md` "modules").
pub fn modules_without_inside(mut modules: Value) -> Value {
    if let Some(Value::Array(composites)) = modules.get_mut("composites") {
        for entry in composites.iter_mut() {
            if let Some(fields) = entry.as_object_mut() {
                fields.remove("inside");
            }
        }
    }
    modules
}

/// [`library_modules`], each entry with its `inside` (its level as `graph`
/// gives a composite's) when `inside` is set: the UI protocol's `modules`.
fn library_entries(registry: &Registry, inside: bool) -> Result<Vec<Value>, Vec<Diagnostic>> {
    let instances: String = Library::builtin()
        .names()
        .map(|n| format!("{n} = {{ module = \"{LIBRARY_PREFIX}{n}\" }}\n"))
        .collect();
    let text =
        format!("schema_version = 1\n\n[project]\nname = \"library\"\n\n[instances]\n{instances}");
    let project = Project::from_sources(vec![SourceFile {
        path: PROJECT_FILE.to_owned(),
        text,
    }])?;
    let walk = project.walk(registry, BTreeMap::new());
    let unaddressed = |mut value: Value| {
        if let Value::Object(fields) = &mut value {
            fields.remove("address");
        }
        value
    };
    Ok(project
        .library
        .iter()
        .map(|l| {
            let (ports, mut parameters) = walk.interface(&l.def, &|_| String::new());
            // The default is the value the composite sets, where it sets one.
            for parameter in &mut parameters {
                let set = parameter["name"]
                    .as_str()
                    .and_then(|name| l.def.exports.parameters.iter().find(|e| e.name == name))
                    .and_then(|export| export.to.first())
                    .and_then(|target| target.split_once('.'))
                    .and_then(|(instance, key)| {
                        let found = l.def.instances.iter().find(|i| i.name == instance)?;
                        found.parameters.get(key).copied()
                    });
                if let (Some(value), Value::Object(fields)) = (set, parameter) {
                    fields.insert("default".to_owned(), json!(value));
                }
            }
            let mut entry = json!({
                "module": l.name,
                "composite": true,
                "library": true,
                "category": project.category(&l.name),
                "description": project.description(&l.name),
                "hash": l.hash,
                "ports": ports.into_iter().map(unaddressed).collect::<Vec<_>>(),
                "parameters": parameters.into_iter().map(unaddressed).collect::<Vec<_>>(),
            });
            // M2.10-4: a validation reference names its native type, only
            // when it is one (the palette groups these apart, last).
            let reference = l
                .name
                .strip_prefix(LIBRARY_PREFIX)
                .and_then(Library::reference_for);
            if let (Some(native), Value::Object(fields)) = (reference, &mut entry) {
                fields.insert("reference_for".to_owned(), json!(native));
            }
            if let (true, Value::Object(fields)) = (inside, &mut entry) {
                fields.insert(
                    "inside".to_owned(),
                    project.composite_level(&walk, &l.def, ""),
                );
            }
            entry
        })
        .collect())
}

impl Walk<'_> {
    fn reach(&self, def: &CompositeDef, prefix: &str, first: &mut BTreeMap<String, String>) {
        for instance in self.project.in_source_order(def) {
            if let Some(inner) = self.by_name.get(instance.module.as_str()) {
                let address = format!("{prefix}/{}", instance.name);
                if !first.contains_key(&inner.name) {
                    first.insert(inner.name.clone(), address.clone());
                    self.reach(inner, &address, first);
                }
            }
        }
    }

    /// A level: its instances and connections. `prefix` is the address of an
    /// instance of it (empty for the top level); `relative` names its parts
    /// without the leading `/`.
    fn level(&self, def: &CompositeDef, prefix: &str, relative: bool) -> Value {
        let lead = if relative { "" } else { "/" };
        let instances: Vec<Value> = self
            .project
            .written_order(def)
            .into_iter()
            .map(|written| {
                let instance = match written {
                    Written::Instance(instance) => instance,
                    Written::EmptySlot(name) => return self.slot_box(def, name, None, lead),
                };
                if self.project.def_spans(def).is_some_and(|s| {
                    s.contents
                        .get(&instance.name)
                        .is_some_and(|c| c.slot.is_some())
                }) {
                    return self.slot_box(def, &instance.name, Some(instance), lead);
                }
                let name = &instance.name;
                let at = |port: &str| format!("{lead}{name}.{port}");
                let (composite, ports, parameters) =
                    match self.by_name.get(instance.module.as_str()) {
                        Some(inner) => {
                            let (ports, parameters) = self.interface(inner, &at);
                            (true, ports, parameters)
                        }
                        None => match self.registry.get(&instance.module) {
                            Some(module) => (
                                false,
                                module
                                    .descriptor
                                    .ports
                                    .iter()
                                    .map(|p| {
                                        port_json(
                                            p.name,
                                            &at(p.name),
                                            p.direction,
                                            Some(Endpoint::Port(p)),
                                        )
                                    })
                                    .collect(),
                                module
                                    .descriptor
                                    .parameters
                                    .iter()
                                    .map(|p| parameter_json(p, p.name, &at(p.name)))
                                    .collect(),
                            ),
                            None => (false, Vec::new(), Vec::new()),
                        },
                    };
                json!({
                    "name": name,
                    "address": format!("{lead}{name}"),
                    "module": instance.module,
                    "composite": composite,
                    "ports": ports,
                    "parameters": parameters,
                })
            })
            .collect();
        let connections: Vec<Value> = def
            .connections
            .iter()
            .map(|c| {
                let mut converters: Vec<Value> = Vec::new();
                for from in self.primitives(def, prefix, &c.from, true) {
                    for to in self.primitives(def, prefix, &c.to, false) {
                        if let Some(entry) = self.converters.get(&(from.clone(), to))
                            && !converters.contains(entry)
                        {
                            converters.push(entry.clone());
                        }
                    }
                }
                json!({
                    "from": format!("{lead}{}", c.from),
                    "to": format!("{lead}{}", c.to),
                    "scale": c.scale,
                    "converters": converters,
                })
            })
            .collect();
        json!({"instances": instances, "connections": connections})
    }

    /// ADR 0037 §7: a slot's box as `graph` gives it. Its ports and
    /// parameters are its interface's, loaded or not (a matched parameter
    /// with the range and default of the content's), its `module` the
    /// content as written (`""` when empty), and `slot` what the plan build
    /// decided: `{interface, from, load, project, matches, unmatched,
    /// unheard, dropped}`.
    fn slot_box(
        &self,
        def: &CompositeDef,
        name: &str,
        instance: Option<&InstanceDef>,
        lead: &str,
    ) -> Value {
        let spans = self.project.def_spans(def);
        let written = spans
            .and_then(|s| s.contents.get(name))
            .and_then(|c| c.slot.as_ref());
        let key = if def.name.is_empty() {
            format!("/{name}")
        } else {
            format!("{}/{name}", def.name)
        };
        let resolution = self.slots.get(&key);
        let interface = resolution
            .map(|r| r.interface.clone())
            .or_else(|| written.and_then(|w| w.interface.clone()))
            .unwrap_or_default();
        let load = written.and_then(|w| w.load.as_ref().map(|(l, _)| l.clone()));
        let module = instance.map_or(String::new(), |i| i.module.clone());
        let at = |port: &str| format!("{lead}{name}.{port}");
        let matched = |port: &str| -> Option<&str> {
            resolution?
                .matches
                .iter()
                .find(|m| m.port == port)
                .and_then(|m| m.content.as_deref())
        };
        // The content's own parameters, to give a matched one its range.
        let content_parameters: Vec<Value> = match self.by_name.get(module.as_str()) {
            Some(inner) => self.interface(inner, &|_| String::new()).1,
            None => self
                .registry
                .get(&module)
                .map(|m| {
                    m.descriptor
                        .parameters
                        .iter()
                        .map(|p| parameter_json(p, p.name, ""))
                        .collect()
                })
                .unwrap_or_default(),
        };
        let mut ports = Vec::new();
        let mut parameters = Vec::new();
        for port in &interface.ports {
            let mut value = json!({
                "name": port.name,
                "address": at(&port.name),
                "shape": serde_json::to_value(port).ok().and_then(|v| v.get("shape").cloned()),
                "rate": port.rate,
                "kind": port.kind,
                "unit": port.unit,
                "range": port.range.map(|(min, max)| json!({"min": min, "max": max})),
            });
            if let (Some(group), Value::Object(fields)) = (&port.group, &mut value) {
                fields.insert("group".to_owned(), json!(group));
            }
            match port.role {
                Role::Input | Role::Output => {
                    if let Value::Object(fields) = &mut value {
                        let direction = if port.role == Role::Output {
                            "out"
                        } else {
                            "in"
                        };
                        fields.insert("direction".to_owned(), json!(direction));
                    }
                    ports.push(value);
                }
                Role::Parameter => {
                    let found = matched(&port.name).and_then(|content| {
                        content_parameters
                            .iter()
                            .find(|p| p["name"].as_str() == Some(content))
                            .cloned()
                    });
                    let value = match found {
                        Some(mut p) => {
                            if let Value::Object(fields) = &mut p {
                                fields.insert("name".to_owned(), json!(port.name));
                                fields.insert("address".to_owned(), json!(at(&port.name)));
                                if port.unit.is_some() {
                                    fields.insert("unit".to_owned(), json!(port.unit));
                                }
                                if let Some(group) = &port.group {
                                    fields.insert("group".to_owned(), json!(group));
                                }
                            }
                            p
                        }
                        None => {
                            if let Value::Object(fields) = &mut value {
                                fields.remove("shape");
                                fields.insert("default".to_owned(), Value::Null);
                                fields.insert("combine".to_owned(), Value::Null);
                            }
                            value
                        }
                    };
                    parameters.push(value);
                }
            }
        }
        let unmatched: Vec<Value> = resolution
            .map(|r| {
                r.matches
                    .iter()
                    .filter(|m| m.content.is_none())
                    .map(|m| json!({"port": m.port, "reason": m.reason}))
                    .collect()
            })
            .unwrap_or_default();
        json!({
            "name": name,
            "address": format!("{lead}{name}"),
            "module": load.clone().unwrap_or_default(),
            "composite": instance.is_some() && self.by_name.contains_key(module.as_str()),
            "ports": ports,
            "parameters": parameters,
            "slot": {
                "interface": interface,
                "from": interface.from,
                "load": load,
                "project": load.as_deref().is_some_and(crate::is_project_path),
                "map": written.map(|w| {
                    w.map
                        .iter()
                        .map(|(k, _, v, _)| {
                            let value = match v {
                                crate::model::MapValue::Name(name) => json!(name),
                                crate::model::MapValue::Unmatched => json!(false),
                            };
                            (k.clone(), value)
                        })
                        .collect::<Map<String, Value>>()
                }),
                "matches": resolution.map(|r| r.matches.clone()).unwrap_or_default(),
                "unmatched": unmatched,
                "unheard": resolution.map(|r| r.unheard.clone()).unwrap_or_default(),
                "dropped": resolution.map(|r| r.dropped.clone()).unwrap_or_default(),
                "inside": self.inside_of(&def.name, name, load.as_deref()).json(),
            },
        })
    }

    /// A composite's ports and parameters as an instance of it shows them:
    /// its exports, with the shape of what they reach, at `at(name)`.
    fn interface(
        &self,
        inner: &CompositeDef,
        at: &dyn Fn(&str) -> String,
    ) -> (Vec<Value>, Vec<Value>) {
        let mut ports = Vec::new();
        for export in &inner.exports.inputs {
            let found = export.to.first().and_then(|t| self.endpoint(inner, t));
            ports.push(port_json(
                &export.name,
                &at(&export.name),
                Direction::In,
                found,
            ));
        }
        for export in &inner.exports.outputs {
            let found = self.endpoint(inner, &export.from);
            ports.push(port_json(
                &export.name,
                &at(&export.name),
                Direction::Out,
                found,
            ));
        }
        let parameters = inner
            .exports
            .parameters
            .iter()
            .filter_map(|export| {
                let target = export.to.first()?;
                match self.endpoint(inner, target)? {
                    Endpoint::Parameter(p) => {
                        // ADR 0042: the export's own range, else an inner
                        // export's on the way.
                        let mut p = *p;
                        if let Some(range) = self.export_range(inner, &export.name) {
                            p.range = range;
                        }
                        let mut value = parameter_json(&p, &export.name, &at(&export.name));
                        // ADR 0043: its own value set, else the one its
                        // targets share, kept to its own range, read as they
                        // read it; none when they share none (as the plan).
                        if let Value::Object(fields) = &mut value {
                            fields.remove("values");
                            fields.remove("snap");
                            if let Some((values, snap)) = self.export_values(inner, &export.name) {
                                fields.insert("values".to_owned(), values);
                                if snap == Snap::Positive {
                                    fields.insert("snap".to_owned(), json!(snap));
                                }
                            }
                        }
                        Some(value)
                    }
                    Endpoint::Port(_) => None,
                }
            })
            .collect();
        (ports, parameters)
    }

    fn exported_inputs(&self, def: &CompositeDef) -> Value {
        Value::Array(
            def.exports
                .inputs
                .iter()
                .map(|e| {
                    let found = e.to.first().and_then(|t| self.endpoint(def, t));
                    with_targets(
                        port_json(&e.name, &e.name, Direction::In, found),
                        "to",
                        json!(e.to),
                    )
                })
                .collect(),
        )
    }

    fn exported_outputs(&self, def: &CompositeDef) -> Value {
        Value::Array(
            def.exports
                .outputs
                .iter()
                .map(|e| {
                    let found = self.endpoint(def, &e.from);
                    with_targets(
                        port_json(&e.name, &e.name, Direction::Out, found),
                        "from",
                        json!(e.from),
                    )
                })
                .collect(),
        )
    }

    fn exported_parameters(&self, def: &CompositeDef) -> Value {
        Value::Array(
            def.exports
                .parameters
                .iter()
                .map(|e| {
                    let mut value = json!({"name": e.name, "to": e.to});
                    if let Value::Object(fields) = &mut value {
                        if let Some(range) = e.range {
                            fields.insert("range".to_owned(), json!(range));
                        }
                        if let Some(values) = &e.values {
                            fields.insert("values".to_owned(), json!(values));
                        }
                    }
                    value
                })
                .collect(),
        )
    }

    /// The primitive port or parameter that endpoint `text` of `def` reaches
    /// (an export's first target, recursively).
    fn endpoint(&self, def: &CompositeDef, text: &str) -> Option<Endpoint> {
        let (instance, port) = text.split_once('.')?;
        let instance = self.instance(def, instance)?;
        if let Some(inner) = self.by_name.get(instance.module.as_str()) {
            let key = (inner.name.as_str(), port);
            let target = self
                .outputs
                .get(&key)
                .and_then(|all| all.first().copied())
                .or_else(|| first_target(&self.inputs, key))
                .or_else(|| first_target(&self.parameters, key))?;
            return self.endpoint(inner, target);
        }
        let descriptor = &self.registry.get(&instance.module)?.descriptor;
        descriptor
            .ports
            .iter()
            .find(|p| p.name == port)
            .map(Endpoint::Port)
            .or_else(|| {
                descriptor
                    .parameters
                    .iter()
                    .find(|p| p.name == port)
                    .map(Endpoint::Parameter)
            })
    }

    /// The flattened primitive ports endpoint `text` of `def` reaches, `def`
    /// instantiated at `prefix`: one for a source (`out`), every target of a
    /// fanned-out export for a target.
    fn primitives(&self, def: &CompositeDef, prefix: &str, text: &str, out: bool) -> Vec<String> {
        let Some((name, port)) = text.split_once('.') else {
            return Vec::new();
        };
        let Some(instance) = self.instance(def, name) else {
            return Vec::new();
        };
        let address = format!("{prefix}/{name}");
        let Some(inner) = self.by_name.get(instance.module.as_str()) else {
            return vec![format!("{address}.{port}")];
        };
        let key = (inner.name.as_str(), port);
        if out {
            self.outputs
                .get(&key)
                .into_iter()
                .flatten()
                .flat_map(|from| self.primitives(inner, &address, from, true))
                .collect()
        } else {
            self.inputs
                .get(&key)
                .into_iter()
                .chain(self.parameters.get(&key))
                .flatten()
                .flat_map(|targets| targets.iter())
                .flat_map(|t| self.primitives(inner, &address, t, false))
                .collect()
        }
    }

    /// ADR 0042: the range exported parameter `name` of `def` declares, else
    /// the intersection of every target's (a primitive parameter's, or an
    /// inner export's, recursively), as the plan and the panel take it.
    fn export_range(&self, def: &CompositeDef, name: &str) -> Option<signal_core::Range> {
        let export = def.exports.parameters.iter().find(|e| e.name == name)?;
        if export.range.is_some() {
            return export.range;
        }
        let mut shared: Option<signal_core::Range> = None;
        for target in &export.to {
            let Some(range) = self.target_range(def, target) else {
                continue;
            };
            shared = Some(match shared {
                Some(a) => signal_core::Range {
                    min: a.min.max(range.min),
                    max: a.max.min(range.max),
                },
                None => range,
            });
        }
        shared
    }

    /// The range of target `text` (`inst.param`) of `def`.
    fn target_range(&self, def: &CompositeDef, text: &str) -> Option<signal_core::Range> {
        let (instance, port) = text.split_once('.')?;
        let instance = self.instance(def, instance)?;
        if let Some(inner) = self.by_name.get(instance.module.as_str()) {
            return self.export_range(inner, port);
        }
        let descriptor = &self.registry.get(&instance.module)?.descriptor;
        descriptor
            .parameters
            .iter()
            .find(|p| p.name == port)
            .map(|p| p.range)
    }

    /// ADR 0043: the value set of exported parameter `name` of `def`, as
    /// the plan takes it: the one it declares, else the one all its targets
    /// share (a primitive's, or an inner export's), kept to its own range;
    /// with the rule it is read by ([`Snap::Positive`] when every target
    /// reads it so). As `[{value, label}]`.
    fn export_values(&self, def: &CompositeDef, name: &str) -> Option<(Value, Snap)> {
        let (set, snap) = self.export_set(def, name)?;
        let values: Vec<Value> = set
            .iter()
            .map(|(value, label)| json!({"value": value, "label": label}))
            .collect();
        Some((Value::Array(values), snap))
    }

    fn export_set(&self, def: &CompositeDef, name: &str) -> Option<(Vec<(f64, String)>, Snap)> {
        let export = def.exports.parameters.iter().find(|e| e.name == name)?;
        let mut sets = Vec::new();
        let mut positive = true;
        for target in &export.to {
            let (set, snap) = self.target_set(def, target);
            positive &= snap == Snap::Positive;
            sets.push(set);
        }
        let snap = if positive && !sets.is_empty() {
            Snap::Positive
        } else {
            Snap::Nearest
        };
        if let Some(values) = &export.values {
            let set = values.iter().map(|v| (v.value, v.label.clone())).collect();
            return Some((set, snap));
        }
        let first = sets.first()?.clone()?;
        if !sets.iter().all(|s| s.as_ref() == Some(&first)) {
            return None;
        }
        let kept: Vec<(f64, String)> = match export.range {
            Some(range) => first
                .into_iter()
                .filter(|(v, _)| range.min <= *v && *v <= range.max)
                .collect(),
            None => first,
        };
        (!kept.is_empty()).then_some((kept, snap))
    }

    /// The value set of target `text` (`inst.param`) of `def`, and its rule.
    fn target_set(&self, def: &CompositeDef, text: &str) -> (Option<Vec<(f64, String)>>, Snap) {
        let Some((instance, port)) = text.split_once('.') else {
            return (None, Snap::Nearest);
        };
        let Some(instance) = self.instance(def, instance) else {
            return (None, Snap::Nearest);
        };
        if let Some(inner) = self.by_name.get(instance.module.as_str()) {
            return match self.export_set(inner, port) {
                Some((set, snap)) => (Some(set), snap),
                None => (None, Snap::Nearest),
            };
        }
        let parameter = self
            .registry
            .get(&instance.module)
            .and_then(|m| m.descriptor.parameters.iter().find(|p| p.name == port));
        match parameter {
            Some(p) => (
                p.values.map(|values| {
                    values
                        .iter()
                        .map(|v| (v.value, v.label.to_owned()))
                        .collect()
                }),
                p.snap,
            ),
            None => (None, Snap::Nearest),
        }
    }

    /// Instance `name` of `def`.
    fn instance(&self, def: &CompositeDef, name: &str) -> Option<&'_ InstanceDef> {
        self.instances.get(def.name.as_str())?.get(name).copied()
    }
}

/// The first target of the first export `key` in `all`.
fn first_target<'a>(all: &ByExport<'a, &'a [String]>, key: (&str, &str)) -> Option<&'a str> {
    all.get(&key)?.first()?.first().map(String::as_str)
}

/// A port as `describe` gives it (`name`, `direction`, `shape`, `rate`,
/// `kind`, `unit`, `range`) with its `address`; an export takes the shape of
/// what it reaches, and one that reaches a parameter is a signal input.
fn port_json(name: &str, address: &str, direction: Direction, found: Option<Endpoint>) -> Value {
    let mut value = match found {
        Some(Endpoint::Port(p)) => serde_json::to_value(p).unwrap_or(Value::Null),
        Some(Endpoint::Parameter(p)) => json!({
            "shape": "signal", "rate": p.rate, "kind": p.kind, "unit": p.unit, "range": p.range,
        }),
        None => json!({"shape": null, "rate": null, "kind": null, "unit": null, "range": null}),
    };
    if let Value::Object(fields) = &mut value {
        fields.insert("name".to_owned(), json!(name));
        fields.insert("direction".to_owned(), json!(direction));
        fields.insert("address".to_owned(), json!(address));
    }
    value
}

fn parameter_json(p: &ParameterDescriptor, name: &str, address: &str) -> Value {
    let mut value = serde_json::to_value(p).unwrap_or(Value::Null);
    if let Value::Object(fields) = &mut value {
        fields.insert("name".to_owned(), json!(name));
        fields.insert("address".to_owned(), json!(address));
    }
    value
}

fn with_targets(mut value: Value, key: &str, targets: Value) -> Value {
    if let Value::Object(fields) = &mut value {
        fields.insert(key.to_owned(), targets);
    }
    value
}

/// ADR 0037, editable insides: what a slot's inside is in `graph`.
enum InsideOf {
    /// Its own inside, this composite (a level of `graph.composites`).
    Own(String),
    /// Not its own yet: shown read-only at `key` (`graph.insides`), made
    /// on the first edit as `name` (`make`: `empty`, `copy` or `wrap`);
    /// `source` is `None` for a project composite (its definition is
    /// shown).
    Preview {
        key: String,
        make: &'static str,
        name: String,
        source: Option<crate::inside::Source>,
    },
    /// No inside to show: why.
    Closed(String),
}

impl InsideOf {
    /// The `inside` field of a slot box: `{level, own, make?, name?, reason?}`.
    fn json(&self) -> Value {
        match self {
            InsideOf::Own(name) => json!({"level": name, "own": true, "name": name}),
            InsideOf::Preview {
                key, make, name, ..
            } => json!({"level": key, "own": false, "make": make, "name": name}),
            InsideOf::Closed(reason) => json!({"level": null, "own": false, "reason": reason}),
        }
    }
}

impl Walk<'_> {
    /// What slot `slot` at `level` (loading `load`) has as an inside.
    fn inside_of(&self, level: &str, slot: &str, load: Option<&str>) -> InsideOf {
        use crate::inside::{self, Source};
        let files = &self.project.files;
        let key = inside::preview_key(level, slot);
        let name = || inside::default_name(level, slot, &|n| inside::taken(files, n));
        let Some(load) = load else {
            return InsideOf::Preview {
                key,
                make: "empty",
                name: name(),
                source: Some(Source::Empty),
            };
        };
        if crate::is_project_path(load) {
            return InsideOf::Closed(
                "a project in a slot is edited in its own folder: a slot's own copy of a project is not made".to_owned(),
            );
        }
        if let Some(text) = inside::composite_file(files, load) {
            if inside::owner_of(&text).as_deref() == Some(inside::owner(level, slot).as_str()) {
                return InsideOf::Own(load.to_owned());
            }
            return InsideOf::Preview {
                key,
                make: "copy",
                name: name(),
                source: None,
            };
        }
        match inside::source(files, load, self.registry) {
            Some(source @ Source::Composite(_)) => InsideOf::Preview {
                key,
                make: "copy",
                name: name(),
                source: Some(source),
            },
            Some(source @ Source::Primitive { .. }) => InsideOf::Preview {
                key,
                make: "wrap",
                name: name(),
                source: Some(source),
            },
            _ => InsideOf::Closed(format!("`{load}` names no module")),
        }
    }

    /// The read-only insides of the slots that have none of their own yet,
    /// by key (`graph.insides`), each with the slot's interface on its
    /// frame; a slot's own inside in `composites` gets that frame too.
    fn insides(&self, composites: &mut Map<String, Value>) -> Map<String, Value> {
        let mut previews = Map::new();
        for (key, resolution) in &self.slots {
            let Some((level, slot)) = key.rsplit_once('/') else {
                continue;
            };
            if !level.is_empty() && !composites.contains_key(level) {
                continue;
            }
            let matched = |port: &str| {
                resolution
                    .matches
                    .iter()
                    .find(|m| m.port == port)
                    .and_then(|m| m.content.clone())
            };
            match self.inside_of(level, slot, resolution.load.as_deref()) {
                InsideOf::Own(name) => {
                    if let Some(inside) = composites.get_mut(&name) {
                        slot_frame(inside, &resolution.interface, &matched, key);
                    }
                }
                InsideOf::Preview {
                    key: at,
                    name,
                    source,
                    ..
                } => {
                    let inside = match &source {
                        None => resolution
                            .load
                            .as_deref()
                            .and_then(|load| self.by_name.get(load))
                            .map(|def| {
                                let prefix = if level.is_empty() {
                                    format!("/{slot}")
                                } else {
                                    String::new()
                                };
                                let mut inside = self.project.composite_level(self, def, &prefix);
                                slot_frame(&mut inside, &resolution.interface, &matched, key);
                                inside
                            }),
                        Some(source) => self.preview(level, slot, &name, source, resolution).map(
                            |mut inside| {
                                // Its exports are named after the slot's ports.
                                slot_frame(
                                    &mut inside,
                                    &resolution.interface,
                                    &|p: &str| Some(p.to_owned()),
                                    key,
                                );
                                inside
                            },
                        ),
                    };
                    if let Some(inside) = inside {
                        previews.insert(at, inside);
                    }
                }
                InsideOf::Closed(_) => {}
            }
        }
        previews
    }

    /// The level the own inside `name` of slot `slot` at `level` would be,
    /// made from `source` as `slot.inside` makes it.
    fn preview(
        &self,
        level: &str,
        slot: &str,
        name: &str,
        source: &crate::inside::Source,
        resolution: &SlotResolution,
    ) -> Option<Value> {
        use crate::inside;
        let matched: Vec<inside::Matched> = resolution
            .matches
            .iter()
            .filter_map(|m| {
                Some(inside::Matched {
                    port: m.port.clone(),
                    role: m.role,
                    content: m.content.clone()?,
                })
            })
            .collect();
        let names = resolution
            .interface
            .ports
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let text =
            inside::text(name, &inside::owner(level, slot), source, &matched, &names).ok()?;
        let host = format!(
            "schema_version = 1\n\n[project]\nname = \"inside\"\n\n[instances]\nx = {{ module = \"{name}\" }}\n"
        );
        let project = Project::from_sources(vec![
            SourceFile {
                path: PROJECT_FILE.to_owned(),
                text: host,
            },
            SourceFile {
                path: format!("{}/{name}.toml", crate::COMPOSITES),
                text,
            },
        ])
        .ok()?;
        let walk = project.walk(self.registry, BTreeMap::new());
        let def = project.composites.iter().find(|c| c.name == name)?;
        Some(project.composite_level(&walk, def, ""))
    }
}

/// `level` (a composite's, as `graph` gives it) with slot `key`'s interface
/// on its frame (ADR 0037, editable insides): its `inputs`, `outputs` and
/// `parameters` are the interface's ports in its order, each with the
/// targets of the export it matches (`matched`: the content's name of a
/// slot port; `export` when it differs), or none (`unmatched`); and
/// `slot`, the slot's key.
fn slot_frame(
    level: &mut Value,
    interface: &crate::slot::Interface,
    matched: &dyn Fn(&str) -> Option<String>,
    key: &str,
) {
    let Value::Object(fields) = level else {
        return;
    };
    let take = |field: &str| {
        fields
            .get(field)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let lists = [take("inputs"), take("outputs"), take("parameters")];
    let mut framed: [Vec<Value>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for port in &interface.ports {
        let at = match port.role {
            Role::Input => 0,
            Role::Output => 1,
            Role::Parameter => 2,
        };
        let content = matched(&port.name);
        let found = content.as_deref().and_then(|c| {
            lists[at]
                .iter()
                .find(|e| e["name"].as_str() == Some(c))
                .cloned()
        });
        let mut value = found.unwrap_or_else(|| {
            let shape = serde_json::to_value(port)
                .ok()
                .and_then(|v| v.get("shape").cloned());
            match port.role {
                Role::Parameter => json!({"name": port.name, "to": [], "unmatched": true}),
                Role::Input => json!({
                    "name": port.name, "address": port.name, "direction": "in", "shape": shape,
                    "rate": port.rate, "kind": port.kind, "unit": port.unit, "range": null,
                    "to": [], "unmatched": true,
                }),
                Role::Output => json!({
                    "name": port.name, "address": port.name, "direction": "out", "shape": shape,
                    "rate": port.rate, "kind": port.kind, "unit": port.unit, "range": null,
                    "from": null, "unmatched": true,
                }),
            }
        });
        if let Value::Object(entry) = &mut value {
            entry.insert("name".to_owned(), json!(port.name));
            if let Some(content) = content.filter(|c| *c != port.name)
                && !entry.contains_key("unmatched")
            {
                entry.insert("export".to_owned(), json!(content));
            }
            entry.insert("order".to_owned(), json!(framed[at].len()));
            match &port.group {
                Some(group) => {
                    entry.insert("group".to_owned(), json!(group));
                }
                None => {
                    entry.remove("group");
                }
            }
        }
        framed[at].push(value);
    }
    // The inside's exports no port of the slot reaches: shown after them
    // (`extra`), so they can be seen, unwired and removed with what they name.
    for (at, list) in lists.iter().enumerate() {
        for entry in list {
            let Some(name) = entry["name"].as_str() else {
                continue;
            };
            let reached = interface
                .ports
                .iter()
                .any(|p| matched(&p.name).as_deref() == Some(name));
            if reached || interface.port(name).is_some() {
                continue;
            }
            let mut value = entry.clone();
            if let Value::Object(fields) = &mut value {
                fields.insert("extra".to_owned(), json!(true));
                fields.insert("order".to_owned(), json!(framed[at].len()));
            }
            framed[at].push(value);
        }
    }
    let [inputs, outputs, parameters] = framed;
    fields.insert("inputs".to_owned(), Value::Array(inputs));
    fields.insert("outputs".to_owned(), Value::Array(outputs));
    fields.insert("parameters".to_owned(), Value::Array(parameters));
    fields.insert("slot".to_owned(), json!(key));
}
