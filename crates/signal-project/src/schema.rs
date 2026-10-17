//! The JSON Schema of project, composite and pattern files (ADR 0009, ADR
//! 0028), generated from the file model plus the module catalogue.

use schemars::generate::SchemaSettings;
use serde_json::{Value, json};
use signal_core::Registry;

use crate::model::{CompositeFile, ProjectFile};
use crate::pattern::{NotesFile, StepsFile};

/// The JSON Schema (draft 2020-12) of `project.toml`,
/// `composites/<name>.toml` and `patterns/<name>.toml` (`kind = "notes"` or
/// `"steps"`): a file is exactly one of the four. A module reference is one
/// of the type names in `registry` or a composite name.
pub fn schema(registry: &Registry) -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let project = generator.subschema_for::<ProjectFile>();
    let composite = generator.subschema_for::<CompositeFile>();
    let notes = generator.subschema_for::<NotesFile>();
    let steps = generator.subschema_for::<StepsFile>();
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
        "title": "Signal project, composite and pattern files",
        "oneOf": [project, composite, notes, steps],
        "$defs": definitions,
    })
}
