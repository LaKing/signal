//! The file model: what `project.toml` and `composites/<name>.toml` may
//! contain, as serde structs that keep the source span of everything an error
//! can point at. The JSON Schema is generated from the same structs.
//!
//! Every table refuses unknown keys. Instance tables are open (their keys are
//! parameter names), so the graph checks those keys against the module;
//! `module`, `gpu` (M2.7-2), `pattern` (ADR 0028 §3), `project`, `seed`,
//! `set` (ADR 0035 §1), `slot`, `load` and `map` (ADR 0037 §2) are reserved
//! there.

use std::collections::BTreeMap;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use toml::Spanned;

/// The instances of a project or composite, by local name.
pub(crate) type Instances = BTreeMap<Spanned<String>, Spanned<InstanceEntry>>;

/// Export tables by export name.
pub(crate) type ExportTargets = BTreeMap<Spanned<String>, Spanned<Targets>>;

/// A Signal project file, `project.toml`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectFile {
    /// The format version of this file; this version of Signal reads 1.
    #[schemars(schema_with = "schema_version")]
    pub(crate) schema_version: Spanned<i64>,
    /// The project's name and run settings.
    pub(crate) project: ProjectSection,
    /// The module instances, by name.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, InstanceEntry>")]
    pub(crate) instances: Option<Spanned<Instances>>,
    /// The connections between instances.
    #[serde(default)]
    pub(crate) wiring: Wiring,
    /// ADR 0035 §2: what the project exposes when another project includes
    /// it (or loads it into a slot). Rendered alone, they mean nothing.
    #[serde(default)]
    pub(crate) exports: ExportsSection,
}

/// A slot interface template, `interfaces/<name>.toml` (ADR 0037 §3): ports
/// and parameters with their types, and nothing inside.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InterfaceFile {
    /// The format version of this file; this version of Signal reads 1.
    #[schemars(schema_with = "schema_version")]
    pub(crate) schema_version: Spanned<i64>,
    /// The template's name and description.
    pub(crate) interface: InterfaceSection,
    /// Its inputs, by name.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, PortSpec>")]
    pub(crate) inputs: BTreeMap<Spanned<String>, Spanned<PortSpec>>,
    /// Its outputs, by name.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, PortSpec>")]
    pub(crate) outputs: BTreeMap<Spanned<String>, Spanned<PortSpec>>,
    /// Its parameters, by name.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, ParamSpec>")]
    pub(crate) parameters: BTreeMap<Spanned<String>, Spanned<ParamSpec>>,
}

/// `[interface]`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InterfaceSection {
    /// The template's name: its file name without `.toml`.
    #[schemars(with = "String")]
    pub(crate) name: Spanned<String>,
    /// What the interface is for.
    #[serde(default)]
    pub(crate) description: Option<String>,
}

/// A port of an interface (ADR 0037 §3): `"<kind>"` (a signal of that kind
/// at rate 1), `"events"` (an event port), or a table.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PortSpec {
    /// `"signal"`, `"events"` or a vector width.
    pub(crate) shape: ShapeSpec,
    /// The rate divisor, at least 1.
    pub(crate) rate: u32,
    /// The kind, `PortKind`'s snake-case name.
    pub(crate) kind: String,
    /// Presentation only.
    pub(crate) unit: Option<String>,
    /// Presentation only.
    pub(crate) range: Option<(f64, f64)>,
    /// Presentation only.
    pub(crate) group: Option<String>,
}

/// A port's shape as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShapeSpec {
    Signal,
    Events,
    Vector(u32),
}

/// The kinds a port may declare: `PortKind`'s names.
pub(crate) const KINDS: [&str; 8] = [
    "audio",
    "control",
    "gate",
    "trigger",
    "frequency",
    "gain",
    "phase",
    "generic",
];

impl<'de> Deserialize<'de> for PortSpec {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PortVisitor;

        fn kind<E: de::Error>(kind: String) -> Result<String, E> {
            if KINDS.contains(&kind.as_str()) {
                Ok(kind)
            } else {
                Err(E::custom(format!(
                    "unknown kind `{kind}`; kinds: {}",
                    KINDS.join(", ")
                )))
            }
        }

        impl<'de> Visitor<'de> for PortVisitor {
            type Value = PortSpec;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a kind (`\"audio\"`), `\"events\"`, or a table `{ shape, rate, kind, unit, range, group }`",
                )
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<PortSpec, E> {
                let (shape, kind) = if value == "events" {
                    (ShapeSpec::Events, "generic".to_owned())
                } else {
                    (ShapeSpec::Signal, kind(value.to_owned())?)
                };
                Ok(PortSpec {
                    shape,
                    rate: 1,
                    kind,
                    unit: None,
                    range: None,
                    group: None,
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PortSpec, A::Error> {
                let mut port = PortSpec {
                    shape: ShapeSpec::Signal,
                    rate: 1,
                    kind: "generic".to_owned(),
                    unit: None,
                    range: None,
                    group: None,
                };
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "shape" => port.shape = map.next_value::<ShapeValue>()?.0,
                        "rate" => {
                            let rate = map.next_value::<u32>()?;
                            if rate == 0 {
                                return Err(de::Error::custom("`rate` is a divisor, at least 1"));
                            }
                            port.rate = rate;
                        }
                        "kind" => port.kind = kind(map.next_value::<String>()?)?,
                        "unit" => port.unit = Some(map.next_value::<String>()?),
                        "range" => {
                            let range = map.next_value::<[f64; 2]>()?;
                            port.range = Some((range[0], range[1]));
                        }
                        "group" => port.group = Some(map.next_value::<String>()?),
                        other => {
                            return Err(de::Error::unknown_field(
                                other,
                                &["shape", "rate", "kind", "unit", "range", "group"],
                            ));
                        }
                    }
                }
                Ok(port)
            }
        }

        deserializer.deserialize_any(PortVisitor)
    }
}

/// `shape = "signal" | "events" | { vector = N }`.
struct ShapeValue(ShapeSpec);

impl<'de> Deserialize<'de> for ShapeValue {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ShapeVisitor;

        impl<'de> Visitor<'de> for ShapeVisitor {
            type Value = ShapeValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("`\"signal\"`, `\"events\"` or `{ vector = <width> }`")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<ShapeValue, E> {
                match value {
                    "signal" => Ok(ShapeValue(ShapeSpec::Signal)),
                    "events" => Ok(ShapeValue(ShapeSpec::Events)),
                    other => Err(E::custom(format!(
                        "unknown shape `{other}`; shapes: \"signal\", \"events\", {{ vector = <width> }}"
                    ))),
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ShapeValue, A::Error> {
                let mut width = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "vector" => width = Some(map.next_value::<u32>()?),
                        other => return Err(de::Error::unknown_field(other, &["vector"])),
                    }
                }
                match width {
                    Some(0) | None => Err(de::Error::custom(
                        "a vector shape is `{ vector = <width> }`, width at least 1",
                    )),
                    Some(width) => Ok(ShapeValue(ShapeSpec::Vector(width))),
                }
            }
        }

        deserializer.deserialize_any(ShapeVisitor)
    }
}

impl JsonSchema for PortSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PortSpec".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "An interface port (ADR 0037 §3): a kind (a signal of that kind at rate 1), \"events\", or a table.",
            "anyOf": [
                { "enum": ["audio", "control", "gate", "trigger", "frequency", "gain", "phase", "generic", "events"] },
                {
                    "type": "object",
                    "properties": {
                        "shape": {
                            "anyOf": [
                                { "enum": ["signal", "events"] },
                                {
                                    "type": "object",
                                    "properties": { "vector": { "type": "integer", "minimum": 1 } },
                                    "required": ["vector"],
                                    "additionalProperties": false
                                }
                            ]
                        },
                        "rate": { "type": "integer", "minimum": 1 },
                        "kind": { "enum": ["audio", "control", "gate", "trigger", "frequency", "gain", "phase", "generic"] },
                        "unit": { "type": "string" },
                        "range": { "type": "array", "items": { "type": "number" }, "minItems": 2 },
                        "group": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            ]
        })
    }
}

/// An interface parameter (ADR 0037 §3): a name with an optional `unit` and
/// `group`; its range and default come from the content.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ParamSpec {
    /// Presentation only.
    #[serde(default)]
    pub(crate) unit: Option<String>,
    /// Presentation only.
    #[serde(default)]
    pub(crate) group: Option<String>,
}

/// A slot's `slot` key (ADR 0037 §2): a template name (`"lib.deck"`, short
/// for `{ from = "lib.deck" }`) or a table declaring the interface.
pub(crate) struct SlotSpec {
    /// The template it starts from, if any, and the value's span.
    pub(crate) from: Option<Spanned<String>>,
    /// Template ports and parameters dropped.
    pub(crate) remove: Vec<Spanned<String>>,
    /// Inputs added or retyped.
    pub(crate) inputs: BTreeMap<Spanned<String>, Spanned<PortSpec>>,
    /// Outputs added or retyped.
    pub(crate) outputs: BTreeMap<Spanned<String>, Spanned<PortSpec>>,
    /// Parameters added or replaced.
    pub(crate) parameters: BTreeMap<Spanned<String>, Spanned<ParamSpec>>,
}

impl<'de> Deserialize<'de> for SlotSpec {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SlotVisitor;

        impl<'de> Visitor<'de> for SlotVisitor {
            type Value = SlotSpec;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a template name (`\"lib.deck\"`) or an interface table `{ from, remove, inputs, outputs, parameters }`",
                )
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<SlotSpec, E> {
                // The string's span is the `slot` value's own.
                Ok(SlotSpec {
                    from: Some(Spanned::new(0..0, value.to_owned())),
                    remove: Vec::new(),
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    parameters: BTreeMap::new(),
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<SlotSpec, A::Error> {
                let mut slot = SlotSpec {
                    from: None,
                    remove: Vec::new(),
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    parameters: BTreeMap::new(),
                };
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "from" => slot.from = Some(map.next_value()?),
                        "remove" => slot.remove = map.next_value()?,
                        "inputs" => slot.inputs = map.next_value()?,
                        "outputs" => slot.outputs = map.next_value()?,
                        "parameters" => slot.parameters = map.next_value()?,
                        other => {
                            return Err(de::Error::unknown_field(
                                other,
                                &["from", "remove", "inputs", "outputs", "parameters"],
                            ));
                        }
                    }
                }
                Ok(slot)
            }
        }

        deserializer.deserialize_any(SlotVisitor)
    }
}

impl JsonSchema for SlotSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SlotSpec".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let port = generator.subschema_for::<PortSpec>();
        let parameter = generator.subschema_for::<ParamSpec>();
        json_schema!({
            "description": "A slot's interface (ADR 0037 §2): a template name (lib.<name> or interfaces/<name>.toml), or a table declaring it, optionally from a template with ports removed, added or retyped.",
            "anyOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string" },
                        "remove": { "type": "array", "items": { "type": "string" } },
                        "inputs": { "type": "object", "additionalProperties": port },
                        "outputs": { "type": "object", "additionalProperties": port },
                        "parameters": { "type": "object", "additionalProperties": parameter }
                    },
                    "additionalProperties": false
                }
            ]
        })
    }
}

/// A `map` value (ADR 0037 §4): the content's port or parameter a slot port
/// matches, or `false`, deliberately unmatched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MapValue {
    Name(String),
    Unmatched,
}

impl<'de> Deserialize<'de> for MapValue {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor;

        impl<'de> Visitor<'de> for MapVisitor {
            type Value = MapValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("the content's port or parameter name, or `false`")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<MapValue, E> {
                Ok(MapValue::Name(value.to_owned()))
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<MapValue, E> {
                if value {
                    Err(E::custom(
                        "`map` takes a name or `false` (deliberately unmatched); `true` means nothing",
                    ))
                } else {
                    Ok(MapValue::Unmatched)
                }
            }
        }

        deserializer.deserialize_any(MapVisitor)
    }
}

/// A Signal composite file, `composites/<name>.toml`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompositeFile {
    /// The format version of this file; this version of Signal reads 1.
    #[schemars(schema_with = "schema_version")]
    pub(crate) schema_version: Spanned<i64>,
    /// The composite's name.
    pub(crate) composite: CompositeSection,
    /// The module instances, by name.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, InstanceEntry>")]
    pub(crate) instances: Option<Spanned<Instances>>,
    /// The connections between instances.
    #[serde(default)]
    pub(crate) wiring: Wiring,
    /// What the composite exposes to the instance that uses it.
    #[serde(default)]
    pub(crate) exports: ExportsSection,
}

/// `[project]`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectSection {
    /// The project's name.
    pub(crate) name: String,
    /// The sample rate in frames per second.
    #[serde(default = "default_sample_rate")]
    #[schemars(with = "u32", range(min = 1))]
    pub(crate) sample_rate: Spanned<u32>,
    /// The master seed of every random stream.
    #[serde(default = "default_seed")]
    #[schemars(with = "u64")]
    pub(crate) seed: Spanned<u64>,
}

fn default_sample_rate() -> Spanned<u32> {
    Spanned::new(0..0, 96_000)
}

fn default_seed() -> Spanned<u64> {
    Spanned::new(0..0, 0)
}

/// `[composite]`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompositeSection {
    /// The composite's name: its file name without `.toml`, used as the module
    /// reference of its instances.
    #[schemars(with = "String")]
    pub(crate) name: Spanned<String>,
    /// The palette group (ADR 0027 §1: `core`, `generator`, `filter`,
    /// `envelope`, `delay`, `physical`, `event`, `visual`, `io`), as a
    /// primitive type's descriptor has one. Required in a library composite.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub(crate) category: Option<Spanned<String>>,
    /// What the composite does and what each export means. Required in a
    /// library composite.
    #[serde(default)]
    pub(crate) description: Option<String>,
}

/// `[wiring]`.
#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Wiring {
    /// Connections from an output port to an input port or parameter.
    #[serde(default)]
    #[schemars(with = "Vec<ConnectionEntry>")]
    pub(crate) connections: Vec<Spanned<ConnectionEntry>>,
}

/// One connection.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConnectionEntry {
    /// The source output port, `instance.port`.
    #[schemars(with = "String")]
    pub(crate) from: Spanned<String>,
    /// The target input port or parameter, `instance.port`.
    #[schemars(with = "String")]
    pub(crate) to: Spanned<String>,
    /// The factor applied to a signal source; 1 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "f64")]
    pub(crate) scale: Option<Spanned<f64>>,
}

/// `[exports]`.
#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExportsSection {
    /// Exported inputs: each maps to one inner input port or a list of them.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, Targets>")]
    pub(crate) inputs: ExportTargets,
    /// Exported outputs: each maps to exactly one inner output port.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, OutputTarget>")]
    pub(crate) outputs: BTreeMap<Spanned<String>, Spanned<OutputTarget>>,
    /// Exported parameters: each maps to one inner parameter or a list of them.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, Targets>")]
    pub(crate) parameters: ExportTargets,
}

fn schema_version(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "const": 1 })
}

/// An instance: `{ module = "<ref>", gpu = "<device>", pattern = "<name>", <parameter> = <number>, … }`,
/// or an included project (`{ project = "<path>", seed, set, … }`, ADR 0035
/// §1), or a slot (`{ slot = <interface>, load, map, seed, set, … }`, ADR
/// 0037 §2). Exactly one of `module`, `project` and `slot`.
pub(crate) struct InstanceEntry {
    pub(crate) module: Option<Spanned<String>>,
    /// ADR 0035 §1: a project folder (or `.signal` file) included as this
    /// instance.
    pub(crate) project: Option<Spanned<String>>,
    /// ADR 0037 §2: the slot's own interface.
    pub(crate) slot: Option<Spanned<SlotSpec>>,
    /// ADR 0037 §2: the slot's content, a module reference or a project path.
    pub(crate) load: Option<Spanned<String>>,
    /// ADR 0037 §4: overrides of the match, by slot port or parameter.
    pub(crate) map: Option<Spanned<MapEntries>>,
    /// ADR 0035 §1: an included project's seed scope.
    pub(crate) seed: Option<Spanned<u64>>,
    /// ADR 0035 §1: overrides of the content's own parameters.
    pub(crate) set: Option<Spanned<SetEntries>>,
    /// M2.7-2: the GPU device a GPU-backed instance is pinned to, by id or
    /// index.
    pub(crate) gpu: Option<Spanned<String>>,
    /// ADR 0028 §3: the pattern file (`patterns/<name>.toml`) whose data the
    /// instance takes.
    pub(crate) pattern: Option<Spanned<String>>,
    /// ADR 0041: the instance's controls left out of snapshots, by
    /// parameter name (only in `project.toml`).
    pub(crate) snapshot_exclude: Option<Spanned<Vec<Spanned<String>>>>,
    pub(crate) parameters: BTreeMap<Spanned<String>, Spanned<f64>>,
}

/// A slot's `map` table: slot port or parameter → content name or `false`.
pub(crate) type MapEntries = BTreeMap<Spanned<String>, Spanned<MapValue>>;

/// A `set` table: content parameter → value.
pub(crate) type SetEntries = BTreeMap<Spanned<String>, Spanned<f64>>;

impl<'de> Deserialize<'de> for InstanceEntry {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = InstanceEntry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "an instance table `{ module = \"<ref>\", gpu = \"<device>\", pattern = \"<name>\", <parameter> = <number>, … }`, `{ project = \"<path>\", … }` or `{ slot = <interface>, load = \"<content>\", … }`",
                )
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<InstanceEntry, A::Error> {
                let mut entry = InstanceEntry {
                    module: None,
                    project: None,
                    slot: None,
                    load: None,
                    map: None,
                    seed: None,
                    set: None,
                    gpu: None,
                    pattern: None,
                    snapshot_exclude: None,
                    parameters: BTreeMap::new(),
                };
                while let Some(key) = map.next_key::<Spanned<String>>()? {
                    match key.get_ref().as_str() {
                        "module" => entry.module = Some(map.next_value()?),
                        "project" => entry.project = Some(map.next_value()?),
                        "slot" => entry.slot = Some(map.next_value()?),
                        "load" => entry.load = Some(map.next_value()?),
                        "map" => entry.map = Some(map.next_value()?),
                        "seed" => entry.seed = Some(map.next_value()?),
                        "set" => entry.set = Some(map.next_value()?),
                        "gpu" => entry.gpu = Some(map.next_value()?),
                        "pattern" => entry.pattern = Some(map.next_value()?),
                        "snapshot_exclude" => entry.snapshot_exclude = Some(map.next_value()?),
                        _ => {
                            entry
                                .parameters
                                .insert(key, map.next_value::<Spanned<f64>>()?);
                        }
                    }
                }
                let kinds = [
                    entry.module.is_some(),
                    entry.project.is_some(),
                    entry.slot.is_some(),
                ];
                match kinds.iter().filter(|k| **k).count() {
                    0 => return Err(de::Error::missing_field("module")),
                    1 => {}
                    _ => {
                        return Err(de::Error::custom(
                            "an instance has exactly one of `module`, `project` and `slot`",
                        ));
                    }
                }
                if entry.slot.is_none() {
                    for (key, given) in
                        [("load", entry.load.is_some()), ("map", entry.map.is_some())]
                    {
                        if given {
                            return Err(de::Error::custom(format!(
                                "`{key}` is a slot's key (`slot = …`, ADR 0037)"
                            )));
                        }
                    }
                }
                if entry.module.is_some() {
                    for (key, given) in
                        [("seed", entry.seed.is_some()), ("set", entry.set.is_some())]
                    {
                        if given {
                            return Err(de::Error::custom(format!(
                                "`{key}` applies to an included project or a slot, not to a module instance"
                            )));
                        }
                    }
                }
                Ok(entry)
            }
        }

        deserializer.deserialize_map(EntryVisitor)
    }
}

impl JsonSchema for InstanceEntry {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "InstanceEntry".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let module = generator.subschema_for::<ModuleRef>();
        let slot = generator.subschema_for::<SlotSpec>();
        json_schema!({
            "description": "An instance: its module (`module`), or an included project (`project`, a folder path, ADR 0035), or a slot (`slot`, its interface, with optional `load` and `map`, ADR 0037); exactly one of the three. A module instance may take a GPU device pin (`gpu`) and a pattern (`pattern`); a project or a slot a `seed` and `set` overrides. Every other key is a parameter value.",
            "type": "object",
            "properties": {
                "module": module,
                "project": { "type": "string" },
                "slot": slot,
                "load": { "type": "string" },
                "map": {
                    "type": "object",
                    "additionalProperties": { "anyOf": [{ "type": "string" }, { "const": false }] }
                },
                "seed": { "type": "integer", "minimum": 0 },
                "set": { "type": "object", "additionalProperties": { "type": "number" } },
                "gpu": { "type": "string", "minLength": 1 },
                "pattern": { "type": "string", "pattern": "^[a-z][a-z0-9_]*$" },
                "snapshot_exclude": {
                    "description": "ADR 0041: the instance's controls (its parameters, or a composite's exported ones) left out of snapshots; only in project.toml.",
                    "type": "array",
                    "items": { "type": "string", "pattern": "^[a-z][a-z0-9_]*$" },
                    "uniqueItems": true
                }
            },
            "oneOf": [
                { "required": ["module"] },
                { "required": ["project"] },
                { "required": ["slot"] }
            ],
            "additionalProperties": { "type": "number" }
        })
    }
}

/// A module reference. Its schema is replaced by the registry's type names
/// when the schema is generated (`crate::schema`).
pub(crate) struct ModuleRef;

impl JsonSchema for ModuleRef {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ModuleRef".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string" })
    }
}

/// The inner ports of an exported input or parameter: one, or a list.
pub(crate) enum Targets {
    /// A single port, `instance.port`; its span is the enclosing value's.
    One(String),
    /// A list of ports (fan-out).
    Many(Vec<Spanned<String>>),
    /// M2.13-2: `{ to = <port or list>, group = "<name>" }`, the same ports
    /// with the group the schematic shows the export in (presentation only).
    Grouped(Box<Targets>, String),
}

impl Targets {
    /// The schematic's group of the export, if the table form names one.
    pub(crate) fn group(&self) -> Option<&str> {
        match self {
            Targets::Grouped(_, group) => Some(group),
            _ => None,
        }
    }
}

/// The inner port of an exported output: `"instance.port"`, or (M2.13-2)
/// `{ from = "instance.port", group = "<name>" }` with the group the
/// schematic shows it in.
pub(crate) struct OutputTarget {
    pub(crate) from: String,
    pub(crate) group: Option<String>,
}

impl<'de> Deserialize<'de> for OutputTarget {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OutputVisitor;

        impl<'de> Visitor<'de> for OutputVisitor {
            type Value = OutputTarget;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // The plain form's wording, which a list (the common mistake) is told.
                f.write_str("a string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<OutputTarget, E> {
                Ok(OutputTarget {
                    from: value.to_owned(),
                    group: None,
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OutputTarget, A::Error> {
                let (mut from, mut group) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "from" => from = Some(map.next_value::<String>()?),
                        "group" => group = Some(map.next_value::<String>()?),
                        other => {
                            return Err(de::Error::unknown_field(other, &["from", "group"]));
                        }
                    }
                }
                let from = from.ok_or_else(|| de::Error::missing_field("from"))?;
                Ok(OutputTarget { from, group })
            }
        }

        deserializer.deserialize_any(OutputVisitor)
    }
}

impl JsonSchema for OutputTarget {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "OutputTarget".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "One inner output port `instance.port`, or a table with it as `from` and the schematic's `group` of the export.",
            "anyOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "properties": { "from": { "type": "string" }, "group": { "type": "string" } },
                    "required": ["from"],
                    "additionalProperties": false
                }
            ]
        })
    }
}

impl<'de> Deserialize<'de> for Targets {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TargetsVisitor;

        impl<'de> Visitor<'de> for TargetsVisitor {
            type Value = Targets;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a port `instance.port`, a list of them, or a table `{ to = <port or list>, group = \"<name>\" }`",
                )
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Targets, A::Error> {
                let (mut to, mut group) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "to" => to = Some(map.next_value::<Targets>()?),
                        "group" => group = Some(map.next_value::<String>()?),
                        other => return Err(de::Error::unknown_field(other, &["to", "group"])),
                    }
                }
                let to = to.ok_or_else(|| de::Error::missing_field("to"))?;
                if matches!(to, Targets::Grouped(..)) {
                    return Err(de::Error::custom(
                        "`to` is a port or a list of ports, not a table",
                    ));
                }
                Ok(match group {
                    Some(group) => Targets::Grouped(Box::new(to), group),
                    None => to,
                })
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Targets, E> {
                Ok(Targets::One(value.to_owned()))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Targets, A::Error> {
                let mut ports = Vec::new();
                while let Some(port) = seq.next_element::<Spanned<String>>()? {
                    ports.push(port);
                }
                Ok(Targets::Many(ports))
            }
        }

        deserializer.deserialize_any(TargetsVisitor)
    }
}

impl JsonSchema for Targets {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Targets".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "One inner port `instance.port`, or a non-empty list of them (fan-out), or a table with them as `to` and the schematic's `group` of the export.",
            "anyOf": [
                { "type": "string" },
                { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                {
                    "type": "object",
                    "properties": {
                        "to": {
                            "anyOf": [
                                { "type": "string" },
                                { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                            ]
                        },
                        "group": { "type": "string" }
                    },
                    "required": ["to"],
                    "additionalProperties": false
                }
            ]
        })
    }
}
