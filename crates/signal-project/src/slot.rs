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
//!
//! ADR 0045: an **auto** port (`{}`, or `auto = true`) takes the fields it
//! does not pin from its defining end, resolved per level in five stages
//! (`Level::infer`) before the checks; nothing of it reaches the plan.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Serialize;
use signal_core::{PortKind, PortShape, Registry};
use signal_graph::{CompositeDef, ModulePort};
use toml::Spanned;

use crate::Project;
use crate::diagnostic::{Diagnostic, Severity};
use crate::locate::{ContentSpans, DefSpans, Location, SlotEnd, SourceMap};
use crate::model::{InterfaceFile, KINDS, MapValue, ParamSpec, PortSpec, ShapeSpec, SlotSpec};

/// The prefix of a library template reference (`lib.deck`).
pub(crate) const LIBRARY_TEMPLATE: &str = "lib.";

/// ADR 0044 §9: the most voices a slot may have, `std.voice_alloc`'s width.
pub(crate) const MAX_VOICES: u32 = 64;

/// ADR 0044 §4–5: the generated composite's allocator, unpack and sum types.
const ALLOCATOR: &str = "std.voice_alloc";
const UNPACK: &str = "std.unpack64";
const ADD: &str = "std.add";

/// ADR 0044 §2: the allocator's outputs an `each` input may name.
const ALLOCATED: [&str; 3] = ["hz", "gate", "velocity"];

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
    /// ADR 0045: present on an auto port, whose fields above are then its
    /// effective type: the pinned ones as written, the others inferred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<Auto>,
    /// ADR 0044 §2: its role in a slot with `voices`, as written; `None`
    /// for the default (`shared` for an input, `sum` for an output).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<Voice>,
}

/// A port's role in a polyphonic slot (ADR 0044 §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Voice {
    /// An input every voice receives.
    Shared,
    /// A signal input fed per voice by the allocator's output of its name.
    Each,
    /// An output: the voices' outputs summed.
    Sum,
    /// The events input that feeds the slot's allocator.
    Alloc,
}

impl Voice {
    /// The name the field is written with.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Voice::Shared => "shared",
            Voice::Each => "each",
            Voice::Sum => "sum",
            Voice::Alloc => "alloc",
        }
    }

    /// The role of a written name.
    pub(crate) fn named(name: &str) -> Option<Voice> {
        [Voice::Shared, Voice::Each, Voice::Sum, Voice::Alloc]
            .into_iter()
            .find(|v| v.name() == name)
    }
}

impl InterfacePort {
    /// Whether `field` is pinned: written on an auto port, or any field of
    /// a typed port or a parameter.
    pub fn pinned(&self, field: Field) -> bool {
        self.auto
            .as_ref()
            .is_none_or(|auto| !auto.inferred.contains(&field))
    }
}

/// A field of a slot port that an auto port may infer (ADR 0045 §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// `shape`.
    Shape,
    /// `rate`.
    Rate,
    /// `kind`.
    Kind,
    /// `unit`.
    Unit,
    /// `range`.
    Range,
}

/// The fields an auto port may infer, in their order.
pub(crate) const FIELDS: [Field; 5] = [
    Field::Shape,
    Field::Rate,
    Field::Kind,
    Field::Unit,
    Field::Range,
];

/// Which end typed an auto port (ADR 0045 §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoEnd {
    /// A wire at the slot's level: a source of an input, a consumer of an
    /// output.
    Host,
    /// The content's matched port.
    Content,
    /// Nothing: a signal at rate 1 of kind `generic`, no unit, no range.
    Default,
}

/// Where an auto port's inferred fields come from: `describe`'s and
/// `graph`'s `auto.from` (ADR 0045 §9).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AutoFrom {
    /// The defining end's side.
    pub end: AutoEnd,
    /// The defining end: a host endpoint qualified by the level as
    /// [`SlotResolution::slot`] is (`/osc.out`, `rack/osc.out`), or the
    /// content's port name; `None` for the default.
    pub port: Option<String>,
}

/// What makes a port auto (ADR 0045 §1, §9).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Auto {
    /// The fields not pinned, in the order shape, rate, kind, unit, range.
    pub inferred: Vec<Field>,
    /// Their defining end, once resolved (a template's or a written
    /// interface's port: the default).
    pub from: AutoFrom,
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
    let auto = spec.auto.then(|| Auto {
        inferred: FIELDS
            .into_iter()
            .filter(|field| !match field {
                Field::Shape => spec.written.shape,
                Field::Rate => spec.written.rate,
                Field::Kind => spec.written.kind,
                Field::Unit => spec.written.unit,
                Field::Range => spec.written.range,
            })
            .collect(),
        from: AutoFrom {
            end: AutoEnd::Default,
            port: None,
        },
    });
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
        // ADR 0045 §1: in an auto port, `unit = ""` pins "no unit".
        unit: spec
            .unit
            .clone()
            .filter(|unit| !(spec.auto && unit.is_empty())),
        range: spec.range,
        group: spec.group.clone(),
        auto,
        voice: spec.voice,
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
        auto: None,
        voice: None,
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
    } else if name == "from" || name == "remove" || name == "voices" {
        Some(format!(
            "`{name}` is reserved in a slot's interface and cannot name a port"
        ))
    } else {
        None
    }
}

/// Reads an interface template, `interfaces/<stem>.toml` (or a library
/// one, `library`), checking that its `name` is `stem`, and that a library
/// template's ports are all typed (ADR 0045 §5).
pub(crate) fn parse_interface(
    text: &str,
    stem: &str,
    library: bool,
) -> Result<Interface, Vec<Problem>> {
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
        } else if library && port.auto.is_some() {
            problems.push((
                offset,
                format!(
                    "`{}` is an auto port; a library interface's ports are typed, a contract its contents fit (ADR 0045 §5)",
                    port.name
                ),
            ));
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
                let interface = parse_interface(file.text, file.name, true).ok()?;
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
    /// ADR 0044 §5: a polyphonic slot's voices; `None` for a slot without
    /// `voices`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voices: Option<SlotVoices>,
}

/// A polyphonic slot's voices, as the side table records them (ADR 0044
/// §5).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SlotVoices {
    /// How many.
    pub count: u32,
    /// The allocator's legato.
    pub legato: bool,
    /// The generated definition its instance is of (`poly:/synth` at the root, `poly:rack/synth` in the composite `rack`).
    pub definition: String,
    /// Each voice's address relative to the slot's (`v0`, `v1`, …); empty
    /// for an empty slot.
    pub voices: Vec<String>,
}

/// An auto port's effective type (ADR 0045 §9, `retyped`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PortType {
    /// Its shape.
    #[serde(serialize_with = "shape_json")]
    pub shape: PortShape,
    /// Its rate divisor.
    pub rate: u32,
    /// Its kind.
    pub kind: PortKind,
    /// Its unit.
    pub unit: Option<String>,
    /// Its range.
    pub range: Option<(f64, f64)>,
}

/// An auto port whose inferred type an edit changed (ADR 0045 phase 2,
/// `retyped` on `reload` and in the `patch` reply).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Retyped {
    /// The slot, as [`SlotResolution::slot`] names it.
    pub slot: String,
    /// The port.
    pub port: String,
    /// Its type before the edit.
    pub before: PortType,
    /// Its type after it.
    pub after: PortType,
}

/// The auto ports of `after`'s slots whose effective type differs from the
/// same slot's same port in `before` (a slot or port in only one of them
/// is no retype), in `after`'s order.
pub fn retyped(before: &[SlotResolution], after: &[SlotResolution]) -> Vec<Retyped> {
    let type_of = |p: &InterfacePort| PortType {
        shape: p.shape,
        rate: p.rate,
        kind: p.kind,
        unit: p.unit.clone(),
        range: p.range,
    };
    let mut found = Vec::new();
    for slot in after {
        let Some(old) = before.iter().find(|s| s.slot == slot.slot) else {
            continue;
        };
        for port in slot.interface.ports.iter().filter(|p| p.auto.is_some()) {
            let Some(was) = old
                .interface
                .ports
                .iter()
                .find(|p| p.name == port.name && p.role == port.role)
            else {
                continue;
            };
            let (b, a) = (type_of(was), type_of(port));
            if a != b {
                found.push(Retyped {
                    slot: slot.slot.clone(),
                    port: port.name.clone(),
                    before: b,
                    after: a,
                });
            }
        }
    }
    found
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
    crate::locate::shape_words(shape)
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

/// The type of a known end (ADR 0045 §2): what an auto port copies.
#[derive(Clone, Debug, PartialEq)]
struct EndType {
    shape: PortShape,
    rate: u32,
    kind: PortKind,
    unit: Option<String>,
    range: Option<(f64, f64)>,
}

impl EndType {
    /// The default: a signal at rate 1 of kind `generic`, no unit, no range.
    const DEFAULT: EndType = EndType {
        shape: PortShape::Signal,
        rate: 1,
        kind: PortKind::Generic,
        unit: None,
        range: None,
    };

    fn of_module(port: &ModulePort) -> EndType {
        EndType {
            shape: port.shape,
            rate: port.rate.get(),
            kind: port.kind,
            unit: port.unit.clone(),
            range: port.range.map(|r| (r.min, r.max)),
        }
    }

    fn of_port(port: &InterfacePort) -> EndType {
        EndType {
            shape: port.shape,
            rate: port.rate,
            kind: port.kind,
            unit: port.unit.clone(),
            range: port.range,
        }
    }

    /// The auto port `port`'s effective type with `end` its defining end
    /// (`None`: the default): its pinned fields as written, the others
    /// copied.
    fn effective(port: &InterfacePort, end: Option<&EndType>) -> EndType {
        let end = end.unwrap_or(&EndType::DEFAULT);
        EndType {
            shape: if port.pinned(Field::Shape) {
                port.shape
            } else {
                end.shape
            },
            rate: if port.pinned(Field::Rate) {
                port.rate
            } else {
                end.rate
            },
            kind: if port.pinned(Field::Kind) {
                port.kind
            } else {
                end.kind
            },
            unit: if port.pinned(Field::Unit) {
                port.unit.clone()
            } else {
                end.unit.clone()
            },
            range: if port.pinned(Field::Range) {
                port.range
            } else {
                end.range
            },
        }
    }
}

/// An auto port resolved from an end: its effective type, where it comes
/// from, and the defining end as written (a host endpoint) if it is one.
type Resolution = (EndType, AutoFrom, Option<String>);

/// The auto ports of one level, by slot and port.
type ResolvedPorts = BTreeMap<(String, String), Resolution>;

/// One definition's level, as inference reads it (ADR 0045 §2–3).
struct Level<'l> {
    /// Each slot's interface as written.
    declared: &'l BTreeMap<&'l str, &'l Interface>,
    spans: &'l DefSpans,
    modules: &'l BTreeMap<&'l str, &'l str>,
    defs: &'l [CompositeDef],
    /// The definition's name, `""` for the root.
    name: &'l str,
}

/// What inference gives for one level.
struct Inferred {
    /// Every slot's effective interface: its auto ports' fields resolved.
    interfaces: BTreeMap<String, Interface>,
    /// The auto ports resolved from an end: the known ones.
    resolved: ResolvedPorts,
}

impl Inferred {
    /// Whether the auto port `slot.port` is a known end.
    fn known(&self, slot: &str, port: &str) -> bool {
        self.resolved
            .contains_key(&(slot.to_owned(), port.to_owned()))
    }

    /// The host endpoint, as written, that defines the auto port `slot.port`.
    fn defining(&self, slot: &str, port: &str) -> Option<&str> {
        self.resolved
            .get(&(slot.to_owned(), port.to_owned()))?
            .2
            .as_deref()
    }
}

impl Level<'_> {
    /// The slot `slot`'s address: `/fx`, `rack/fx`.
    fn slot_text(&self, slot: &str) -> String {
        self.qualified(slot)
    }

    /// A written endpoint or name qualified by the level: `/osc.out`,
    /// `rack/osc.out`.
    fn qualified(&self, text: &str) -> String {
        if self.name.is_empty() {
            format!("/{text}")
        } else {
            format!("{}/{text}", self.name)
        }
    }

    /// The type of the written endpoint `endpoint` as a known end (ADR 0045
    /// §2), as a wire's source (`source`) or consumer: a module's port, a
    /// typed slot port, or an auto port in `resolved`. A parameter, an auto
    /// port not resolved from an end, or a port on the wrong side is none.
    fn known(
        &self,
        ports: &mut Ports<'_>,
        resolved: &ResolvedPorts,
        endpoint: &str,
        source: bool,
    ) -> Option<EndType> {
        let (instance, port) = split(endpoint)?;
        let wanted = if source { Role::Output } else { Role::Input };
        if let Some(interface) = self.declared.get(instance) {
            let p = interface.port(port)?;
            if p.role != wanted {
                return None;
            }
            return match p.auto {
                None => Some(EndType::of_port(p)),
                Some(_) => resolved
                    .get(&(instance.to_owned(), port.to_owned()))
                    .map(|(end, _, _)| end.clone()),
            };
        }
        if self
            .spans
            .contents
            .get(instance)
            .is_some_and(|c| c.slot.is_some())
        {
            // A slot whose interface did not resolve (reported).
            return None;
        }
        let module = self.modules.get(instance)?;
        let p = ports
            .of(module, self.defs)?
            .iter()
            .find(|p| p.name == port)?;
        let direction = if source {
            signal_core::Direction::Out
        } else {
            signal_core::Direction::In
        };
        (!p.parameter && p.direction == direction).then(|| EndType::of_module(p))
    }

    /// The other ends of the wires on `slot.port` at this level, as written:
    /// the sources of an input, the consumers of an output, each once, in
    /// `str::cmp` order (ADR 0045 §2).
    fn ends(&self, slot: &str, port: &str, input: bool) -> BTreeSet<&str> {
        self.spans
            .connections
            .iter()
            .filter_map(|c| {
                let (here, there) = if input {
                    (&c.to.0, &c.from.0)
                } else {
                    (&c.from.0, &c.to.0)
                };
                (split(here) == Some((slot, port))).then_some(there.as_str())
            })
            .collect()
    }

    /// The content's port each slot port matches (ADR 0037 §4, by name and
    /// `map`), with its type: a content end (ADR 0045 §2).
    fn content_ends(&self, ports: &mut Ports<'_>) -> BTreeMap<(String, String), (EndType, String)> {
        let mut found = BTreeMap::new();
        for (slot, interface) in self.declared {
            let Some(module) = self.modules.get(slot) else {
                continue;
            };
            let Some(content) = ports.of(module, self.defs).cloned() else {
                continue;
            };
            let map = self
                .spans
                .contents
                .get(*slot)
                .and_then(|c| c.slot.as_ref())
                .map(|s| s.map.as_slice())
                .unwrap_or_default();
            for port in interface.ports.iter().filter(|p| p.role != Role::Parameter) {
                let candidate = match map.iter().find(|(key, ..)| *key == port.name) {
                    Some((_, _, MapValue::Unmatched, _)) => continue,
                    Some((_, _, MapValue::Name(target), _)) => target.as_str(),
                    None => port.name.as_str(),
                };
                let output = port.role == Role::Output;
                let Some(matched) = content.iter().find(|c| {
                    c.name == candidate
                        && !c.parameter
                        && (c.direction == signal_core::Direction::Out) == output
                }) else {
                    continue;
                };
                found.insert(
                    ((*slot).to_owned(), port.name.clone()),
                    (EndType::of_module(matched), candidate.to_owned()),
                );
            }
        }
        found
    }

    /// ADR 0045 §3: the auto ports of this level resolved in five stages,
    /// each reading only the ports earlier stages resolved, so the result
    /// is a pure function of the level's text and the content's types,
    /// whatever the order of the entries.
    fn infer(&self, ports: &mut Ports<'_>) -> Inferred {
        let content = self.content_ends(ports);
        let autos: Vec<(&str, &InterfacePort)> = self
            .declared
            .iter()
            .flat_map(|(slot, interface)| {
                interface
                    .ports
                    .iter()
                    .filter(|p| p.auto.is_some() && p.role != Role::Parameter)
                    .map(move |p| (*slot, p))
            })
            .collect();
        let key = |slot: &str, port: &InterfacePort| (slot.to_owned(), port.name.clone());
        let mut resolved = ResolvedPorts::new();
        let from_content = |slot: &str, port: &InterfacePort| {
            content.get(&key(slot, port)).map(|(end, name)| {
                (
                    EndType::effective(port, Some(end)),
                    AutoFrom {
                        end: AutoEnd::Content,
                        port: Some(name.clone()),
                    },
                    None,
                )
            })
        };
        let from_host = |ports: &mut Ports<'_>,
                         resolved: &ResolvedPorts,
                         slot: &str,
                         port: &InterfacePort|
         -> Option<Resolution> {
            let input = port.role == Role::Input;
            self.ends(slot, &port.name, input)
                .into_iter()
                .find_map(|end| {
                    let found = self.known(ports, resolved, end, input)?;
                    Some((
                        EndType::effective(port, Some(&found)),
                        AutoFrom {
                            end: AutoEnd::Host,
                            port: Some(self.qualified(end)),
                        },
                        Some(end.to_owned()),
                    ))
                })
        };
        let inputs = || autos.iter().filter(|(_, p)| p.role == Role::Input);
        let outputs = || autos.iter().filter(|(_, p)| p.role == Role::Output);
        // 1: outputs of loaded slots whose port matched, from the content.
        for (slot, port) in outputs() {
            if let Some(found) = from_content(slot, port) {
                resolved.insert(key(slot, port), found);
            }
        }
        // 2: inputs from their first known host source (an output is known
        // here only if typed or resolved in stage 1).
        for (slot, port) in inputs() {
            if let Some(found) = from_host(ports, &resolved, slot, port) {
                resolved.insert(key(slot, port), found);
            }
        }
        // 3: inputs still open, from the content.
        for (slot, port) in inputs() {
            if !resolved.contains_key(&key(slot, port))
                && let Some(found) = from_content(slot, port)
            {
                resolved.insert(key(slot, port), found);
            }
        }
        // 4: outputs still open, from their first known consumer (an input
        // resolved in stage 2 or 3).
        for (slot, port) in outputs() {
            if !resolved.contains_key(&key(slot, port))
                && let Some(found) = from_host(ports, &resolved, slot, port)
            {
                resolved.insert(key(slot, port), found);
            }
        }
        // 5: inputs still open, from a source resolved in stage 4.
        for (slot, port) in inputs() {
            if !resolved.contains_key(&key(slot, port))
                && let Some(found) = from_host(ports, &resolved, slot, port)
            {
                resolved.insert(key(slot, port), found);
            }
        }
        // The rest: the default, which is no known end.
        let interfaces = self
            .declared
            .iter()
            .map(|(slot, interface)| {
                let mut effective = (*interface).clone();
                for port in &mut effective.ports {
                    if port.auto.is_none() || port.role == Role::Parameter {
                        continue;
                    }
                    let (end, from) = match resolved.get(&key(slot, port)) {
                        Some((end, from, _)) => (end.clone(), from.clone()),
                        None => (
                            EndType::effective(port, None),
                            AutoFrom {
                                end: AutoEnd::Default,
                                port: None,
                            },
                        ),
                    };
                    port.shape = end.shape;
                    port.rate = end.rate;
                    port.kind = end.kind;
                    port.unit = end.unit;
                    port.range = end.range;
                    if let Some(auto) = &mut port.auto {
                        auto.from = from;
                    }
                }
                ((*slot).to_owned(), effective)
            })
            .collect();
        Inferred {
            interfaces,
            resolved,
        }
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
    /// ADR 0044 §5: the composites its polyphonic slots generated.
    generated: Vec<CompositeDef>,
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
                let mut canonical = match &content.included {
                    Some(dir) => format!("project {}", dir.display()),
                    None => format!("module {}", instance.module),
                };
                // ADR 0044 §8: mono ↔ poly moves the content to other
                // addresses, so it is a Replace; a change of N is not.
                if content.slot.as_ref().is_some_and(|s| s.voices.is_some()) {
                    canonical.push_str(" in voices");
                }
                found.insert((name.to_owned(), slot.clone()), canonical);
            }
        }
        found
    }

    /// What a loaded slot's content plays, as text: the plan of its content
    /// alone, instantiated under the slot's name with the values set on it
    /// (the flattened instances, their types, parameters and wiring at their
    /// addresses), whatever the definition it loads is called. Patterns are
    /// left out: a changed table between two otherwise equal contents is an
    /// ordinary edit, which swaps it in under the survivors. Two `load`s
    /// with equal texts play the same: making an own copy of the content
    /// under another name is no Replace (ADR 0037 Notes). `None` when the slot is not loaded or does not plan.
    pub(crate) fn slot_content_plan(&self, key: &SlotKey, registry: &Registry) -> Option<String> {
        let (def, slot) = key;
        let level = if def.is_empty() {
            &self.root
        } else {
            self.composites.iter().find(|c| c.name == *def)?
        };
        let instance = level.instances.iter().find(|i| i.name == *slot)?.clone();
        let mut definitions = self.definitions();
        let mut errors = Vec::new();
        definitions.extend(self.tapped(registry, &mut errors));
        let root = CompositeDef {
            name: String::new(),
            instances: vec![instance],
            ..CompositeDef::default()
        };
        let plan = signal_graph::build_plan(&root, &definitions, registry, 0).ok()?;
        // ADR 0044 §8: mono ↔ poly moves the content to other addresses, so
        // the two never play alike (review of 8f01c59: the plan of the
        // written instance alone did not show it).
        let spans = if def.is_empty() {
            Some(&self.spans.root)
        } else {
            self.spans.composites.get(def)
        };
        let poly = spans
            .and_then(|s| s.contents.get(slot))
            .and_then(|c| c.slot.as_ref())
            .is_some_and(|s| s.voices.is_some());
        let text = serde_json::to_string(&plan).ok()?;
        Some(if poly { format!("voices {text}") } else { text })
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
        defs: &mut Vec<CompositeDef>,
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
            // ADR 0044 §5: what a polyphonic slot's host is made of.
            next.extend(
                results
                    .iter()
                    .flat_map(|(_, r)| r.generated.iter().cloned()),
            );
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
            defs.extend(resolved.generated.iter().cloned());
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
                    let id = format!("{id}/");
                    r.slot = r.slot.replace(&id, "");
                    if let Some(voices) = r.voices.as_mut() {
                        voices.definition = voices.definition.replace(&id, "");
                    }
                    for port in &mut r.interface.ports {
                        if let Some(from) = port.auto.as_mut().and_then(|a| a.from.port.as_mut()) {
                            *from = from.replace(&id, "");
                        }
                    }
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
        generated: Vec::new(),
    };
    let declared: BTreeMap<&str, &Interface> = spans
        .contents
        .iter()
        .filter_map(|(n, c)| Some((n.as_str(), c.slot.as_ref()?.interface.as_ref()?)))
        .collect();
    let modules: BTreeMap<&str, &str> = def
        .instances
        .iter()
        .map(|i| (i.name.as_str(), i.module.as_str()))
        .collect();
    // ADR 0045 §2–3: the auto ports typed by their connections; from here
    // on every slot's interface is its effective one.
    let level = Level {
        declared: &declared,
        spans,
        modules: &modules,
        defs,
        name,
    };
    let inferred = level.infer(ports);
    let interfaces: BTreeMap<&str, &Interface> = inferred
        .interfaces
        .iter()
        .map(|(n, i)| (n.as_str(), i))
        .collect();
    // The type of a written endpoint: a module's port, or a slot's port as
    // its interface types it; `None` for an auto port that no end typed
    // (ADR 0045 §2: no known end).
    let typed_of = |endpoint: &str, ports: &mut Ports<'_>| -> Option<Typed> {
        let (instance, port) = split(endpoint)?;
        if let Some(interface) = interfaces.get(instance) {
            let p = interface.port(port)?;
            if p.auto.is_some() && !inferred.known(instance, port) {
                return None;
            }
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

    // One error per wire when both its ends are slot ports: a shape or rate
    // error found from an output's side is reported only when the input's
    // side reported none on that wire (by its index).
    let mut input_reported: BTreeSet<usize> = BTreeSet::new();
    let mut output_side: Vec<(usize, (usize, usize, String))> = Vec::new();

    // Per slot: its matched names (slot port → content name), and whether
    // it is loaded.
    let mut renames: BTreeMap<&str, (bool, BTreeMap<String, String>)> = BTreeMap::new();
    // The `each` ports of the slots with voices (ADR 0044 §2).
    let mut fed: BTreeSet<(String, String)> = BTreeSet::new();
    for (slot_name, content) in &spans.contents {
        let Some(slot) = &content.slot else {
            continue;
        };
        let Some(interface) = interfaces.get(slot_name.as_str()).copied() else {
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
            voices: None,
        };
        let poly = slot
            .voices
            .as_ref()
            .map(|(voices, span)| (*voices, span.start));
        if poly.is_some() {
            fed.extend(
                interface
                    .ports
                    .iter()
                    .filter(|p| p.voice == Some(Voice::Each) && p.role == Role::Input)
                    .map(|p| (slot_name.clone(), p.name.clone())),
            );
        }
        if let Some((voices, _)) = poly {
            resolution.voices = Some(SlotVoices {
                count: voices.count,
                legato: voices.legato,
                definition: poly_name(name, slot_name),
                voices: Vec::new(),
            });
        }
        check_roles(&mut out, file, &slot_text, interface, poly, load_at);

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
        for (index, connection) in spans.connections.iter().enumerate() {
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
                if poly.is_some() && declared.voice == Some(Voice::Each) {
                    out.errors.push((
                        file,
                        span.start,
                        format!(
                            "{slot_text}: `{port}` is fed by the slot's allocator (`voice = \"each\"`, ADR 0044 §2); the host cannot wire it"
                        ),
                    ));
                    continue;
                }
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
                // ADR 0045 §4: a pinned field (every field of a typed
                // port) is checked against every end; an inferred one is
                // judged at the wire by the graph, or among the ends.
                let mut report = |message: String| {
                    if side {
                        output_side.push((index, (file, span.start, message)));
                    } else {
                        input_reported.insert(index);
                        out.errors.push((file, span.start, message));
                    }
                };
                if declared.pinned(Field::Shape)
                    && other_type.shape != declared.shape
                    && declared.role != Role::Parameter
                {
                    report(format!(
                        "{slot_text}: `{port}` is declared {} on the slot's interface; `{other}` is {}",
                        shape_text(declared.shape),
                        shape_text(other_type.shape)
                    ));
                } else if declared.pinned(Field::Rate)
                    && other_type.rate != declared.rate
                    && declared.role != Role::Parameter
                {
                    report(format!(
                        "{slot_text}: `{port}` is declared at rate divisor {} on the slot's interface; `{other}` runs at {}",
                        declared.rate, other_type.rate
                    ));
                } else if declared.pinned(Field::Kind) && !other_type.kind.matches(declared.kind) {
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
        if poly.is_some() && !content_is_module(content_spans_of(spans, slot_name)) {
            out.errors.push((
                file,
                load_at,
                format!(
                    "{slot_text}: a slot with `voices` loads a module, not a project (ADR 0044 §1): its copies would draw identical streams"
                ),
            ));
            renames.insert(slot_name, (false, BTreeMap::new()));
            continue;
        }
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
            if poly.is_some()
                && interface
                    .port(key)
                    .is_some_and(|p| p.voice == Some(Voice::Alloc))
            {
                out.errors.push((
                    file,
                    key_span.start,
                    format!(
                        "{slot_text}: `{key}` feeds the slot's allocator; it has no content port"
                    ),
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
            if poly.is_some() && port.voice == Some(Voice::Alloc) {
                resolution.matches.push(Match {
                    port: port.name.clone(),
                    role: port.role,
                    content: None,
                    reason: Some("it feeds the slot's allocator".to_owned()),
                });
                continue;
            }
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
            } else if port.role != Role::Parameter
                && port.pinned(Field::Shape)
                && found.shape != port.shape
            {
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
            } else if port.role != Role::Parameter
                && port.pinned(Field::Rate)
                && found.rate.get() != port.rate
            {
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
            if port.pinned(Field::Kind) && !found.kind.matches(port.kind) {
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
                if matched.contains_key(&port.name)
                    || map.contains_key(port.name.as_str())
                    || (poly.is_some() && port.voice == Some(Voice::Alloc))
                {
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
        if let Some((voices, voices_at)) = poly {
            let definition = poly_name(name, slot_name);
            let generated = generate(&definition, voices, module, interface, &matched, &content);
            for problem in generated.problems {
                out.errors
                    .push((file, voices_at, format!("{slot_text}: {problem}")));
            }
            if let Some(instance) = out.def.instances.iter_mut().find(|i| i.name == *slot_name) {
                instance.module = definition;
            }
            if let Some(v) = resolution.voices.as_mut() {
                v.voices = (0..voices.count).map(|e| format!("v{e}")).collect();
            }
            out.generated.push(generated.def);
            renames.insert(slot_name, (true, generated.host));
        } else {
            renames.insert(slot_name, (true, matched));
        }
        out.resolutions.push(resolution);
    }

    // ADR 0045 §4: the ends of an auto port that no renamed wire carries
    // (an empty slot, an unmatched port) agree with its defining end in
    // shape; the graph judges the others.
    for (slot_name, interface) in &interfaces {
        let slot_text = level.slot_text(slot_name);
        for port in &interface.ports {
            let Some(auto) = &port.auto else {
                continue;
            };
            let carried = renames
                .get(slot_name)
                .is_some_and(|(loaded, matched)| *loaded && matched.contains_key(&port.name));
            if carried || port.pinned(Field::Shape) || auto.from.end != AutoEnd::Host {
                continue;
            }
            let Some(defining) = inferred.defining(slot_name, &port.name) else {
                continue;
            };
            let input = port.role == Role::Input;
            for (index, connection) in spans.connections.iter().enumerate() {
                let (here, there) = if input {
                    (&connection.to, &connection.from)
                } else {
                    (&connection.from, &connection.to)
                };
                if split(&here.0) != Some((slot_name, port.name.as_str())) || there.0 == defining {
                    continue;
                }
                let Some(other) = level.known(ports, &inferred.resolved, &there.0, input) else {
                    continue;
                };
                if other.shape != port.shape {
                    let error = (
                        file,
                        here.1.start,
                        format!(
                            "`{slot_text}.{}` (auto) is {}, from `{}`; `{}` {} {}",
                            port.name,
                            shape_text(port.shape),
                            level.qualified(defining),
                            level.qualified(&there.0),
                            if input { "sends" } else { "takes" },
                            shape_text(other.shape)
                        ),
                    );
                    if input {
                        input_reported.insert(index);
                        out.errors.push(error);
                    } else {
                        output_side.push((index, error));
                    }
                }
            }
        }
    }

    for (index, error) in output_side {
        if !input_reported.contains(&index) {
            out.errors.push(error);
        }
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
        // ADR 0045 §4: what the graph's errors on the renamed wire name.
        for (source, endpoint, other) in [
            (true, &connection.from, &connection.to),
            (false, &connection.to, &connection.from),
        ] {
            if let Some(end) = slot_end(&level, &interfaces, &renames, source, endpoint, other) {
                connection_span.slot_ends.push(end);
            }
        }
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
    exports(&mut out, &renames, &interfaces, &fed, name);
    out
}

/// ADR 0045 §4: the slot end `endpoint` of a host wire (its `from` when
/// `source`; `other` its other end, as written), renamed to the content's
/// port: what a graph error on the wire names.
fn slot_end(
    level: &Level<'_>,
    interfaces: &BTreeMap<&str, &Interface>,
    renames: &BTreeMap<&str, (bool, BTreeMap<String, String>)>,
    source: bool,
    endpoint: &str,
    other: &str,
) -> Option<SlotEnd> {
    let (instance, port) = split(endpoint)?;
    let (true, matched) = renames.get(instance)? else {
        return None;
    };
    let target = matched.get(port)?;
    let declared = interfaces.get(instance)?.port(port)?;
    let load = level
        .spans
        .contents
        .get(instance)
        .and_then(|c| c.slot.as_ref())
        .and_then(|s| s.load.as_ref())
        .map_or(
            level.modules.get(instance).copied().unwrap_or(""),
            |(l, _)| l.as_str(),
        );
    let how = declared
        .auto
        .as_ref()
        .map(|auto| match (&auto.from.end, &auto.from.port) {
            (AutoEnd::Host, Some(port)) => format!("auto, from {port}"),
            (AutoEnd::Content, Some(port)) => format!("auto, from the content's {port}"),
            _ => "auto".to_owned(),
        });
    Some(SlotEnd {
        source,
        port: format!("{}.{port}", level.slot_text(instance)),
        how,
        reaches: if source {
            level.qualified(other)
        } else {
            format!("{load}'s {target}")
        },
    })
}

/// ADR 0037 §2: a composite's exports that name a slot's port, checked
/// against the slot's interface and renamed through its match like a wire;
/// a target on an unmatched port or an empty slot is left out, and an
/// export left with no target is dropped (recorded in `vanished`).
fn exports(
    out: &mut Resolved,
    renames: &BTreeMap<&str, (bool, BTreeMap<String, String>)>,
    interfaces: &BTreeMap<&str, &Interface>,
    fed: &BTreeSet<(String, String)>,
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
        // ADR 0044 §2: an `each` port of a slot with voices, as a host wire.
        if fed.contains(&(slot.to_owned(), port.to_owned())) {
            errors.push((
                file,
                at,
                format!(
                    "{}: `{port}` is fed by the slot's allocator (`voice = \"each\"`, ADR 0044 §2); export `{export}` cannot reach it",
                    slot_text(slot)
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

/// ADR 0044 §5: the generated definition of the polyphonic slot `slot` of
/// the definition `host`: `poly:pad_rack/synth`, and `poly:/synth` at the
/// project root (`poly:project/…` would collide with a composite named
/// `project`, review of 8f01c59).
fn poly_name(host: &str, slot: &str) -> String {
    format!("poly:{host}/{slot}")
}

/// ADR 0044 §2: a slot's `voice` fields checked, its errors at `voices`
/// (`poly`, with its offset) and its warnings at `load_at`; without
/// `voices`, only the warning for an `alloc` port that feeds nothing.
fn check_roles(
    out: &mut Resolved,
    file: usize,
    slot_text: &str,
    interface: &Interface,
    poly: Option<(crate::model::VoicesSpec, usize)>,
    load_at: usize,
) {
    let Some((_, at)) = poly else {
        for port in interface
            .ports
            .iter()
            .filter(|p| p.voice == Some(Voice::Alloc))
        {
            out.warnings.push((
                file,
                load_at,
                format!(
                    "{slot_text}: `{}` feeds nothing: the slot has no voices",
                    port.name
                ),
            ));
        }
        return;
    };
    let mut error = |message: String| {
        out.errors
            .push((file, at, format!("{slot_text}: {message}")))
    };
    let mut allocs = 0;
    for port in &interface.ports {
        let name = &port.name;
        match (port.voice, port.role) {
            (Some(Voice::Each), Role::Input) => {
                if port.shape != PortShape::Signal {
                    error(format!(
                        "`{name}` is `each` and {}; an `each` input is a signal, one value per voice (a vector from the host is not yet built, ADR 0044 §2)",
                        shape_text(port.shape)
                    ));
                } else if !ALLOCATED.contains(&name.as_str()) {
                    error(format!(
                        "`{name}` is `each`, fed by the allocator's output of its name; the allocator outputs {}",
                        ALLOCATED.join(", ")
                    ));
                }
            }
            (Some(Voice::Each), _) => {
                error(format!("`{name}` is `each`; only an input may be"));
            }
            (Some(Voice::Alloc), Role::Input) => {
                allocs += 1;
                if port.shape != PortShape::Events {
                    error(format!(
                        "`{name}` is `alloc`; the allocator's input takes note events, so it is an events input"
                    ));
                } else if allocs == 2 {
                    error(format!(
                        "`{name}` is a second `alloc` port; the slot has one allocator"
                    ));
                }
            }
            (Some(Voice::Alloc), _) => {
                error(format!("`{name}` is `alloc`; only an events input may be"));
            }
            (Some(Voice::Sum), Role::Input) => {
                error(format!("`{name}` is `sum`; only an output may be"));
            }
            (Some(Voice::Shared), Role::Output) => {
                error(format!("`{name}` is `shared`; only an input may be"));
            }
            (_, Role::Output) if port.shape != PortShape::Signal => {
                error(format!(
                    "`{name}` is {}; a slot with `voices` sums signal outputs, and merging {} is not yet built (ADR 0044 §2)",
                    shape_text(port.shape),
                    if port.shape == PortShape::Events {
                        "events"
                    } else {
                        "vectors"
                    }
                ));
            }
            _ => {}
        }
    }
    let fed = interface
        .ports
        .iter()
        .any(|p| matches!(p.voice, Some(Voice::Each | Voice::Alloc)));
    if !fed {
        out.warnings.push((
            file,
            load_at,
            format!("{slot_text}: every voice gets the same inputs: the interface has no `each` and no `alloc` port"),
        ));
    }
    if let Some(from @ ("lib.voice" | "lib.instrument")) = interface.from.as_deref() {
        out.warnings.push((
            file,
            load_at,
            format!(
                "{slot_text}: a slot with `voices` from `{from}`; did you mean `lib.poly_voice`?"
            ),
        ));
    }
}

/// A polyphonic slot's generated composite (ADR 0044 §5), and the names
/// the host's wires on the slot's ports take.
struct Generated {
    def: CompositeDef,
    /// Slot port → the generated definition's export.
    host: BTreeMap<String, String>,
    problems: Vec<String>,
}

/// The composite `name` of `voices` copies of `content`: the allocator,
/// an unpack per matched `each` input, the voices, and an `std.add` chain
/// per matched output. Its exports carry the content's names, so the host's
/// values and wires on matched ports reach every voice as they reach a
/// mono slot's content; the `alloc` port exports under its own name.
fn generate(
    name: &str,
    voices: crate::model::VoicesSpec,
    module: &str,
    interface: &Interface,
    matched: &BTreeMap<String, String>,
    content: &[ModulePort],
) -> Generated {
    use signal_graph::{ConnectionDef, InputExport, InstanceDef, OutputExport, ParameterExport};
    let n = voices.count;
    let voice = |e: u32, port: &str| format!("v{e}.{port}");
    let every = |port: &str| (0..n).map(|e| voice(e, port)).collect::<Vec<_>>();
    let wire = |from: String, to: String| ConnectionDef {
        from,
        to,
        scale: None,
    };
    let mut def = CompositeDef {
        name: name.to_owned(),
        ..CompositeDef::default()
    };
    let mut host = BTreeMap::new();
    let mut problems = Vec::new();
    def.instances.push(InstanceDef {
        name: "alloc".to_owned(),
        module: ALLOCATOR.to_owned(),
        parameters: BTreeMap::from([
            ("voices".to_owned(), f64::from(n)),
            ("legato".to_owned(), if voices.legato { 1.0 } else { 0.0 }),
        ]),
        scope: None,
    });
    for e in 0..n {
        def.instances.push(InstanceDef {
            name: format!("v{e}"),
            module: module.to_owned(),
            ..InstanceDef::default()
        });
    }
    let mut sums = Vec::new();
    for port in &interface.ports {
        if port.voice == Some(Voice::Alloc) {
            host.insert(port.name.clone(), port.name.clone());
            def.exports.inputs.push(InputExport {
                name: port.name.clone(),
                to: vec!["alloc.notes".to_owned()],
            });
            continue;
        }
        let Some(target) = matched.get(&port.name) else {
            continue;
        };
        match (port.role, port.voice) {
            (Role::Input, Some(Voice::Each)) => {
                let unpack = format!("each_{}", port.name);
                def.instances.push(InstanceDef {
                    name: unpack.clone(),
                    module: UNPACK.to_owned(),
                    ..InstanceDef::default()
                });
                def.connections
                    .push(wire(format!("alloc.{}", port.name), format!("{unpack}.in")));
                for e in 0..n {
                    def.connections
                        .push(wire(format!("{unpack}.e{e}"), voice(e, target)));
                }
            }
            (Role::Input, _) => {
                host.insert(port.name.clone(), target.clone());
                def.exports.inputs.push(InputExport {
                    name: target.clone(),
                    to: every(target),
                });
            }
            (Role::Output, _) => {
                // Several slot outputs may match one content output (mono
                // into `l` and `r`): one sum, both exported from it.
                host.insert(port.name.clone(), target.clone());
                if !sums.iter().any(|(_, t)| t == target) {
                    sums.push((port.name.clone(), target.clone()));
                }
            }
            (Role::Parameter, _) => {
                host.insert(port.name.clone(), target.clone());
                def.exports.parameters.push(ParameterExport {
                    name: target.clone(),
                    to: every(target),
                    range: None,
                    values: None,
                });
            }
        }
    }
    // The sums: `sum_<port>` is the last add, which the output exports;
    // the voices enter in index order, `((v0 + v1) + v2) + …` (ADR 0054: an
    // input takes one connection, so a chain, not a fan-in).
    for (port, target) in sums {
        let last = format!("sum_{port}");
        let adds = n.saturating_sub(1).max(1);
        for k in 1..=adds {
            let add = if k == adds {
                last.clone()
            } else {
                format!("sum_{port}_{k}")
            };
            def.instances.push(InstanceDef {
                name: add.clone(),
                module: ADD.to_owned(),
                ..InstanceDef::default()
            });
            let a = if k == 1 {
                voice(0, &target)
            } else {
                format!("sum_{port}_{}.out", k - 1)
            };
            def.connections.push(wire(a, format!("{add}.a")));
            if n > 1 {
                def.connections
                    .push(wire(voice(k, &target), format!("{add}.b")));
            }
        }
        def.exports.outputs.push(OutputExport {
            name: target,
            from: format!("{last}.out"),
        });
    }
    // The content's parameters outside the interface, under their own
    // names, so `set` and live edits reach every voice.
    for parameter in content.iter().filter(|c| c.parameter) {
        if matched.values().any(|t| *t == parameter.name) {
            continue;
        }
        def.exports.parameters.push(ParameterExport {
            name: parameter.name.clone(),
            to: every(&parameter.name),
            range: None,
            values: None,
        });
    }
    let mut names = BTreeSet::new();
    let exported = def
        .exports
        .inputs
        .iter()
        .map(|e| &e.name)
        .chain(def.exports.outputs.iter().map(|e| &e.name))
        .chain(def.exports.parameters.iter().map(|e| &e.name));
    for export in exported {
        if !names.insert(export.clone()) {
            problems.push(format!(
                "`{export}` names both the allocator's input and a port or parameter of the content; rename the `alloc` port"
            ));
        }
    }
    Generated {
        def,
        host,
        problems,
    }
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
            let interface = parse_interface(&file.text, stem, false).ok()?;
            Some((stem.to_owned(), interface))
        })
        .collect()
}

/// The library template `name` (`lib.voice`), resolved.
pub fn library_template(name: &str) -> Option<Interface> {
    library_templates().get(name).cloned()
}
