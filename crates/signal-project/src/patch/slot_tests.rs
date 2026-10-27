//! ADR 0037 test 4 (the text half) and the slot ops of `patch`: add, load,
//! Replace, unload, map and every interface edit write the expected text,
//! every comment outside the keys they write kept; a key removed from a
//! section takes its comment lines and trailing comment, which its undo
//! puts back; each undo puts the file back byte for byte (the tables it
//! made removed again) and its redo writes the edit again; a rename keeps
//! each key in its place and rewrites the wires, `map` keys, values and
//! `snapshot_exclude`; chained renames in one op are refused; removing a
//! wired or exported port is refused, naming them; untrusted restores
//! touch nothing but the slot's own keys.

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

/// What `std.gain` and `std.mix` offer, as the host's closure gives it.
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

fn ctx() -> Context<'static> {
    Context {
        level: "",
        module: &modules,
        templates: &NO_TEMPLATES,
        content: &content,
    }
}

fn plans(text: &str) -> Result<(), String> {
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".into(),
        text: text.to_owned(),
    }])
    .map_err(|d| format!("{d:?}"))?;
    project
        .plan(&registry())
        .map(|_| ())
        .map_err(|d| format!("{d:?}"))
}

/// `ops` on `text`; its undo back to `text` byte for byte; the redo to the
/// patched text again. Returns the patched text and the patch.
fn round_trip(text: &str, ops: &[Op]) -> (String, Patched) {
    let patched = apply(text, None, ops, &ctx()).unwrap_or_else(|e| panic!("{ops:?}: {e}"));
    let inverse: Vec<Op> =
        serde_json::from_str(&serde_json::to_string(&patched.inverse).unwrap()).unwrap();
    let undone =
        apply(&patched.text, None, &inverse, &ctx()).unwrap_or_else(|e| panic!("undo: {e}"));
    assert_eq!(undone.text, text, "the undo of {ops:?} is not byte-exact");
    let redone =
        apply(&undone.text, None, &undone.inverse, &ctx()).unwrap_or_else(|e| panic!("redo: {e}"));
    assert_eq!(redone.text, patched.text, "the redo of {ops:?} differs");
    (patched.text.clone(), patched)
}

const HOST: &str = r#"schema_version = 1

[project]
name = "slots"
sample_rate = 48000

[instances]
# the source
osc = { module = "std.sine_osc", freq = 220 }
fx = { slot = { inputs = { x = "audio" }, outputs = { y = "audio" } }, load = "std.gain", map = { x = "in", y = "out" } }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "fx.x" },   # into the slot
  { from = "fx.y", to = "out.l" },
  { from = "fx.y", to = "out.r" },
]
"#;

fn op(json: serde_json::Value) -> Op {
    serde_json::from_value(json).unwrap()
}

#[test]
fn the_fixture_plans() {
    plans(HOST).unwrap();
}

#[test]
fn a_slot_is_added_inline_and_undone() {
    let (text, _) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.add", "name": "spare", "slot": "lib.voice"}),
        )],
    );
    assert!(
        text.contains("out = { module = \"std.audio_out\" }\nspare = { slot = \"lib.voice\" }\n"),
        "{text}"
    );
    plans(&text).unwrap();
    let (text, _) = round_trip(
        HOST,
        &[op(serde_json::json!({"op": "slot.add", "name": "blank"}))],
    );
    assert!(text.contains("blank = { slot = {} }\n"), "{text}");
    plans(&text).unwrap();
    let (text, _) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.add", "name": "g", "slot": {"inputs": {"in": "audio"}, "outputs": {"out": "audio"}}, "load": "std.gain"}),
        )],
    );
    assert!(
        text.contains(
            "g = { slot = { inputs = { in = \"audio\" }, outputs = { out = \"audio\" } }, load = \"std.gain\" }\n"
        ),
        "{text}"
    );
    plans(&text).unwrap();
    // Refused: a taken name, a bad one, a number for `slot`.
    for bad in [
        serde_json::json!({"op": "slot.add", "name": "fx"}),
        serde_json::json!({"op": "slot.add", "name": "Fx"}),
        serde_json::json!({"op": "slot.add", "name": "z", "slot": 3}),
    ] {
        assert!(
            apply(HOST, None, &[op(bad.clone())], &ctx()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn unload_load_and_replace_write_load_and_keep_what_the_content_takes() {
    let (empty, _) = round_trip(
        HOST,
        &[op(serde_json::json!({"op": "slot.unload", "slot": "fx"}))],
    );
    assert!(
        empty.contains("fx = { slot = { inputs = { x = \"audio\" }, outputs = { y = \"audio\" } }, map = { x = \"in\", y = \"out\" } }\n"),
        "{empty}"
    );
    plans(&empty).unwrap();
    // Unloading an empty slot is refused.
    assert!(
        apply(
            &empty,
            None,
            &[op(serde_json::json!({"op": "slot.unload", "slot": "fx"}))],
            &ctx()
        )
        .is_err()
    );
    // Load again: the map is kept, it names std.gain's ports.
    let (loaded, patched) = round_trip(
        &empty,
        &[op(
            serde_json::json!({"op": "slot.load", "slot": "fx", "load": "std.gain"}),
        )],
    );
    assert!(patched.notes.is_empty(), "{:?}", patched.notes);
    assert!(
        loaded.contains("map = { x = \"in\", y = \"out\" }, load = \"std.gain\" }"),
        "{loaded}"
    );
    plans(&loaded).unwrap();
    // Replace with std.mix: no `in`, no `out` input... `out` is kept (mix has
    // an output `out`), `in` is removed, and the reply says so.
    let (replaced, patched) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.load", "slot": "fx", "load": "std.mix"}),
        )],
    );
    assert_eq!(
        patched.notes,
        ["removed map.x = \"in\": `std.mix` has no such port"]
    );
    assert!(
        replaced.contains("load = \"std.mix\", map = { y = \"out\" } }"),
        "{replaced}"
    );
    plans(&replaced).unwrap();
    // An explicit map replaces the slot's.
    let (mapped, _) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.load", "slot": "fx", "load": "std.mix", "map": {"x": "a", "y": "out"}}),
        )],
    );
    assert!(
        mapped.contains("map = { x = \"a\", y = \"out\" }"),
        "{mapped}"
    );
    plans(&mapped).unwrap();
    // Not a slot: refused.
    assert!(
        apply(
            HOST,
            None,
            &[op(
                serde_json::json!({"op": "slot.load", "slot": "osc", "load": "std.mix"})
            )],
            &ctx()
        )
        .is_err()
    );
}

#[test]
fn map_is_written_and_removed() {
    let (text, _) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.map", "slot": "fx", "map": {"x": "in", "y": false}}),
        )],
    );
    assert!(text.contains("map = { x = \"in\", y = false }"), "{text}");
    let (text, _) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.map", "slot": "fx", "map": {}}),
        )],
    );
    assert!(!text.contains("map ="), "{text}");
    // A key that is no slot port is refused.
    assert!(
        apply(
            HOST,
            None,
            &[op(
                serde_json::json!({"op": "slot.map", "slot": "fx", "map": {"nope": "in"}})
            )],
            &ctx()
        )
        .is_err()
    );
}

#[test]
fn interface_edits_add_remove_retype_and_rename() {
    // Add a parameter, then a value set on it plays (std.gain's db).
    let (text, _) = round_trip(
        HOST,
        &[op(serde_json::json!({"op": "slot.interface", "slot": "fx",
            "add": [{"name": "volume", "role": "parameter", "unit": "dB"}]}))],
    );
    assert!(
        text.contains("parameters = { volume = { unit = \"dB\" } }"),
        "{text}"
    );
    plans(&text).unwrap();
    // Retype: y becomes a control output of rate 1 with a group.
    let (text, _) = round_trip(
        HOST,
        &[op(serde_json::json!({"op": "slot.interface", "slot": "fx",
            "retype": [{"name": "y", "kind": "audio", "group": "out"}]}))],
    );
    assert!(
        text.contains("outputs = { y = { kind = \"audio\", group = \"out\" } }"),
        "{text}"
    );
    plans(&text).unwrap();
    // Remove a wired port: refused, naming the wires.
    let refused = apply(
        HOST,
        None,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "fx", "remove": ["y"]}),
        )],
        &ctx(),
    )
    .unwrap_err();
    assert_eq!(refused.code, "in-use");
    assert!(refused.message.contains("fx.y → out.l") && refused.message.contains("fx.y → out.r"));
    // With its wires, and its map key; undone byte for byte.
    let (text, _) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "fx", "remove": ["y"], "removeWires": true}),
        )],
    );
    assert!(!text.contains("fx.y") && !text.contains("y = "), "{text}");
    assert!(text.contains("map = { x = \"in\" }"), "{text}");
    plans(&text).unwrap();
    // Rename x → input: the wire, the map key and its comment follow.
    let (text, patched) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"x": "input"}}),
        )],
    );
    assert!(
        text.contains("{ from = \"osc.out\", to = \"fx.input\" },   # into the slot"),
        "{text}"
    );
    assert!(
        text.contains("map = { input = \"in\", y = \"out\" }"),
        "{text}"
    );
    assert!(text.contains("inputs = { input = \"audio\" }"), "{text}");
    assert_eq!(
        patched.renamed,
        [("fx.x".to_owned(), "fx.input".to_owned())]
    );
    assert!(text.contains("# the source"), "{text}");
    plans(&text).unwrap();
    // A rename onto a taken name, an unknown port: refused.
    for bad in [
        serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"x": "y"}}),
        serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"nope": "z"}}),
        serde_json::json!({"op": "slot.interface", "slot": "fx", "add": [{"name": "from", "role": "input"}]}),
        serde_json::json!({"op": "slot.interface", "slot": "fx", "add": [{"name": "k", "role": "input", "kind": "loud"}]}),
    ] {
        assert!(
            apply(HOST, None, &[op(bad.clone())], &ctx()).is_err(),
            "{bad}"
        );
    }
}

const TEMPLATED: &str = r#"schema_version = 1

[project]
name = "templated"
sample_rate = 48000

[instances]
synth = { slot = "lib.voice" }
out = { module = "std.audio_out" }

[wiring]
connections = []
"#;

#[test]
fn template_ports_are_removed_through_remove_and_detach_writes_them_inline() {
    // Adding to a template's slot keeps `from`.
    let (text, _) = round_trip(
        TEMPLATED,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "synth",
            "add": [{"name": "cv", "role": "input", "kind": "control"}]}),
        )],
    );
    assert!(
        text.contains("synth = { slot = { from = \"lib.voice\", inputs = { cv = \"control\" } } }"),
        "{text}"
    );
    plans(&text).unwrap();
    // Removing a template's port names it in `remove`.
    let (text, _) = round_trip(
        TEMPLATED,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "synth", "remove": ["velocity"]}),
        )],
    );
    assert!(
        text.contains("synth = { slot = { from = \"lib.voice\", remove = [\"velocity\"] } }"),
        "{text}"
    );
    plans(&text).unwrap();
    // Adding it back takes it out of `remove`.
    let patched = apply(
        &text,
        None,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "synth",
            "add": [{"name": "velocity", "role": "input", "kind": "control"}]}),
        )],
        &ctx(),
    )
    .unwrap();
    assert!(!patched.text.contains("remove"), "{}", patched.text);
    plans(&patched.text).unwrap();
    // Detach: the resolved interface inline, no `from`.
    let (text, _) = round_trip(
        TEMPLATED,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "synth", "detach": true}),
        )],
    );
    assert!(!text.contains("from"), "{text}");
    assert!(text.contains("outputs = { out = \"audio\" }"), "{text}");
    plans(&text).unwrap();
    // Set to a template alone.
    let (text, _) = round_trip(
        HOST,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "fx", "template": "lib.instrument"}),
        )],
    );
    assert!(text.contains("fx = { slot = \"lib.instrument\""), "{text}");
    // Detaching a slot without a template, or naming no known one: refused.
    for bad in [
        serde_json::json!({"op": "slot.interface", "slot": "fx", "detach": true}),
        serde_json::json!({"op": "slot.interface", "slot": "fx", "template": "lib.nothing"}),
    ] {
        assert!(
            apply(HOST, None, &[op(bad.clone())], &ctx()).is_err(),
            "{bad}"
        );
    }
}

const SECTIONS: &str = r#"schema_version = 1

[project]
name = "sections"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc" }
out = { module = "std.audio_out" }

# The bus, as a table.
[instances.bus]
load = "std.gain"
map = { x = "in" }

[instances.bus.slot]
inputs = { x = "audio" }  # what comes in
outputs = { out = "audio" }

[wiring]
connections = [
  { from = "osc.out", to = "bus.x" },
  { from = "bus.out", to = "out.l" },
]
"#;

#[test]
fn a_slot_written_as_sections_is_edited_in_place_with_its_comments() {
    plans(SECTIONS).unwrap();
    let (text, _) = round_trip(
        SECTIONS,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "bus",
            "add": [{"name": "side", "role": "input", "kind": "audio"}]}),
        )],
    );
    assert!(
        text.contains("inputs = { x = \"audio\", side = \"audio\" }  # what comes in"),
        "{text}"
    );
    assert!(text.contains("# The bus, as a table."), "{text}");
    plans(&text).unwrap();
    let (text, _) = round_trip(
        SECTIONS,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "bus", "rename": {"x": "input"}}),
        )],
    );
    assert!(
        text.contains("{ from = \"osc.out\", to = \"bus.input\" }"),
        "{text}"
    );
    plans(&text).unwrap();
    let (text, _) = round_trip(
        SECTIONS,
        &[op(
            serde_json::json!({"op": "slot.load", "slot": "bus", "load": "std.mix", "map": {"x": "a"}}),
        )],
    );
    assert!(
        text.contains("load = \"std.mix\"\nmap = { x = \"a\" }\n"),
        "{text}"
    );
    plans(&text).unwrap();
}

#[test]
fn an_untrusted_restore_writes_only_the_slots_own_keys() {
    for bad in [
        serde_json::json!({"op": "slot.restore", "slot": "fx", "changes": [{"path": ["module"], "value": "\"std.mix\""}]}),
        serde_json::json!({"op": "slot.restore", "slot": "fx", "changes": [{"path": ["gpu"], "value": "\"metal\""}]}),
        serde_json::json!({"op": "slot.restore", "slot": "osc", "changes": [{"path": ["load"], "value": "\"std.mix\""}]}),
        serde_json::json!({"op": "slot.restore", "slot": "fx", "changes": [{"path": ["load"], "value": "not toml ="}]}),
        serde_json::json!({"op": "slot.restore", "slot": "fx", "changes": [{"path": ["slot"]}]}),
        serde_json::json!({"op": "slot.restore", "slot": "fx", "wires": {"x": "Bad"}}),
    ] {
        assert!(
            apply(HOST, None, &[op(bad.clone())], &ctx()).is_err(),
            "{bad}"
        );
    }
    // A restore of its own keys is a plain edit.
    let patched = apply(
        HOST,
        None,
        &[op(serde_json::json!({"op": "slot.restore", "slot": "fx", "changes": [{"path": ["load"], "value": "\"std.mix\""}]}))],
        &ctx(),
    )
    .unwrap();
    assert!(patched.text.contains("load = \"std.mix\""));
}

#[test]
fn a_removed_slot_is_put_back_by_the_instance_undo() {
    let patched = apply(
        HOST,
        None,
        &[op(
            serde_json::json!({"op": "instance.remove", "name": "fx"}),
        )],
        &ctx(),
    )
    .unwrap();
    assert!(!patched.text.contains("fx"));
    let undone = apply(&patched.text, None, &patched.inverse, &ctx()).unwrap();
    assert_eq!(undone.text, HOST);
    let redone = apply(&undone.text, None, &undone.inverse, &ctx()).unwrap();
    assert_eq!(redone.text, patched.text);
}

fn plan_dump(text: &str) -> String {
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".into(),
        text: text.to_owned(),
    }])
    .unwrap();
    let planned = project.plan(&registry()).unwrap();
    serde_json::to_string(&planned.plan).unwrap()
}

/// ADR 0037 test 4: a rename (its wires and map key following) plans
/// byte-identically, and so does an edit that changes no match (a port's
/// group, a port nothing matches added); a retype that changes nothing but
/// presentation too.
#[test]
fn an_edit_that_changes_no_match_plans_byte_identically() {
    let before = plan_dump(HOST);
    for edit in [
        serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"x": "input", "y": "wet"}}),
        serde_json::json!({"op": "slot.interface", "slot": "fx", "retype": [{"name": "y", "group": "out", "unit": "V"}]}),
        serde_json::json!({"op": "slot.interface", "slot": "fx", "add": [{"name": "side", "role": "input", "kind": "audio"}]}),
        serde_json::json!({"op": "slot.interface", "slot": "fx", "add": [{"name": "volume", "role": "parameter"}]}),
    ] {
        let patched = apply(HOST, None, &[op(edit.clone())], &ctx()).unwrap();
        assert_eq!(plan_dump(&patched.text), before, "{edit}");
    }
    // Unloading changes the plan; loading the same content back restores it.
    let empty = apply(
        HOST,
        None,
        &[op(serde_json::json!({"op": "slot.unload", "slot": "fx"}))],
        &ctx(),
    )
    .unwrap();
    assert_ne!(plan_dump(&empty.text), before);
    let again = apply(
        &empty.text,
        None,
        &[op(
            serde_json::json!({"op": "slot.load", "slot": "fx", "load": "std.gain"}),
        )],
        &ctx(),
    )
    .unwrap();
    assert_eq!(plan_dump(&again.text), before);
}

const BY_NAME: &str = r#"schema_version = 1

[project]
name = "by-name"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc" }
fx = { slot = { inputs = { in = "audio" }, outputs = { out = "audio" } }, load = "std.gain" }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "fx.in" },
  { from = "fx.out", to = "out.l" },
]
"#;

/// A port matched by name keeps its match when renamed: `map` takes the
/// content's name, so the plan is the same (ADR 0037 test 4); the reply
/// says so. Undone byte for byte.
#[test]
fn renaming_a_port_matched_by_name_keeps_its_match() {
    let before = plan_dump(BY_NAME);
    let (text, patched) = round_trip(
        BY_NAME,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"in": "input", "out": "wet"}}),
        )],
    );
    assert!(
        text.contains("map = { input = \"in\", wet = \"out\" }"),
        "{text}"
    );
    assert_eq!(patched.notes.len(), 2, "{:?}", patched.notes);
    assert_eq!(plan_dump(&text), before);
    // A rename and a direction change in one op: the wires on the old name count.
    let refused = apply(
        BY_NAME,
        None,
        &[op(serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"in": "x"}, "retype": [{"name": "x", "role": "output"}]}))],
        &ctx(),
    )
    .unwrap_err();
    assert_eq!(refused.code, "in-use");
}

/// A port the level's `[exports]` name is not removed (`in-use`, naming them).
#[test]
fn an_exported_slot_port_is_not_removed() {
    let text = BY_NAME.replace(
        "[wiring]",
        "[exports]\noutputs = { main = \"fx.out\" }\n\n[wiring]",
    );
    let refused = apply(
        &text,
        None,
        &[op(serde_json::json!({"op": "slot.interface", "slot": "fx", "remove": ["out"], "removeWires": true}))],
        &ctx(),
    )
    .unwrap_err();
    assert_eq!(refused.code, "in-use");
    assert!(
        refused.message.contains("exports.outputs.main"),
        "{}",
        refused.message
    );
}

/// A rename whose new name another rename of the same op takes or gives
/// is refused (chain them as separate ops); across ops, each op reads the
/// file as the one before left it, so the map follows.
#[test]
fn chained_renames_are_separate_ops() {
    for bad in [
        serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"x": "z", "z": "w"}}),
        serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"x": "y", "y": "x"}}),
    ] {
        let refused = apply(HOST, None, &[op(bad.clone())], &ctx()).unwrap_err();
        assert!(
            refused.message.contains("renamed and renamed to"),
            "{bad}: {}",
            refused.message
        );
    }
    let (text, patched) = round_trip(
        HOST,
        &[
            op(serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"x": "z"}})),
            op(serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"z": "w"}})),
        ],
    );
    assert!(text.contains("map = { w = \"in\", y = \"out\" }"), "{text}");
    assert!(text.contains("to = \"fx.w\""), "{text}");
    assert_eq!(
        patched.renamed,
        [
            ("fx.x".to_owned(), "fx.z".to_owned()),
            ("fx.z".to_owned(), "fx.w".to_owned())
        ]
    );
    plans(&text).unwrap();
}

/// ADR 0041 §8: a renamed or removed interface parameter's
/// `snapshot_exclude` entry follows it; the slot's names are checked
/// against its interface.
#[test]
fn snapshot_exclude_follows_the_interface() {
    let text = HOST.replace(
        r#"outputs = { y = "audio" } }, load = "std.gain", map = { x = "in", y = "out" } }"#,
        r#"outputs = { y = "audio" }, parameters = { volume = {} } }, load = "std.gain", map = { x = "in", y = "out", volume = "db" }, volume = -6, snapshot_exclude = ["volume"] }"#,
    );
    plans(&text).unwrap();
    let (renamed, patched) = round_trip(
        &text,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "fx", "rename": {"volume": "amount"}}),
        )],
    );
    assert!(
        renamed.contains("snapshot_exclude = [\"amount\"]"),
        "{renamed}"
    );
    assert!(renamed.contains("amount = -6"), "{renamed}");
    assert!(patched.removed.is_empty());
    plans(&renamed).unwrap();
    let (removed, patched) = round_trip(
        &text,
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "fx", "remove": ["volume"]}),
        )],
    );
    assert!(!removed.contains("snapshot_exclude"), "{removed}");
    assert_eq!(patched.removed, ["fx.volume"]);
    plans(&removed).unwrap();
    let wrong = text.replace(
        r#"snapshot_exclude = ["volume"]"#,
        r#"snapshot_exclude = ["db"]"#,
    );
    let error = plans(&wrong).unwrap_err();
    assert!(
        error.contains("no parameter of the slot's interface"),
        "{error}"
    );
}

const COMMENTED: &str = r#"schema_version = 1

[project]
name = "commented"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc" }
out = { module = "std.audio_out" }

[instances.bus]
# what it plays
load = "std.gain"  # for now
# how it maps
map = { x = "in" }
seed_note = 1

[instances.bus.slot]
inputs = { x = "audio" }
outputs = { out = "audio" }

[wiring]
connections = [
  { from = "osc.out", to = "bus.x" },
]
"#;

/// A key removed from a section takes its comment lines and trailing
/// comment with it, and its undo puts them back byte for byte.
#[test]
fn a_removed_key_s_comments_come_back_with_its_undo() {
    let text = COMMENTED.replace("seed_note = 1\n", "");
    plans(&text).unwrap();
    let (unloaded, _) = round_trip(
        &text,
        &[op(serde_json::json!({"op": "slot.unload", "slot": "bus"}))],
    );
    assert!(
        !unloaded.contains("what it plays") && !unloaded.contains("for now"),
        "{unloaded}"
    );
    assert!(
        unloaded.contains("# how it maps\nmap = { x = \"in\" }"),
        "{unloaded}"
    );
    let (unmapped, _) = round_trip(
        &text,
        &[op(
            serde_json::json!({"op": "slot.map", "slot": "bus", "map": {}}),
        )],
    );
    assert!(
        unmapped.contains("load = \"std.gain\"  # for now\n\n[instances.bus.slot]"),
        "{unmapped}"
    );
}

/// A restore that makes the tables it writes into removes them again on
/// its undo: no empty table is left.
#[test]
fn a_restore_s_undo_removes_the_tables_it_made() {
    let text = HOST.replace(r#", map = { x = "in", y = "out" }"#, "");
    assert!(!text.contains("map ="));
    let (written, _) = round_trip(
        &text,
        &[op(
            serde_json::json!({"op": "slot.restore", "slot": "fx", "changes": [
                {"path": ["map", "x"], "value": "\"in\""},
                {"path": ["slot", "parameters", "gain"], "value": "{}"},
            ]}),
        )],
    );
    assert!(written.contains("map = { x = \"in\" }"), "{written}");
    // The round trip checked the undo byte for byte: no `map = {}` stays.
}
