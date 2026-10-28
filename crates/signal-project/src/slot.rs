//! Slots (ADR 0037): an instance with its own interface and optional
//! content, resolved here, before the graph sees it, into an ordinary
//! instance of the content (or nothing, when the slot is empty) with the
//! host's wires renamed to the content's matched ports.
//!
//! - [`Interface`]: a resolved interface, its ports and parameters with
//!   their types; [`parse_interface`] reads an `interfaces/<name>.toml`
//!   template, [`resolve`] a slot's `slot` key (a template, then `remove`,
//!   then the inline entries).
//! - [`SlotResolution`]: what the plan build decided for one slot (the match
//!   table, the dropped wires, the unheard outputs), for `describe` and the
//!   UI; the plan itself carries no record of it.
//!
//! The matching rule is exact names plus `map` (ADR 0037 §4); everything
//! about it is decided by the two texts alone.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Serialize;
use signal_core::{PortKind, PortShape, Registry};
use signal_graph::{CompositeDef, ModulePort};
use toml::Spanned;

use crate::Project;
use crate::diagnostic::{Diagnostic, Severity};
use crate::locate::{ContentSpans, DefSpans, Location, SourceMap};
use crate::model::{InterfaceFile, KINDS, MapValue, ParamSpec, PortSpec, ShapeSpec, SlotSpec};

/// The prefix of a library template reference (`lib.deck`).
pub(crate) const LIBRARY_TEMPLATE: &str = "lib.";

/// What a port of an interface is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// An input port.
    Input,
    /// An output port.
    Output,
    /// A parameter.
    Parameter,
}

/// One port or parameter of an interface, with its declared type (ADR 0037
/// §3). A parameter is a signal of kind `generic` at rate 1 unless it says
/// otherwise; its range and default come from the content.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InterfacePort {
    /// Its name.
    pub name: String,
    /// Input, output or parameter.
    pub role: Role,
    /// `signal`, `events` or `{"vector": N}`.
    #[serde(serialize_with = "shape_json")]
    pub shape: PortShape,
    /// Its rate divisor.
    pub rate: u32,
    /// Its kind.
    pub kind: PortKind,
    /// Presentation only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Presentation only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<(f64, f64)>,
    /// Presentation only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

fn shape_json<S: serde::Serializer>(shape: &PortShape, serializer: S) -> Result<S::Ok, S::Error> {
    match shape {
        PortShape::Vector(width) => {
            use serde::ser::SerializeMap;
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry("vector", width)?;
            map.end()
        }
        PortShape::Events => serializer.serialize_str("events"),
        _ => serializer.serialize_str("signal"),
    }
}

/// A resolved interface: its ports and parameters in the order of the
/// template, then of the slot's own entries (inputs, outputs, parameters,
/// each in name order); one namespace.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Interface {
    /// The template it started from, as written (`lib.deck`, `deck`), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The template's description, if it gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its ports and parameters.
    pub ports: Vec<InterfacePort>,
}

impl Interface {
    /// The port or parameter `name`.
    pub fn port(&self, name: &str) -> Option<&InterfacePort> {
        self.ports.iter().find(|p| p.name == name)
    }
}

/// A problem in a file: a byte offset and the message.
pub(crate) type Problem = (usize, String);

/// The `PortKind` of a kind name of [`KINDS`].
pub(crate) fn kind_of(name: &str) -> PortKind {
    match name {
        "audio" => PortKind::Audio,
        "control" => PortKind::Control,
        "gate" => PortKind::Gate,
        "trigger" => PortKind::Trigger,
        "frequency" => PortKind::Frequency,
        "gain" => PortKind::Gain,
        "phase" => PortKind::Phase,
        _ => PortKind::Generic,
    }
}

/// The name of `kind`, as written in an interface.
pub(crate) fn kind_name(kind: PortKind) -> &'static str {
    KINDS
        .iter()
        .copied()
        .find(|name| kind_of(name) == kind && (*name != "generic" || kind == PortKind::Generic))
        .unwrap_or("generic")
}

fn port(name: &str, role: Role, spec: &PortSpec) -> InterfacePort {
    InterfacePort {
        name: name.to_owned(),
        role,
        shape: match spec.shape {
            ShapeSpec::Signal => PortShape::Signal,
            ShapeSpec::Events => PortShape::Events,
            ShapeSpec::Vector(width) => PortShape::Vector(width),
        },
        rate: spec.rate,
        kind: kind_of(&spec.kind),
        unit: spec.unit.clone(),
        range: spec.range,
        group: spec.group.clone(),
    }
}

fn parameter(name: &str, spec: &ParamSpec) -> InterfacePort {
    InterfacePort {
        name: name.to_owned(),
        role: Role::Parameter,
        shape: PortShape::Signal,
        rate: 1,
        kind: PortKind::Generic,
        unit: spec.unit.clone(),
        range: None,
        group: spec.group.clone(),
    }
}

/// The entries of a template or a slot table, in role order, each with its
/// key's span.
fn entries<'s>(
    inputs: &'s BTreeMap<Spanned<String>, Spanned<PortSpec>>,
    outputs: &'s BTreeMap<Spanned<String>, Spanned<PortSpec>>,
    parameters: &'s BTreeMap<Spanned<String>, Spanned<ParamSpec>>,
) -> Vec<(usize, InterfacePort)> {
    let inputs = inputs
        .iter()
        .map(|(k, v)| (k.span().start, port(k.get_ref(), Role::Input, v.get_ref())));
    let outputs = outputs
        .iter()
        .map(|(k, v)| (k.span().start, port(k.get_ref(), Role::Output, v.get_ref())));
    let parameters = parameters
        .iter()
        .map(|(k, v)| (k.span().start, parameter(k.get_ref(), v.get_ref())));
    inputs.chain(outputs).chain(parameters).collect()
}

/// Whether `name` may name a port: `[a-z][a-z0-9_]*`, and not `from` or
/// `remove` (reserved in the `slot` table).
fn port_name_problem(name: &str) -> Option<String> {
    if !crate::valid_name(name) {
        Some(format!(
            "invalid port name `{name}`; names use a-z, 0-9, _ and start with a-z"
        ))
    } else if name == "from" || name == "remove" {
        Some(format!(
            "`{name}` is reserved in a slot's interface and cannot name a port"
        ))
    } else {
        None
    }
}

/// Reads an interface template, `interfaces/<stem>.toml` (or a library
/// one), checking that its `name` is `stem`.
pub(crate) fn parse_interface(text: &str, stem: &str) -> Result<Interface, Vec<Problem>> {
    let file: InterfaceFile = toml::from_str(text).map_err(|error| {
        let offset = error.span().map_or(0, |s| s.start);
        vec![(offset, error.message().trim().to_owned())]
    })?;
    let mut problems = Vec::new();
    if *file.schema_version.get_ref() != 1 {
        problems.push((
            file.schema_version.span().start,
            format!(
                "unsupported schema_version {}; this version of Signal reads 1",
                file.schema_version.get_ref()
            ),
        ));
    }
    let name = &file.interface.name;
    if !crate::valid_name(name.get_ref()) {
        problems.push((
            name.span().start,
            format!(
                "invalid interface name `{}`; names use a-z, 0-9, _ and start with a-z",
                name.get_ref()
            ),
        ));
    } else if name.get_ref() != stem {
        problems.push((
            name.span().start,
            format!(
                "interface `{}` must be defined in interfaces/{}.toml",
                name.get_ref(),
                name.get_ref()
            ),
        ));
    }
    let mut interface = Interface {
        description: file.interface.description.clone(),
        ..Interface::default()
    };
    for (offset, port) in entries(&file.inputs, &file.outputs, &file.parameters) {
        if let Some(problem) = port_name_problem(&port.name) {
            problems.push((offset, problem));
        } else if interface.port(&port.name).is_some() {
            problems.push((
                offset,
                format!(
                    "`{}` is declared more than once: inputs, outputs and parameters share one namespace",
                    port.name
                ),
            ));
        } else {
            interface.ports.push(port);
        }
    }
    if problems.is_empty() {
        Ok(interface)
    } else {
        Err(problems)
    }
}

/// The library templates (`lib.voice`, `lib.instrument`, `lib.deck`), read
/// once.
pub(crate) fn library_templates() -> &'static BTreeMap<String, Interface> {
    static TEMPLATES: OnceLock<BTreeMap<String, Interface>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        signal_lib::interfaces()
            .iter()
            .filter_map(|file| {
                // The library's own test reads every template; one that does
                // not read is left out here and reported as unknown.
                let interface = parse_interface(file.text, file.name).ok()?;
                Some((format!("{LIBRARY_TEMPLATE}{}", file.name), interface))
            })
            .collect()
    })
}

/// Resolves a slot's `slot` key at `span` (ADR 0037 §2): its template from
/// `templates` (the project's `interfaces/`, by name) or the library, then
/// `remove`, then its own entries, each adding a port or retyping the
/// template's port of that name.
pub(crate) fn resolve(
    spec: &SlotSpec,
    span: usize,
    templates: &BTreeMap<String, Interface>,
) -> Result<Interface, Vec<Problem>> {
    let mut problems = Vec::new();
    let mut interface = Interface::default();
    if let Some(from) = &spec.from {
        let at = if from.span().end > 0 {
            from.span().start
        } else {
            span
        };
        let name = from.get_ref();
        let found = if name.starts_with(LIBRARY_TEMPLATE) {
            library_templates().get(name)
        } else {
            templates.get(name)
        };
        match found {
            Some(template) => {
                interface.ports = template.ports.clone();
                interface.description = template.description.clone();
            }
            None if name.starts_with(LIBRARY_TEMPLATE) => {
                let known: Vec<&str> = library_templates().keys().map(String::as_str).collect();
                problems.push((
                    at,
                    format!(
                        "unknown library interface `{name}`; library interfaces: {}",
                        known.join(", ")
                    ),
                ));
            }
            None => problems.push((
                at,
                format!("unknown interface `{name}`: interfaces/{name}.toml does not exist"),
            )),
        }
        interface.from = Some(name.clone());
    }
    let declared = entries(&spec.inputs, &spec.outputs, &spec.parameters);
    for removed in &spec.remove {
        let name = removed.get_ref();
        if declared.iter().any(|(_, p)| p.name == *name) {
            problems.push((
                removed.span().start,
                format!("`{name}` is both removed and declared"),
            ));
        } else if spec.from.is_none() {
            problems.push((
                removed.span().start,
                format!("`remove` drops a template's port; this slot has no `from` (`{name}`)"),
            ));
        } else if interface.port(name).is_none() && problems.is_empty() {
            problems.push((
                removed.span().start,
                format!(
                    "`{name}` is not a port or parameter of the interface `{}`",
                    spec.from.as_ref().map_or("", |f| f.get_ref())
                ),
            ));
        }
        interface.ports.retain(|p| p.name != *name);
    }
    let mut own: Vec<String> = Vec::new();
    for (offset, port) in declared {
        if let Some(problem) = port_name_problem(&port.name) {
            problems.push((offset, problem));
            continue;
        }
        if own.contains(&port.name) {
            problems.push((
                offset,
                format!(
                    "`{}` is declared more than once: inputs, outputs and parameters share one namespace",
                    port.name
                ),
            ));
            continue;
        }
        own.push(port.name.clone());
        match interface.ports.iter_mut().find(|p| p.name == port.name) {
            // A retype: the entry replaces the template's port of that name.
            Some(existing) => *existing = port,
            None => interface.ports.push(port),
        }
    }
    if problems.is_empty() {
        Ok(interface)
    } else {
        Err(problems)
    }
}

/// One row of a slot's match table.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Match {
    /// The slot's port or parameter.
    pub port: String,
    /// Input, output or parameter.
    pub role: Role,
    /// The content's port or parameter it matched, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Why it is unmatched, if it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What the plan build decided for one slot (ADR 0037 §5): the UI's side
/// table. The plan carries no record of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SlotResolution {
    /// The slot's address in the written structure: `/deck_a`, or
    /// `<composite>/<instance>` for a slot inside a project composite.
    pub slot: String,
    /// Its resolved interface.
    pub interface: Interface,
    /// Its `load`, as written; `None` for an empty slot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load: Option<String>,
    /// The match table, one row per interface port and parameter (empty
    /// for an empty slot).
    pub matches: Vec<Match>,
    /// The host's wires the plan leaves out, `from -> to` as written.
    pub dropped: Vec<String>,
    /// The content's outputs not in the interface: computed, heard
    /// nowhere.
    pub unheard: Vec<String>,
}

/// A port as the other end of a host wire sees it: its role, shape, rate
/// and kind.
#[derive(Clone, Copy)]
struct Typed {
    output: bool,
    shape: PortShape,
    rate: u32,
    kind: PortKind,
}

fn shape_text(shape: PortShape) -> String {
    match shape {
        PortShape::Signal => "a signal".to_owned(),
        PortShape::Events => "events".to_owned(),
        PortShape::Vector(n) => format!("a vector of {n}"),
        _ => "another shape".to_owned(),
    }
}

/// The written endpoint `instance.port`.
fn split(endpoint: &str) -> Option<(&str, &str)> {
    endpoint.split_once('.')
}

/// The ports of every module the resolution asks about, computed once.
struct Ports<'r> {
    registry: &'r Registry,
    cache: BTreeMap<String, Option<Vec<ModulePort>>>,
}

impl Ports<'_> {
    fn of(&mut self, module: &str, defs: &[CompositeDef]) -> Option<&Vec<ModulePort>> {
        if !self.cache.contains_key(module) {
            let ports = signal_graph::ports_of(module, defs, self.registry).ok();
            self.cache.insert(module.to_owned(), ports);
        }
        self.cache.get(module).and_then(Option::as_ref)
    }
}

/// What resolving one definition's slots gives.
struct Resolved {
    def: CompositeDef,
    spans: DefSpans,
    resolutions: Vec<SlotResolution>,
    errors: Vec<(usize, usize, String)>,
    warnings: Vec<(usize, usize, String)>,
    /// The definition's exports left out because the slot port behind them
    /// is unmatched or the slot empty: an instance's wires on them are left
    /// out too, as a wire on that slot port would be.
    vanished: Vec<String>,
}

/// The exports each definition lost to an unmatched or empty slot.
type Vanished = BTreeMap<String, BTreeSet<String>>;

/// A slot of a project's own files: the definition naming it (`""`: the
/// project root) and its instance name.
pub(crate) type SlotKey = (String, String);

impl Project {
    /// ADR 0037 §6: every loaded slot of the project's own files (its root
    /// and its composites) with what decides "the same content": the
    /// canonical folder of a project it loads, else the module reference
    /// its `load` resolves to. Two values that differ make a Replace.
    pub(crate) fn slot_contents(&self) -> BTreeMap<SlotKey, String> {
        let mut found = BTreeMap::new();
        let defs = std::iter::once(("", &self.root, &self.spans.root)).chain(
            self.composites
                .iter()
                .filter_map(|c| Some((c.name.as_str(), c, self.spans.composites.get(&c.name)?))),
        );
        for (name, def, spans) in defs {
            for (slot, content) in &spans.contents {
                if content.slot.as_ref().is_none_or(|s| s.load.is_none()) {
                    continue;
                }
                let Some(instance) = def.instances.iter().find(|i| i.name == *slot) else {
                    continue;
                };
                let canonical = match &content.included {
                    Some(dir) => format!("project {}", dir.display()),
                    None => format!("module {}", instance.module),
                };
                found.insert((name.to_owned(), slot.clone()), canonical);
            }
        }
        found
    }

    /// This project with the slots `slots` unloaded: their content gone
    /// from the plan, and their files written as `slot.unload` writes
    /// them (`load` and `seed` removed), so the result is a project its
    /// own files say. What a Replace plays between its unload and its load
    /// (ADR 0037 §6). A key naming no slot is skipped.
    pub(crate) fn with_slots_emptied(&self, slots: &[SlotKey]) -> Project {
        let mut project = self.clone();
        for (def, slot) in slots {
            let found = if def.is_empty() {
                Some((&mut project.root, &mut project.spans.root))
            } else {
                project
                    .composites
                    .iter_mut()
                    .find(|c| c.name == *def)
                    .zip(project.spans.composites.get_mut(def))
            };
            let Some((composite, spans)) = found else {
                continue;
            };
            let Some(content) = spans.contents.get_mut(slot) else {
                continue;
            };
            if let Some(written) = &mut content.slot {
                written.load = None;
            }
            content.included = None;
            content.seed = None;
            composite.instances.retain(|i| i.name != *slot);
            let file = spans.file;
            project
                .content
                .entries
                .retain(|e| !(e.def == *def && e.instance == *slot));
            if let Some(source) = project.files.get_mut(file)
                && let Some(text) = crate::patch::unloaded_text(&source.text, slot)
            {
                source.text = text;
            }
        }
        project
    }

    /// ADR 0037 §4–5: every slot of `root` and of `defs` resolved into an
    /// ordinary instance of its content, or into nothing for an empty
    /// slot, with the wires on matched ports renamed to the content's names
    /// and the others left out; `spans` follow the renaming, so graph
    /// errors are still located at the wires as written. Every wire on a
    /// slot is checked against its interface first, loaded or not.
    pub(crate) fn resolve_slots(
        &self,
        root: &mut CompositeDef,
        defs: &mut [CompositeDef],
        spans: &mut SourceMap,
        registry: &Registry,
        errors: &mut Vec<Diagnostic>,
        warnings: &mut Vec<Diagnostic>,
    ) -> Vec<SlotResolution> {
        let has_slots = |spans: &DefSpans| spans.contents.values().any(|c| c.slot.is_some());
        let written_root = root.clone();
        let written: Vec<CompositeDef> = defs.to_vec();
        // Content may itself hold slots: resolve until the definitions the
        // matching reads stop changing (a nesting guard bounds it).
        let mut snapshot: Vec<CompositeDef> = written.clone();
        let mut last: Vec<(Option<usize>, Resolved)> = Vec::new();
        let mut vanished = Vanished::new();
        for _ in 0..=crate::MAX_DEPTH + 1 {
            let touched = |def: &CompositeDef, spans: &DefSpans| {
                has_slots(spans)
                    || def
                        .instances
                        .iter()
                        .any(|i| vanished.contains_key(&i.module))
            };
            let mut ports = Ports {
                registry,
                cache: BTreeMap::new(),
            };
            let mut results = Vec::new();
            if touched(&written_root, &self.spans.root) {
                let resolved = resolve_def(
                    &written_root,
                    &self.spans.root,
                    &snapshot,
                    &mut ports,
                    "",
                    &vanished,
                );
                results.push((None, resolved));
            }
            for (index, def) in written.iter().enumerate() {
                let Some(def_spans) = self.spans.composites.get(&def.name) else {
                    continue;
                };
                if touched(def, def_spans) {
                    let resolved =
                        resolve_def(def, def_spans, &snapshot, &mut ports, &def.name, &vanished);
                    results.push((Some(index), resolved));
                }
            }
            let mut next = written.clone();
            for (index, resolved) in &results {
                if let Some(index) = index
                    && let Some(def) = next.get_mut(*index)
                {
                    *def = resolved.def.clone();
                }
            }
            let lost: Vanished = results
                .iter()
                .filter_map(|(index, r)| {
                    let def = written.get((*index)?)?;
                    (!r.vanished.is_empty())
                        .then(|| (def.name.clone(), r.vanished.iter().cloned().collect()))
                })
                .collect();
            let stable = next == snapshot && lost == vanished;
            snapshot = next;
            vanished = lost;
            last = results;
            if stable {
                break;
            }
        }
        let mut resolutions = Vec::new();
        for (index, resolved) in last {
            for (file, offset, message) in resolved.errors {
                let diagnostic =
                    self.diagnostic(Severity::Error, Location { file, offset }, message);
                if !errors.contains(&diagnostic) {
                    errors.push(diagnostic);
                }
            }
            for (file, offset, message) in resolved.warnings {
                let diagnostic =
                    self.diagnostic(Severity::Warning, Location { file, offset }, message);
                if !warnings.contains(&diagnostic) {
                    warnings.push(diagnostic);
                }
            }
            match index {
                None => {
                    *root = resolved.def;
                    spans.root = resolved.spans;
                }
                Some(index) => {
                    if let Some(def) = defs.get_mut(index) {
                        spans.composites.insert(def.name.clone(), resolved.spans);
                        *def = resolved.def;
                    }
                }
            }
            resolutions.extend(resolved.resolutions.into_iter().map(|mut r| {
                for id in &self.content.ids {
                    r.slot = r.slot.replace(&format!("{id}/"), "");
                }
                r
            }));
        }
        resolutions
    }
}

/// Resolves the slots of the definition `def` (named `name`, `""` for the
/// root) against the definitions `defs`.
fn resolve_def(
    def: &CompositeDef,
    spans: &DefSpans,
    defs: &[CompositeDef],
    ports: &mut Ports<'_>,
    name: &str,
    vanished: &Vanished,
) -> Resolved {
    let file = spans.file;
    let mut out = Resolved {
        def: def.clone(),
        spans: spans.clone(),
        resolutions: Vec::new(),
        errors: Vec::new(),
        warnings: Vec::new(),
        vanished: Vec::new(),
    };
    let interfaces: BTreeMap<&str, &Interface> = spans
        .contents
        .iter()
        .filter_map(|(n, c)| Some((n.as_str(), c.slot.as_ref()?.interface.as_ref()?)))
        .collect();
    let modules: BTreeMap<&str, &str> = def
        .instances
        .iter()
        .map(|i| (i.name.as_str(), i.module.as_str()))
        .collect();
    // The type of a written endpoint that is not on a slot: a module's port.
    let typed_of = |endpoint: &str, ports: &mut Ports<'_>| -> Option<Typed> {
        let (instance, port) = split(endpoint)?;
        if let Some(interface) = interfaces.get(instance) {
            let p = interface.port(port)?;
            return Some(Typed {
                output: p.role == Role::Output,
                shape: p.shape,
                rate: p.rate,
                kind: p.kind,
            });
        }
        let module = modules.get(instance)?;
        let p = ports.of(module, defs)?.iter().find(|p| p.name == port)?;
        Some(Typed {
            output: p.direction == signal_core::Direction::Out,
            shape: p.shape,
            rate: p.rate.get(),
            kind: p.kind,
        })
    };

    // Per slot: its matched names (slot port → content name), and whether
    // it is loaded.
    let mut renames: BTreeMap<&str, (bool, BTreeMap<String, String>)> = BTreeMap::new();
    for (slot_name, content) in &spans.contents {
        let Some(slot) = &content.slot else {
            continue;
        };
        let Some(interface) = &slot.interface else {
            // Its interface did not resolve (reported): nothing of it plans.
            renames.insert(slot_name, (false, BTreeMap::new()));
            continue;
        };
        let instance_spans = spans.instances.get(slot_name);
        let load_at = slot.load.as_ref().map_or(slot.span.start, |(_, s)| s.start);
        let slot_text = if name.is_empty() {
            format!("/{slot_name}")
        } else {
            format!("{name}/{slot_name}")
        };
        let mut resolution = SlotResolution {
            slot: slot_text.clone(),
            interface: interface.clone(),
            load: slot.load.as_ref().map(|(l, _)| l.clone()),
            matches: Vec::new(),
            dropped: Vec::new(),
            unheard: Vec::new(),
        };

        // Values set on the slot name its interface parameters.
        if let Some(instance_spans) = instance_spans {
            for (key, (key_span, _)) in &instance_spans.parameters {
                if interface
                    .port(key)
                    .is_none_or(|p| p.role != Role::Parameter)
                {
                    out.errors.push((
                        file,
                        key_span.start,
                        format!("{slot_text}: `{key}` is not a parameter of the slot's interface"),
                    ));
                }
            }
        }

        // The host's wires on the slot, checked against its interface.
        for connection in &spans.connections {
            for (side, (text, span)) in [(true, &connection.from), (false, &connection.to)] {
                let Some((instance, port)) = split(text) else {
                    continue;
                };
                if instance != slot_name {
                    continue;
                }
                let Some(declared) = interface.port(port) else {
                    out.errors.push((
                        file,
                        span.start,
                        format!(
                            "{slot_text}: `{port}` is not a port of the slot's interface{}",
                            interface
                                .from
                                .as_ref()
                                .map_or(String::new(), |f| format!(" (from `{f}`)"))
                        ),
                    ));
                    continue;
                };
                let wrong = if side {
                    declared.role != Role::Output
                } else {
                    declared.role == Role::Output
                };
                if wrong {
                    out.errors.push((
                        file,
                        span.start,
                        format!(
                            "{slot_text}: `{port}` is an {} of the slot's interface; a wire's `{}` must name an {}",
                            if declared.role == Role::Output { "output" } else { "input" },
                            if side { "from" } else { "to" },
                            if side { "output" } else { "input or a parameter" },
                        ),
                    ));
                    continue;
                }
                let other = if side {
                    &connection.to.0
                } else {
                    &connection.from.0
                };
                let Some(other_type) = typed_of(other, ports) else {
                    // The graph reports an unknown other end.
                    continue;
                };
                if other_type.shape != declared.shape && declared.role != Role::Parameter {
                    out.errors.push((
                        file,
                        span.start,
                        format!(
                            "{slot_text}: `{port}` is declared {} on the slot's interface; `{other}` is {}",
                            shape_text(declared.shape),
                            shape_text(other_type.shape)
                        ),
                    ));
                } else if other_type.rate != declared.rate && declared.role != Role::Parameter {
                    out.errors.push((
                        file,
                        span.start,
                        format!(
                            "{slot_text}: `{port}` is declared at rate divisor {} on the slot's interface; `{other}` runs at {}",
                            declared.rate, other_type.rate
                        ),
                    ));
                } else if !other_type.kind.matches(declared.kind) {
                    out.warnings.push((
                        file,
                        span.start,
                        format!(
                            "{slot_text}: kind mismatch: `{port}` is declared {} on the slot's interface; `{other}` is {}",
                            kind_name(declared.kind),
                            kind_name(other_type.kind)
                        ),
                    ));
                }
                let _ = other_type.output;
            }
        }

        let Some(module) = modules.get(slot_name.as_str()).copied() else {
            // Empty: valid and silent (ADR 0037 §5).
            renames.insert(slot_name, (false, BTreeMap::new()));
            out.resolutions.push(resolution);
            continue;
        };
        let load_text = slot
            .load
            .as_ref()
            .map_or("", |(l, _)| l.as_str())
            .to_owned();
        let Some(content) = ports.of(module, defs).cloned() else {
            out.errors.push((
                file,
                load_at,
                format!("{slot_text}: `{load_text}` does not resolve to a module with ports"),
            ));
            renames.insert(slot_name, (false, BTreeMap::new()));
            continue;
        };

        // `map` keys name slot ports; values name content ports.
        let mut map: BTreeMap<&str, (&MapValue, usize)> = BTreeMap::new();
        for (key, key_span, value, value_span) in &slot.map {
            if interface.port(key).is_none() {
                out.errors.push((
                    file,
                    key_span.start,
                    format!("{slot_text}: `map` key `{key}` is not a port or parameter of the slot's interface"),
                ));
                continue;
            }
            if let MapValue::Name(target) = value
                && !content.iter().any(|c| c.name == *target)
            {
                out.errors.push((
                    file,
                    value_span.start,
                    format!("{slot_text}: `map` value `{target}`: `{load_text}` has no port or parameter `{target}`"),
                ));
                continue;
            }
            map.insert(key.as_str(), (value, value_span.start));
        }

        let mut matched: BTreeMap<String, String> = BTreeMap::new();
        let mut taken_inputs: BTreeMap<String, String> = BTreeMap::new();
        for port in &interface.ports {
            let (candidate, at) = match map.get(port.name.as_str()) {
                Some((MapValue::Unmatched, _)) => {
                    resolution.matches.push(Match {
                        port: port.name.clone(),
                        role: port.role,
                        content: None,
                        reason: Some("`map` sets it to false".to_owned()),
                    });
                    continue;
                }
                Some((MapValue::Name(target), at)) => (target.clone(), *at),
                None => (port.name.clone(), load_at),
            };
            let Some(found) = content.iter().find(|c| c.name == candidate) else {
                resolution.matches.push(Match {
                    port: port.name.clone(),
                    role: port.role,
                    content: None,
                    reason: Some(format!("`{load_text}` has no `{candidate}`")),
                });
                continue;
            };
            let found_role = if found.parameter {
                Role::Parameter
            } else if found.direction == signal_core::Direction::Out {
                Role::Output
            } else {
                Role::Input
            };
            let role_word = |role: Role| match role {
                Role::Input => "an input",
                Role::Output => "an output",
                Role::Parameter => "a parameter",
            };
            let problem = if found_role != port.role {
                Some(format!(
                    "`{load_text}`'s `{candidate}` is {}; the slot's interface declares `{}` {}",
                    role_word(found_role),
                    port.name,
                    role_word(port.role)
                ))
            } else if port.role != Role::Parameter && found.shape != port.shape {
                Some(format!(
                    "`{load_text}` {} `{candidate}` is {}; the slot's interface declares `{}` {}",
                    if port.role == Role::Output {
                        "output"
                    } else {
                        "input"
                    },
                    shape_text(found.shape),
                    port.name,
                    shape_text(port.shape)
                ))
            } else if port.role != Role::Parameter && found.rate.get() != port.rate {
                Some(format!(
                    "`{load_text}` `{candidate}` runs at rate divisor {}; the slot's interface declares `{}` at {}",
                    found.rate.get(),
                    port.name,
                    port.rate
                ))
            } else {
                None
            };
            if let Some(problem) = problem {
                out.errors
                    .push((file, at, format!("{slot_text}: {problem}")));
                resolution.matches.push(Match {
                    port: port.name.clone(),
                    role: port.role,
                    content: None,
                    reason: Some(problem),
                });
                continue;
            }
            if port.role != Role::Output {
                // One export, two meanings: two slot inputs onto one.
                if let Some(other) = taken_inputs.insert(candidate.clone(), port.name.clone()) {
                    out.errors.push((
                        file,
                        at,
                        format!(
                            "{slot_text}: `{other}` and `{}` both match `{load_text}`'s `{candidate}`; an input or parameter of the content takes one",
                            port.name
                        ),
                    ));
                    continue;
                }
            }
            if !found.kind.matches(port.kind) {
                out.warnings.push((
                    file,
                    at,
                    format!(
                        "{slot_text}: kind mismatch: `{load_text}`'s `{candidate}` is {}; the slot's interface declares `{}` {}",
                        kind_name(found.kind),
                        port.name,
                        kind_name(port.kind)
                    ),
                ));
            }
            resolution.matches.push(Match {
                port: port.name.clone(),
                role: port.role,
                content: Some(candidate.clone()),
                reason: None,
            });
            matched.insert(port.name.clone(), candidate);
        }

        let heard: Vec<&String> = interface
            .ports
            .iter()
            .filter(|p| p.role == Role::Output)
            .filter_map(|p| matched.get(&p.name))
            .collect();
        resolution.unheard = content
            .iter()
            .filter(|c| !c.parameter && c.direction == signal_core::Direction::Out)
            .filter(|c| !heard.contains(&&c.name))
            .map(|c| c.name.clone())
            .collect();

        if matched.is_empty() {
            out.warnings.push((
                file,
                load_at,
                format!("{slot_text}: `{load_text}` matches none of the slot's ports"),
            ));
        } else {
            // A wired slot port left unmatched beside an unmatched content
            // port of its direction: perhaps a `map` is missing.
            let used: Vec<&String> = matched.values().collect();
            for port in &interface.ports {
                if matched.contains_key(&port.name) || map.contains_key(port.name.as_str()) {
                    continue;
                }
                let wired = spans.connections.iter().any(|c| {
                    [&c.from.0, &c.to.0]
                        .iter()
                        .any(|e| split(e) == Some((slot_name.as_str(), port.name.as_str())))
                });
                if !wired {
                    continue;
                }
                let suggestion = content.iter().find(|c| {
                    let role = if c.parameter {
                        Role::Parameter
                    } else if c.direction == signal_core::Direction::Out {
                        Role::Output
                    } else {
                        Role::Input
                    };
                    role == port.role && c.kind == port.kind && !used.contains(&&c.name)
                });
                if let Some(suggestion) = suggestion {
                    out.warnings.push((
                        file,
                        load_at,
                        format!(
                            "{slot_text}: `{}` is wired but `{load_text}` has no `{}`; did you mean `map = {{ {} = \"{}\" }}`?",
                            port.name, port.name, port.name, suggestion.name
                        ),
                    ));
                }
            }
        }

        // The instance's values: interface parameters renamed to the
        // content's, then `set` (a module's own parameters outside the
        // interface; a project's `set` is applied to its copy).
        if let Some(instance) = out.def.instances.iter_mut().find(|i| i.name == *slot_name) {
            let written = std::mem::take(&mut instance.parameters);
            for (key, value) in written {
                if let Some(target) = matched.get(&key) {
                    instance.parameters.insert(target.clone(), value);
                }
            }
            if content_is_module(content_spans_of(spans, slot_name)) {
                for (key, span, value) in &spans.contents[slot_name.as_str()].set {
                    let known = content.iter().any(|c| c.parameter && c.name == *key);
                    if !known {
                        out.errors.push((
                            file,
                            span.start,
                            format!("{slot_text}: `set` key `{key}`: `{load_text}` has no parameter `{key}`"),
                        ));
                    } else if matched.values().any(|v| v == key) {
                        out.errors.push((
                            file,
                            span.start,
                            format!("{slot_text}: `set` key `{key}` is a parameter of the slot's interface; set it by its name there"),
                        ));
                    } else {
                        instance.parameters.insert(key.clone(), *value);
                    }
                }
            }
        }
        renames.insert(slot_name, (true, matched));
        out.resolutions.push(resolution);
    }

    // The wires: renamed through the matches, or left out.
    let mut connections = Vec::new();
    let mut connection_spans = Vec::new();
    for (connection, connection_span) in def.connections.iter().zip(&spans.connections) {
        let rename = |endpoint: &str| -> Option<Option<String>> {
            let (instance, port) = split(endpoint)?;
            let (loaded, matched) = renames.get(instance)?;
            if !loaded {
                return Some(None);
            }
            Some(
                matched
                    .get(port)
                    .map(|target| format!("{instance}.{target}")),
            )
        };
        // A wire on an export a child lost to its own slot goes too.
        let lost = |endpoint: &str| {
            split(endpoint).is_some_and(|(instance, port)| {
                modules
                    .get(instance)
                    .and_then(|m| vanished.get(*m))
                    .is_some_and(|names| names.contains(port))
            })
        };
        if lost(&connection.from) || lost(&connection.to) {
            continue;
        }
        let from = rename(&connection.from);
        let to = rename(&connection.to);
        if matches!(from, Some(None)) || matches!(to, Some(None)) {
            let text = format!("{} -> {}", connection.from, connection.to);
            for side in [&connection.from, &connection.to] {
                if let Some((instance, _)) = split(side)
                    && let Some(resolution) = out
                        .resolutions
                        .iter_mut()
                        .find(|r| r.slot.rsplit('/').next() == Some(instance))
                    && !resolution.dropped.contains(&text)
                {
                    resolution.dropped.push(text.clone());
                }
            }
            continue;
        }
        let mut connection = connection.clone();
        let mut connection_span = connection_span.clone();
        if let Some(Some(from)) = from {
            connection.from = from.clone();
            connection_span.from.0 = from;
        }
        if let Some(Some(to)) = to {
            connection.to = to.clone();
            connection_span.to.0 = to;
        }
        connections.push(connection);
        connection_spans.push(connection_span);
    }
    out.def.connections = connections;
    out.spans.connections = connection_spans;
    exports(&mut out, &renames, &interfaces, name);
    out
}

/// ADR 0037 §2: a composite's exports that name a slot's port, checked
/// against the slot's interface and renamed through its match like a wire;
/// a target on an unmatched port or an empty slot is left out, and an
/// export left with no target is dropped (recorded in `vanished`).
fn exports(
    out: &mut Resolved,
    renames: &BTreeMap<&str, (bool, BTreeMap<String, String>)>,
    interfaces: &BTreeMap<&str, &Interface>,
    name: &str,
) {
    let file = out.spans.file;
    // The spans of the exports, by kind, in the definition's order.
    let mut spans: BTreeMap<(u8, String), Vec<(String, usize)>> = BTreeMap::new();
    for export in &out.spans.exports {
        let kind = match export.kind {
            crate::locate::ExportKind::Input => 0,
            crate::locate::ExportKind::Output => 1,
            crate::locate::ExportKind::Parameter => 2,
        };
        spans.insert(
            (kind, export.name.clone()),
            export
                .targets
                .iter()
                .map(|(t, s)| (t.clone(), s.start))
                .collect(),
        );
    }
    let slot_text = |slot: &str| {
        if name.is_empty() {
            format!("/{slot}")
        } else {
            format!("{name}/{slot}")
        }
    };
    let mut errors = Vec::new();
    // `Some(Some(new))`: renamed; `Some(None)`: left out; `None`: not on a slot.
    let mut fate = |kind: u8, export: &str, target: &str| -> Option<Option<String>> {
        let (slot, port) = split(target)?;
        let interface = interfaces.get(slot)?;
        let at = spans
            .get(&(kind, export.to_owned()))
            .and_then(|list| list.iter().find(|(t, _)| t == target))
            .map_or(0, |(_, at)| *at);
        let Some(declared) = interface.port(port) else {
            errors.push((
                file,
                at,
                format!(
                    "{}: `{port}` is not a port of the slot's interface",
                    slot_text(slot)
                ),
            ));
            return Some(None);
        };
        let fits = match kind {
            0 => declared.role != Role::Output,
            1 => declared.role == Role::Output,
            _ => declared.role == Role::Parameter,
        };
        if !fits {
            errors.push((
                file,
                at,
                format!(
                    "{}: export `{export}` names `{port}`, which the slot's interface declares {}",
                    slot_text(slot),
                    match declared.role {
                        Role::Input => "an input",
                        Role::Output => "an output",
                        Role::Parameter => "a parameter",
                    }
                ),
            ));
            return Some(None);
        }
        let (loaded, matched) = renames.get(slot)?;
        if !loaded {
            return Some(None);
        }
        Some(matched.get(port).map(|target| format!("{slot}.{target}")))
    };
    let mut vanished = Vec::new();
    let mut fan = |kind: u8, export: &str, targets: &mut Vec<String>| {
        let mut kept = Vec::new();
        for target in targets.iter() {
            match fate(kind, export, target) {
                None => kept.push(target.clone()),
                Some(Some(renamed)) => kept.push(renamed),
                Some(None) => {}
            }
        }
        if kept.is_empty() && !targets.is_empty() {
            vanished.push(export.to_owned());
        }
        *targets = kept;
    };
    for export in &mut out.def.exports.inputs {
        fan(0, &export.name, &mut export.to);
    }
    for export in &mut out.def.exports.parameters {
        fan(2, &export.name, &mut export.to);
    }
    for export in &mut out.def.exports.outputs {
        let mut one = vec![export.from.clone()];
        fan(1, &export.name, &mut one);
        if let Some(from) = one.pop() {
            export.from = from;
        }
    }
    out.def
        .exports
        .inputs
        .retain(|e| !vanished.contains(&e.name));
    out.def
        .exports
        .parameters
        .retain(|e| !vanished.contains(&e.name));
    out.def
        .exports
        .outputs
        .retain(|e| !vanished.contains(&e.name));
    // The spans follow: renamed texts, dropped exports.
    for export in &mut out.spans.exports {
        let renamed: Option<Vec<String>> = match export.kind {
            crate::locate::ExportKind::Input => out
                .def
                .exports
                .inputs
                .iter()
                .find(|e| e.name == export.name)
                .map(|e| e.to.clone()),
            crate::locate::ExportKind::Parameter => out
                .def
                .exports
                .parameters
                .iter()
                .find(|e| e.name == export.name)
                .map(|e| e.to.clone()),
            crate::locate::ExportKind::Output => out
                .def
                .exports
                .outputs
                .iter()
                .find(|e| e.name == export.name)
                .map(|e| vec![e.from.clone()]),
        };
        if let Some(texts) = renamed {
            let spans: Vec<_> = export.targets.iter().map(|(_, s)| s.clone()).collect();
            export.targets = texts
                .into_iter()
                .zip(spans.into_iter().chain(std::iter::repeat(0..0)))
                .collect();
        }
    }
    out.spans.exports.retain(|e| !vanished.contains(&e.name));
    out.errors.extend(errors);
    out.vanished = vanished;
}

/// The content spans of the slot `name`.
fn content_spans_of<'s>(spans: &'s DefSpans, name: &str) -> Option<&'s ContentSpans> {
    spans.contents.get(name)
}

/// Whether a slot's content is a module (not a project).
fn content_is_module(content: Option<&ContentSpans>) -> bool {
    content
        .and_then(|c| c.slot.as_ref())
        .and_then(|s| s.load.as_ref())
        .is_some_and(|(load, _)| !crate::is_project_path(load))
}

/// A library interface template as `signal modules --interfaces` lists it
/// (ADR 0037 §3).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LibraryInterface {
    /// Its reference, `lib.<name>`.
    pub name: String,
    /// The FNV-1a 64 of its text, as pinned.
    pub hash: String,
    /// Its ports and parameters.
    pub interface: Interface,
}

/// Every library interface template, in name order.
pub fn library_interfaces() -> Vec<LibraryInterface> {
    signal_lib::interfaces()
        .iter()
        .filter_map(|file| {
            let name = format!("{LIBRARY_TEMPLATE}{}", file.name);
            Some(LibraryInterface {
                interface: library_templates().get(&name)?.clone(),
                hash: file.hash(),
                name,
            })
        })
        .collect()
}

/// One port or parameter a slot's content offers ([`ContentNames`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContentPort {
    /// Its name.
    pub name: String,
    /// Input, output or parameter.
    pub role: Role,
    /// `signal`, `events` or `{"vector": N}`.
    #[serde(serialize_with = "shape_json")]
    pub shape: PortShape,
    /// Its rate divisor.
    pub rate: u32,
    /// Its kind.
    pub kind: PortKind,
}

/// What a slot's content offers by name (ADR 0037 §7): its ports and
/// parameters, and the `set` keys it takes. What a Replace keeps of a
/// slot's `map` and `set`, and what the Load dialog counts matches by.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ContentNames {
    /// Its ports and parameters, in its order (a primitive's descriptor
    /// order, a composite's or a project's exports in name order).
    pub ports: Vec<ContentPort>,
    /// The `set` keys it takes: a module's parameters, a project's
    /// `<inner>.<parameter>` (ADR 0035 §1).
    pub settable: Vec<String>,
}

fn content_ports(ports: &[ModulePort]) -> Vec<ContentPort> {
    ports
        .iter()
        .map(|p| ContentPort {
            name: p.name.clone(),
            role: if p.parameter {
                Role::Parameter
            } else if p.direction == signal_core::Direction::Out {
                Role::Output
            } else {
                Role::Input
            },
            shape: p.shape,
            rate: p.rate.get(),
            kind: p.kind,
        })
        .collect()
}

/// The definitions a module reference of `project` resolves in: the
/// project's composites and every library composite (so a `lib.` reference
/// the project does not use yet resolves too).
fn module_definitions(project: Option<&Project>) -> Vec<CompositeDef> {
    static LIBRARY: OnceLock<Vec<CompositeDef>> = OnceLock::new();
    let library = LIBRARY.get_or_init(|| {
        let instances: String = crate::Library::builtin()
            .names()
            .map(|n| {
                format!(
                    "{n} = {{ module = \"{}{n}\" }}\n",
                    signal_core::LIBRARY_PREFIX
                )
            })
            .collect();
        let text = format!(
            "schema_version = 1\n\n[project]\nname = \"library\"\n\n[instances]\n{instances}"
        );
        Project::from_sources(vec![crate::SourceFile {
            path: crate::PROJECT_FILE.to_owned(),
            text,
        }])
        .map(|all| all.library.iter().map(|l| l.def.clone()).collect())
        .unwrap_or_default()
    });
    let mut defs: Vec<CompositeDef> = project.map(Project::definitions).unwrap_or_default();
    for def in library {
        if !defs.iter().any(|d| d.name == def.name) {
            defs.push(def.clone());
        }
    }
    defs
}

/// What `load` (a slot's content, as written) offers, `project` being the
/// project the slot is in (its folder `dir`, from which a project path is
/// read): `None` when it does not resolve (a `.signal` file is not read).
pub fn content_names(
    project: Option<&Project>,
    dir: &std::path::Path,
    load: &str,
    registry: &Registry,
) -> Option<ContentNames> {
    if !crate::is_project_path(load) {
        let defs = module_definitions(project);
        let ports = signal_graph::ports_of(load, &defs, registry).ok()?;
        let settable = ports
            .iter()
            .filter(|p| p.parameter)
            .map(|p| p.name.clone())
            .collect();
        return Some(ContentNames {
            ports: content_ports(&ports),
            settable,
        });
    }
    if load.ends_with(".signal") {
        return None;
    }
    let loaded = Project::load(&dir.join(load)).ok()?;
    project_content(&loaded, registry)
}

/// What the project `loaded` offers as a slot's content (ADR 0035 §2–3):
/// its exports, its sinks planned as their taps and, without `[exports]
/// outputs`, their channels exported.
pub fn project_content(loaded: &Project, registry: &Registry) -> Option<ContentNames> {
    let mut defs = loaded.definitions();
    defs.extend(loaded.tapped(registry, &mut Vec::new()));
    let mut root = loaded.root.clone();
    root.name = "@content".to_owned();
    let mut channels = Vec::new();
    for instance in &mut root.instances {
        let Some(module) = registry.get(&instance.module) else {
            continue;
        };
        if module.descriptor.sink.is_none() {
            continue;
        }
        let Some(tap) = registry.sink_tap(module.descriptor.type_name) else {
            continue;
        };
        for port in
            module.descriptor.ports.iter().filter(|p| {
                p.direction == signal_core::Direction::In && p.shape == PortShape::Signal
            })
        {
            channels.push(signal_graph::OutputExport {
                name: port.name.to_owned(),
                from: format!("{}.{}_out", instance.name, port.name),
            });
        }
        instance.module = tap.to_owned();
    }
    if root.exports.outputs.is_empty() {
        root.exports.outputs = channels;
    }
    let settable = loaded
        .root
        .instances
        .iter()
        .flat_map(|instance| {
            signal_graph::ports_of(&instance.module, &defs, registry)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.parameter)
                .map(move |p| format!("{}.{}", instance.name, p.name))
        })
        .collect();
    defs.push(root);
    let ports = signal_graph::ports_of("@content", &defs, registry).ok()?;
    Some(ContentNames {
        ports: content_ports(&ports),
        settable,
    })
}

/// The project's interface templates (`interfaces/<name>.toml` of `files`)
/// by name, each one that reads; the others are the load's to report.
pub fn interface_templates(files: &[crate::SourceFile]) -> BTreeMap<String, Interface> {
    let prefix = format!("{}/", crate::INTERFACES);
    files
        .iter()
        .filter_map(|file| {
            let stem = file.path.strip_prefix(&prefix)?.strip_suffix(".toml")?;
            let interface = parse_interface(&file.text, stem).ok()?;
            Some((stem.to_owned(), interface))
        })
        .collect()
}

/// The library template `name` (`lib.voice`), resolved.
pub fn library_template(name: &str) -> Option<Interface> {
    library_templates().get(name).cloned()
}
