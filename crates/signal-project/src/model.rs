//! The file model: what `project.toml` and `composites/<name>.toml` may
//! contain, as serde structs that keep the source span of everything an error
//! can point at. The JSON Schema is generated from the same structs.
//!
//! Every table refuses unknown keys. Instance tables are open (their keys are
//! parameter names), so the graph checks those keys against the module.

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
    #[serde(default)]
    pub(crate) seed: u64,
}

fn default_sample_rate() -> Spanned<u32> {
    Spanned::new(0..0, 96_000)
}

/// `[composite]`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompositeSection {
    /// The composite's name: its file name without `.toml`, used as the module
    /// reference of its instances.
    #[schemars(with = "String")]
    pub(crate) name: Spanned<String>,
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
    #[schemars(with = "BTreeMap<String, String>")]
    pub(crate) outputs: BTreeMap<Spanned<String>, Spanned<String>>,
    /// Exported parameters: each maps to one inner parameter or a list of them.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, Targets>")]
    pub(crate) parameters: ExportTargets,
}

fn schema_version(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "const": 1 })
}

/// An instance: `{ module = "<ref>", <parameter> = <number>, … }`.
pub(crate) struct InstanceEntry {
    pub(crate) module: Spanned<String>,
    pub(crate) parameters: BTreeMap<Spanned<String>, Spanned<f64>>,
}

impl<'de> Deserialize<'de> for InstanceEntry {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = InstanceEntry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an instance table `{ module = \"<ref>\", <parameter> = <number>, … }`")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<InstanceEntry, A::Error> {
                let mut module = None;
                let mut parameters = BTreeMap::new();
                while let Some(key) = map.next_key::<Spanned<String>>()? {
                    if key.get_ref() == "module" {
                        module = Some(map.next_value::<Spanned<String>>()?);
                    } else {
                        parameters.insert(key, map.next_value::<Spanned<f64>>()?);
                    }
                }
                let module = module.ok_or_else(|| de::Error::missing_field("module"))?;
                Ok(InstanceEntry { module, parameters })
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
            "description": "An instance: its module and the parameter values set on it.",
            "type": "object",
            "properties": { "module": module },
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
}

impl<'de> Deserialize<'de> for Targets {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TargetsVisitor;

        impl<'de> Visitor<'de> for TargetsVisitor {
            type Value = Targets;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a port `instance.port` or a list of them")
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
            "description": "One inner port `instance.port`, or a non-empty list of them (fan-out).",
            "anyOf": [
                { "type": "string" },
                { "type": "array", "items": { "type": "string" }, "minItems": 1 }
            ]
        })
    }
}
