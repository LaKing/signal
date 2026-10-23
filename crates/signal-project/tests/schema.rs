//! The generated JSON Schema accepts the example project and rejects files the
//! loader rejects.
//!
//! No JSON Schema validator crate is locked, so this file has its own: a
//! subset validator for exactly the keywords the generated schema uses. It is
//! not a reference implementation. It fails closed: an unsupported keyword or
//! pattern panics instead of passing.

use std::fs;
use std::path::Path;

use serde_json::Value;
use signal_core::Registry;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// Validates instances against the schema whose `$defs` are in `root`.
struct Validator<'s> {
    root: &'s Value,
}

impl Validator<'_> {
    fn valid(&self, schema: &Value, value: &Value) -> bool {
        let object = match schema {
            Value::Bool(accept) => return *accept,
            Value::Object(object) => object,
            other => panic!("not a schema: {other}"),
        };
        let mut valid = true;
        for (keyword, argument) in object {
            valid &=
                match keyword.as_str() {
                    "$schema" | "title" | "description" | "default" | "format" | "$defs" => true,
                    "$ref" => self.valid(self.reference(argument), value),
                    "type" => match argument {
                        Value::String(name) => has_type(value, name),
                        Value::Array(names) => {
                            names.iter().any(|n| has_type(value, n.as_str().unwrap()))
                        }
                        other => panic!("unsupported type {other}"),
                    },
                    "enum" => argument.as_array().unwrap().contains(value),
                    "const" => argument == value,
                    "anyOf" => argument
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|s| self.valid(s, value)),
                    "oneOf" => {
                        argument
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|s| self.valid(s, value))
                            .count()
                            == 1
                    }
                    "properties" => match value.as_object() {
                        Some(fields) => argument.as_object().unwrap().iter().all(|(name, s)| {
                            fields.get(name).is_none_or(|field| self.valid(s, field))
                        }),
                        None => true,
                    },
                    "required" => match value.as_object() {
                        Some(fields) => argument
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|name| fields.contains_key(name.as_str().unwrap())),
                        None => true,
                    },
                    "additionalProperties" => match value.as_object() {
                        Some(fields) => {
                            let declared = object.get("properties").and_then(Value::as_object);
                            fields
                                .iter()
                                .filter(|(name, _)| declared.is_none_or(|d| !d.contains_key(*name)))
                                .all(|(_, field)| self.valid(argument, field))
                        }
                        None => true,
                    },
                    "items" => match value.as_array() {
                        Some(items) => items.iter().all(|item| self.valid(argument, item)),
                        None => true,
                    },
                    "minItems" => value
                        .as_array()
                        .is_none_or(|items| items.len() as u64 >= argument.as_u64().unwrap()),
                    "minimum" => value
                        .as_f64()
                        .is_none_or(|x| x >= argument.as_f64().unwrap()),
                    "maximum" => value
                        .as_f64()
                        .is_none_or(|x| x <= argument.as_f64().unwrap()),
                    "pattern" => {
                        assert_eq!(
                            argument, "^[a-z][a-z0-9_]*$",
                            "unsupported pattern {argument}"
                        );
                        value.as_str().is_none_or(is_name)
                    }
                    other => panic!("unsupported keyword `{other}`"),
                };
        }
        valid
    }

    fn reference(&self, argument: &Value) -> &Value {
        let path = argument.as_str().unwrap();
        let name = path
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("unsupported $ref {path}"));
        &self.root["$defs"][name]
    }
}

fn has_type(value: &Value, name: &str) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|x| x.fract() == 0.0)
        }
        other => panic!("unsupported type `{other}`"),
    }
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn example(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/example")
        .join(file);
    fs::read_to_string(path).unwrap()
}

/// Whether the TOML `text` validates against the schema.
fn validates(text: &str) -> bool {
    let schema = signal_project::schema(&registry());
    let value: Value = toml::from_str(text).unwrap();
    Validator { root: &schema }.valid(&schema, &value)
}

#[test]
fn the_example_project_validates() {
    assert!(validates(&example("project.toml")));
    assert!(validates(&example("composites/voice.toml")));
}

#[test]
fn the_schema_rejects_what_the_loader_rejects() {
    let project = example("project.toml");
    let composite = example("composites/voice.toml");
    let cases = [
        project.replace("[wiring]", "colour = 1\n\n[wiring]"),
        project.replace("seed = 7", "seed = 7\nnmae = \"x\""),
        project.replace("std.audio_out", "std.nope"),
        project.replace("schema_version = 1", "schema_version = 2"),
        project.replace(r#"scale = 0.5"#, r#"scale = "half""#),
        composite.replace(r#"out = "shaper.out""#, r#"out = ["shaper.out"]"#),
        composite.replace(r#"pm = ["low.pm", "high.pm"]"#, "pm = []"),
        // ADR 0035 §2: a project may export, so a composite's text under
        // `[project]` is a valid project now; under `[interface]` it is
        // none of the file kinds (an interface has no instances).
        composite.replace("[composite]\n", "[interface]\n"),
    ];
    for case in &cases {
        assert!(!validates(case), "accepted:\n{case}");
    }
}

#[test]
fn module_references_list_the_registry() {
    let schema = signal_project::schema(&registry());
    let names: Vec<&str> = registry().iter().map(|t| t.descriptor.type_name).collect();
    let listed: Vec<&str> = schema["$defs"]["ModuleRef"]["anyOf"][0]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(listed, names);
    assert!(listed.contains(&"std.sine_osc"));
}

/// M2.11-1 (ADR 0028 §2): both pattern kinds validate, a file is exactly one
/// kind, and what the loader refuses by schema the schema refuses.
#[test]
fn pattern_files_validate_by_kind() {
    let notes = "schema_version = 1\nkind = \"notes\"\nlength = 4\n\n[[notes]]\nkey = 60\nstart = 0\nlength = 1\nvelocity = 0.5\n";
    let steps = "schema_version = 1\nkind = \"steps\"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = \"x..X\"\nhat = \".x.x\"\n\n[levels]\nhat = 0.5\n";
    assert!(validates(notes));
    assert!(validates(steps));
    assert!(validates(&notes.replace(
        "\n[[notes]]\nkey = 60\nstart = 0\nlength = 1\nvelocity = 0.5\n",
        "notes = []\n"
    )));
    for case in [
        notes.replace("kind = \"notes\"", "kind = \"steps\""),
        notes.replace("velocity = 0.5", "velocity = 1.5"),
        notes.replace("velocity = 0.5", "velocity = 0.5\ncolour = 1"),
        notes.replace("length = 4\n", ""),
        steps.replace("steps = 4", "steps = 300"),
        notes.replace("key = 60", "key = 60.5"),
        notes.replace("key = 60", "key = 128"),
        notes.replace(
            "\n[[notes]]\nkey = 60\nstart = 0\nlength = 1\nvelocity = 0.5\n",
            "",
        ),
        steps.replace("hat = 0.5", "hat = 1.5"),
        steps.replace("hat = \".x.x\"", "hat = { cells = \".x.x\", level = 0.5 }"),
        steps.replace("kind = \"steps\"", "kind = \"chords\""),
    ] {
        assert!(!validates(&case), "accepted:\n{case}");
    }
    let project = example("project.toml");
    assert!(validates(&project.replace(
        "module = \"std.audio_out\"",
        "module = \"std.audio_out\", pattern = \"beat\""
    )));
    assert!(!validates(&project.replace(
        "module = \"std.audio_out\"",
        "module = \"std.audio_out\", pattern = 3"
    )));
}
