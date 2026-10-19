//! The file model: what `project.toml` and `composites/<name>.toml` may
//! contain, as serde structs that keep the source span of everything an error
//! can point at. The JSON Schema is generated from the same structs.
//!
//! Every table refuses unknown keys. Instance tables are open (their keys are
//! parameter names), so the graph checks those keys against the module;
//! `module`, `gpu` (M2.7-2) and `pattern` (ADR 0028 §3) are reserved there.

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

/// An instance: `{ module = "<ref>", gpu = "<device>", pattern = "<name>", <parameter> = <number>, … }`.
pub(crate) struct InstanceEntry {
    pub(crate) module: Spanned<String>,
    /// M2.7-2: the GPU device a GPU-backed instance is pinned to, by id or
    /// index.
    pub(crate) gpu: Option<Spanned<String>>,
    /// ADR 0028 §3: the pattern file (`patterns/<name>.toml`) whose data the
    /// instance takes.
    pub(crate) pattern: Option<Spanned<String>>,
    pub(crate) parameters: BTreeMap<Spanned<String>, Spanned<f64>>,
}

impl<'de> Deserialize<'de> for InstanceEntry {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = InstanceEntry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "an instance table `{ module = \"<ref>\", gpu = \"<device>\", pattern = \"<name>\", <parameter> = <number>, … }`",
                )
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<InstanceEntry, A::Error> {
                let mut module = None;
                let mut gpu = None;
                let mut pattern = None;
                let mut parameters = BTreeMap::new();
                while let Some(key) = map.next_key::<Spanned<String>>()? {
                    match key.get_ref().as_str() {
                        "module" => module = Some(map.next_value::<Spanned<String>>()?),
                        "gpu" => gpu = Some(map.next_value::<Spanned<String>>()?),
                        "pattern" => pattern = Some(map.next_value::<Spanned<String>>()?),
                        _ => {
                            parameters.insert(key, map.next_value::<Spanned<f64>>()?);
                        }
                    }
                }
                let module = module.ok_or_else(|| de::Error::missing_field("module"))?;
                Ok(InstanceEntry {
                    module,
                    gpu,
                    pattern,
                    parameters,
                })
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
        json_schema!({
            "description": "An instance: its module, the GPU device it is pinned to (`gpu`, a device id or index; GPU-backed instances only), the pattern whose data it takes (`pattern`, the name of patterns/<name>.toml; types that take one only), and the parameter values set on it.",
            "type": "object",
            "properties": {
                "module": module,
                "gpu": { "type": "string", "minLength": 1 },
                "pattern": { "type": "string", "pattern": "^[a-z][a-z0-9_]*$" }
            },
            "required": ["module"],
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
