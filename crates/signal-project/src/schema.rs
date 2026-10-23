//! The JSON Schema of project, composite and pattern files (ADR 0009, ADR
//! 0028), generated from the file model plus the module catalogue.

use schemars::generate::SchemaSettings;
use serde_json::{Value, json};
use signal_core::{LIBRARY_PREFIX, Registry};

use crate::Library;
use crate::model::{CompositeFile, InterfaceFile, ProjectFile};
use crate::pattern::{NotesFile, StepsFile};

/// The JSON Schema (draft 2020-12) of `project.toml`,
/// `composites/<name>.toml`, `patterns/<name>.toml` (`kind = "notes"` or
/// `"steps"`) and `interfaces/<name>.toml` (ADR 0037 §3): a file is exactly
/// one of the five. A module reference is one
/// of the type names in `registry`, a library composite's `lib.<name>`, or a
/// composite name.
pub fn schema(registry: &Registry) -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let project = generator.subschema_for::<ProjectFile>();
    let composite = generator.subschema_for::<CompositeFile>();
    let notes = generator.subschema_for::<NotesFile>();
    let steps = generator.subschema_for::<StepsFile>();
    let interface = generator.subschema_for::<InterfaceFile>();
    let mut definitions = generator.take_definitions(true);
    let types: Vec<&str> = registry.iter().map(|t| t.descriptor.type_name).collect();
    let library: Vec<String> = Library::builtin()
        .names()
        .map(|n| format!("{LIBRARY_PREFIX}{n}"))
        .collect();
    definitions.insert(
        "ModuleRef".to_owned(),
        json!({
            "description": "A module reference: a registered primitive type, a library composite (lib.<name>, ADR 0027 §4), or the name of a composite in composites/<name>.toml.",
            "anyOf": [
                { "enum": types },
                { "enum": library },
                { "type": "string", "pattern": "^[a-z][a-z0-9_]*$" }
            ]
        }),
    );
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Signal project, composite, pattern and interface files",
        "oneOf": [project, composite, notes, steps, interface],
        "$defs": definitions,
    })
}
