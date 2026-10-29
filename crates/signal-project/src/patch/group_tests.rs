//! ADR 0046 §7–8 (the text half of tests 1, 2 and 4): `slot.group` writes
//! the slot entry in the instance's place, every other key and comment
//! kept, and plans byte-identically; its undo gives the file back byte for
//! byte and its redo groups again; the refusals; a slot's part removed with
//! the slot and given back by the undo; `panel.restore`'s files.

use std::cell::RefCell;
use std::collections::BTreeMap as Files;

use super::*;
use crate::slot::{ContentNames, ContentPort, Role};
use crate::{Project, SourceFile};

static NO_TEMPLATES: BTreeMap<String, Interface> = BTreeMap::new();

fn registry() -> signal_core::Registry {
    let mut registry = signal_core::Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn modules(module: &str) -> Option<Vec<String>> {
    let registry = registry();
    let t = registry.get(module)?;
    Some(
        t.descriptor
            .parameters
            .iter()
            .map(|p| p.name.to_owned())
            .collect(),
    )
}

fn content(load: &str) -> Option<ContentNames> {
    let registry = registry();
    let ports = signal_graph::ports_of(load, &[], &registry).ok()?;
    Some(ContentNames {
        ports: ports
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
            .collect(),
        settable: ports
            .iter()
            .filter(|p| p.parameter)
            .map(|p| p.name.clone())
            .collect(),
    })
}

fn source(load: &str) -> Option<crate::inside::Source> {
    crate::inside::source(&[], load, &registry())
}

fn plan_json(text: &str) -> String {
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".into(),
        text: text.to_owned(),
    }])
    .unwrap_or_else(|d| panic!("{d:?}\n{text}"));
    let plan = project
        .plan(&registry())
        .unwrap_or_else(|d| panic!("{d:?}\n{text}"))
        .plan;
    serde_json::to_string(&plan).unwrap()
}

fn op(json: serde_json::Value) -> Op {
    serde_json::from_value(json).unwrap()
}

/// `ops` applied with the files `files` (the patch's other files) and the
/// bindings `bound`; the files as the patch leaves them.
fn apply_with(
    text: &str,
    ops: &[Op],
    files: &RefCell<Files<String, String>>,
    bound: &dyn Fn(&str) -> Vec<String>,
) -> Result<Patched, PatchError> {
    let file = |path: &str| files.borrow().get(path).cloned();
    let ctx = Context {
        level: "",
        module: &modules,
        templates: &NO_TEMPLATES,
        content: &content,
        file: &file,
        source: &source,
        bound,
    };
    let patched = apply(text, None, ops, &ctx)?;
    for change in &patched.files {
        match &change.after {
            Some(text) => {
                files.borrow_mut().insert(change.path.clone(), text.clone());
            }
            None => {
                files.borrow_mut().remove(&change.path);
            }
        }
    }
    Ok(patched)
}

/// `ops`, then the undo (back to `text` byte for byte), then the redo.
fn round_trip(text: &str, ops: &[Op]) -> String {
    let files = RefCell::new(Files::new());
    let patched =
        apply_with(text, ops, &files, &no_bindings).unwrap_or_else(|e| panic!("{ops:?}: {e}"));
    let inverse: Vec<Op> =
        serde_json::from_str(&serde_json::to_string(&patched.inverse).unwrap()).unwrap();
    let undone = apply_with(&patched.text, &inverse, &files, &no_bindings)
        .unwrap_or_else(|e| panic!("undo: {e}"));
    assert_eq!(undone.text, text, "the undo of {ops:?} is not byte-exact");
    let redone = apply_with(&undone.text, &undone.inverse, &files, &no_bindings)
        .unwrap_or_else(|e| panic!("redo: {e}"));
    assert_eq!(redone.text, patched.text, "the redo of {ops:?} differs");
    patched.text
}

const HOST: &str = r#"schema_version = 1

[project]
name = "group"
sample_rate = 48000

[instances]
# the source
osc = { module = "std.sine_osc", freq = 220 }   # a sine
lfo = { module = "std.sine_osc", freq = 2 }
amp = { module = "std.gain", db = -6 }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "amp.in" },
  { from = "lfo.out", to = "osc.pm", scale = 0.01 },
  { from = "amp.out", to = "out.l" },
  { from = "amp.out", to = "out.r" },
]
"#;

#[test]
fn an_inline_instance_is_grouped_in_its_place_and_plans_the_same() {
    let grouped = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.group", "instance": "osc"}),
        )],
    );
    assert_eq!(
        grouped,
        HOST.replace(
            r#"osc = { module = "std.sine_osc", freq = 220 }   # a sine"#,
            r#"osc = { slot = { inputs = { pm = {} }, outputs = { out = {} }, parameters = { freq = {} } }, load = "std.sine_osc", freq = 220 }   # a sine"#,
        )
    );
    assert_eq!(plan_json(&grouped), plan_json(HOST));
    // A primitive that only feeds: its output, its value.
    let amp = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.group", "instance": "amp"}),
        )],
    );
    assert!(
        amp.contains(r#"amp = { slot = { inputs = { in = {} }, outputs = { out = {} }, parameters = { db = {} } }, load = "std.gain", db = -6 }"#),
        "{amp}"
    );
    assert_eq!(plan_json(&amp), plan_json(HOST));
}

#[test]
fn a_section_keeps_its_comments_and_every_other_key() {
    let host = HOST.replace("amp = { module = \"std.gain\", db = -6 }\n", "")
        + "\n[instances.amp]\n# the level\nmodule = \"std.gain\"   # gain\ndb = -6\nsnapshot_exclude = [\"db\"]\n";
    let grouped = round_trip(
        &host,
        &[op(
            serde_json::json!({"op": "slot.group", "instance": "amp"}),
        )],
    );
    assert!(
        grouped.contains(
            "[instances.amp]\n# the level\nslot = { inputs = { in = {} }, outputs = { out = {} }, parameters = { db = {} } }   # gain\nload = \"std.gain\"\ndb = -6\nsnapshot_exclude = [\"db\"]\n"
        ),
        "{grouped}"
    );
    assert_eq!(plan_json(&grouped), plan_json(&host));
}

#[test]
fn the_panels_bindings_name_ports_and_parameters_too() {
    let files = RefCell::new(Files::new());
    let bound = |name: &str| {
        if name == "lfo" {
            vec!["freq".to_owned(), "nothing".to_owned()]
        } else {
            Vec::new()
        }
    };
    let patched = apply_with(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.group", "instance": "lfo"}),
        )],
        &files,
        &bound,
    )
    .unwrap();
    assert!(
        patched.text.contains(r#"lfo = { slot = { outputs = { out = {} }, parameters = { freq = {} } }, load = "std.sine_osc", freq = 2 }"#),
        "{}",
        patched.text
    );
    assert_eq!(plan_json(&patched.text), plan_json(HOST));
    // An unbound, unwired, unset instance: an interface of nothing.
    let lone = HOST.replace(
        "out = { module = \"std.audio_out\" }",
        "out = { module = \"std.audio_out\" }\nidle = { module = \"std.gain\" }",
    );
    let idle = round_trip(
        &lone,
        &[op(
            serde_json::json!({"op": "slot.group", "instance": "idle"}),
        )],
    );
    assert!(
        idle.contains(r#"idle = { slot = {}, load = "std.gain" }"#),
        "{idle}"
    );
    assert_eq!(plan_json(&idle), plan_json(&lone));
}

#[test]
fn grouping_is_refused_with_the_reason() {
    let files = RefCell::new(Files::new());
    let refused = |text: &str, instance: &str| {
        apply_with(
            text,
            &[op(
                serde_json::json!({"op": "slot.group", "instance": instance}),
            )],
            &files,
            &no_bindings,
        )
        .unwrap_err()
    };
    let grouped = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.group", "instance": "osc"}),
        )],
    );
    let again = refused(&grouped, "osc");
    assert!(again.message.contains("a slot already"), "{again}");
    assert_eq!(refused(HOST, "nobody").code, "unknown-instance");
    let gpu = HOST.replace(
        r#"amp = { module = "std.gain", db = -6 }"#,
        r#"amp = { module = "std.gain", gpu = "default", db = -6 }"#,
    );
    let error = refused(&gpu, "amp");
    assert!(error.message.contains("`gpu`"), "{error}");
    let unknown = HOST.replace("std.gain", "nowhere.gain");
    assert_eq!(refused(&unknown, "amp").code, "unknown-module");
}

#[test]
fn an_untrusted_restore_of_module_only_ungroups_a_slot() {
    let files = RefCell::new(Files::new());
    let restore = |text: &str, changes: serde_json::Value| {
        apply_with(
            text,
            &[op(
                serde_json::json!({"op": "slot.restore", "slot": "osc", "changes": changes}),
            )],
            &files,
            &no_bindings,
        )
    };
    // On a plain instance, a `module` change makes no slot of it.
    let error = restore(
        HOST,
        serde_json::json!([{"path": ["module"], "value": "\"std.gain\""}]),
    )
    .unwrap_err();
    assert!(error.message.contains("makes a slot"), "{error}");
    // On a slot, writing `module` while the slot key stays.
    let grouped = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.group", "instance": "osc"}),
        )],
    );
    let error = restore(
        &grouped,
        serde_json::json!([{"path": ["module"], "value": "\"std.gain\""}]),
    )
    .unwrap_err();
    assert!(error.message.contains("makes a slot"), "{error}");
}

const PART: &str =
    "<signal-panel part=\"osc\">\n  <div class=\"signal-cards\"></div>\n</signal-panel>\n";

#[test]
fn a_slot_deleted_takes_its_part_and_the_undo_gives_it_back() {
    let grouped = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.group", "instance": "osc"}),
        )],
    );
    let files = RefCell::new(Files::from([(
        "ui/parts/osc.html".to_owned(),
        PART.to_owned(),
    )]));
    let removed = apply_with(
        &grouped,
        &[
            op(serde_json::json!({"op": "connection.remove", "from": "osc.out", "to": "amp.in"})),
            op(serde_json::json!({"op": "connection.remove", "from": "lfo.out", "to": "osc.pm"})),
            op(serde_json::json!({"op": "instance.remove", "name": "osc"})),
        ],
        &files,
        &no_bindings,
    )
    .unwrap();
    assert_eq!(removed.files.len(), 1);
    assert_eq!(removed.files[0].path, "ui/parts/osc.html");
    assert_eq!(removed.files[0].after, None);
    assert!(files.borrow().is_empty());
    let inverse: Vec<Op> =
        serde_json::from_str(&serde_json::to_string(&removed.inverse).unwrap()).unwrap();
    let undone = apply_with(&removed.text, &inverse, &files, &no_bindings).unwrap();
    assert_eq!(undone.text, grouped);
    assert_eq!(
        files.borrow().get("ui/parts/osc.html").map(String::as_str),
        Some(PART)
    );
    // An instance that is no slot has no part to take.
    let files = RefCell::new(Files::from([(
        "ui/parts/out.html".to_owned(),
        PART.to_owned(),
    )]));
    let plain = apply_with(
        HOST,
        &[
            op(serde_json::json!({"op": "connection.remove", "from": "amp.out", "to": "out.l"})),
            op(serde_json::json!({"op": "connection.remove", "from": "amp.out", "to": "out.r"})),
            op(serde_json::json!({"op": "instance.remove", "name": "out"})),
        ],
        &files,
        &no_bindings,
    )
    .unwrap();
    assert!(plain.files.is_empty());
}

#[test]
fn panel_restore_writes_only_the_presentation_files_it_names() {
    let files = RefCell::new(Files::from([("ui/main.html".to_owned(), "old".to_owned())]));
    let restore = |file: &str, text: Option<&str>, was: Option<&str>| {
        apply_with(
            HOST,
            &[op(
                serde_json::json!({"op": "panel.restore", "file": file, "text": text, "was": was}),
            )],
            &files,
            &no_bindings,
        )
    };
    for file in [
        "../project.toml",
        "studio.css",
        "parts/../main.html",
        "parts/Bad.html",
        "schematic.toml",
    ] {
        let error = restore(file, Some("x"), None).unwrap_err();
        assert_eq!(error.code, "bad-value", "{file}");
    }
    assert!(restore("main.html", Some("new"), Some("other")).is_err());
    let done = restore("main.html", Some("new"), Some("old")).unwrap();
    assert_eq!(done.files[0].path, "ui/main.html");
    assert_eq!(
        done.inverse,
        vec![Op::PanelRestore {
            file: "main.html".to_owned(),
            text: Some("old".to_owned()),
            was: Some("new".to_owned()),
        }]
    );
    assert!(restore("parts/rack.fx.html", Some("x"), None).is_ok());
}
