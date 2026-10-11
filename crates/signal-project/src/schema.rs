//! The JSON Schema of project and composite files (ADR 0009), generated from
//! the file model plus the module catalogue.

use schemars::generate::SchemaSettings;
use serde_json::{Value, json};
use signal_core::Registry;

use crate::model::{CompositeFile, ProjectFile};

/// The JSON Schema (draft 2020-12) of `project.toml` and
/// `composites/<name>.toml`: a file is one or the other. A module reference is
/// one of the type names in `registry` or a composite name.
pub fn schema(registry: &Registry) -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let project = generator.subschema_for::<ProjectFile>();
    let composite = generator.subschema_for::<CompositeFile>();
    let mut definitions = generator.take_definitions(true);
    let types: Vec<&str> = registry.iter().map(|t| t.descriptor.type_name).collect();
    definitions.insert(
        "ModuleRef".to_owned(),
        json!({
            "description": "A module reference: a registered primitive type, or the name of a composite in composites/<name>.toml.",
            "anyOf": [
                { "enum": types },
                { "type": "string", "pattern": "^[a-z][a-z0-9_]*$" }
            ]
        }),
    );
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Signal project and composite files",
        "oneOf": [project, composite],
        "$defs": definitions,
    })
}
