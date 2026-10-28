//! ADR 0037, editable insides (2026-10-27): `slot.inside` makes a slot's own
//! inside from each kind of content (nothing, a project composite, a library
//! composite, a primitive wrapped), named deterministically and free, its
//! exports named after the slot's ports, so the plan does not change (but
//! for the wrapped primitive's one level more); every op is undone byte for
//! byte in both files and redone; an interface edit of a slot holding its
//! own inside renames or removes the inside's export with it; the export
//! ops wire a composite's frame and are undone byte for byte; the
//! refusals (a project, an inside already its own, an untrusted
//! `composite.file` on another composite, an export op at the top level,
//! a second source for an output).

use std::path::Path;

use super::*;
use crate::inside;
use crate::{Project, SourceFile};

type Files = BTreeMap<String, String>;

fn registry() -> signal_core::Registry {
    let mut registry = signal_core::Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn sources(files: &Files) -> Vec<SourceFile> {
    files
        .iter()
        .map(|(path, text)| SourceFile {
            path: path.clone(),
            text: text.clone(),
        })
        .collect()
}

fn op(json: serde_json::Value) -> Op {
    serde_json::from_value(json).unwrap()
}

fn level_path(level: &str) -> String {
    if level.is_empty() {
        "project.toml".to_owned()
    } else {
        format!("composites/{level}.toml")
    }
}

/// `ops` at `level` on `files`, as the host applies them: the level's file
/// and the patch's other files written.
fn run(files: &Files, level: &str, ops: &[Op]) -> Result<(Files, Patched), PatchError> {
    let sources = sources(files);
    let project = Project::from_sources(sources.clone()).ok();
    let registry = registry();
    let module = |m: &str| -> Option<Vec<String>> {
        if let Some(t) = registry.get(m) {
            return Some(
                t.descriptor
                    .parameters
                    .iter()
                    .map(|p| p.name.to_owned())
                    .collect(),
            );
        }
        project
            .as_ref()?
            .composites
            .iter()
            .find(|c| c.name == m)
            .map(|c| {
                c.exports
                    .parameters
                    .iter()
                    .map(|e| e.name.clone())
                    .collect()
            })
    };
    let content =
        |load: &str| crate::content_names(project.as_ref(), Path::new("."), load, &registry);
    let file = |path: &str| files.get(path).cloned();
    let source = |load: &str| inside::source(&sources, load, &registry);
    let templates = crate::interface_templates(&sources);
    let ctx = Context {
        level,
        module: &module,
        templates: &templates,
        content: &content,
        file: &file,
        source: &source,
    };
    let path = level_path(level);
    let patched = apply(&files[&path], None, ops, &ctx)?;
    let mut out = files.clone();
    out.insert(path, patched.text.clone());
    for change in &patched.files {
        match &change.after {
            Some(text) => out.insert(change.path.clone(), text.clone()),
            None => out.remove(&change.path),
        };
    }
    Ok((out, patched))
}

/// `ops`, their undo (through JSON, as the page sends it back) giving every
/// file back byte for byte, and the redo the patched files again.
fn round_trip(files: &Files, level: &str, ops: &[Op]) -> (Files, Patched) {
    let (after, patched) = run(files, level, ops).unwrap_or_else(|e| panic!("{ops:?}: {e}"));
    let inverse: Vec<Op> =
        serde_json::from_str(&serde_json::to_string(&patched.inverse).unwrap()).unwrap();
    let (undone, back) = run(&after, level, &inverse).unwrap_or_else(|e| panic!("undo: {e}"));
    assert_eq!(undone, *files, "the undo of {ops:?} is not byte-exact");
    let (redone, _) = run(&undone, level, &back.inverse).unwrap_or_else(|e| panic!("redo: {e}"));
    assert_eq!(redone, after, "the redo of {ops:?} differs");
    (after, patched)
}

fn project(files: &Files) -> Project {
    Project::from_sources(sources(files)).unwrap_or_else(|d| panic!("{d:#?}"))
}

fn plan_dump(files: &Files) -> String {
    let planned = project(files)
        .plan(&registry())
        .unwrap_or_else(|d| panic!("{d:#?}"));
    serde_json::to_string(&planned.plan).unwrap()
}

fn files(project: &str, more: &[(&str, &str)]) -> Files {
    let mut files = Files::new();
    files.insert("project.toml".to_owned(), project.to_owned());
    for (path, text) in more {
        files.insert((*path).to_owned(), (*text).to_owned());
    }
    files
}

const EMPTY: &str = r#"schema_version = 1

[project]
name = "insides"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc", freq = 220 }
fx = { slot = { inputs = { in = "audio" }, outputs = { out = "audio" } } }   # to be built
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "fx.in" },
  { from = "fx.out", to = "out.l" },
]
"#;

/// An empty slot's inside: an empty composite named `fx_inside` with the
/// slot as its owner, loaded; the plan is the empty slot's. Then assembled
/// inside (a module, the frame wired to it), each step undone byte for byte.
#[test]
fn an_empty_slot_gets_an_empty_inside_to_build_in() {
    let start = files(EMPTY, &[]);
    let before = plan_dump(&start);
    let (made, patched) = round_trip(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "fx"}))],
    );
    assert_eq!(
        made["composites/fx_inside.toml"],
        "schema_version = 1\n\n[composite]\nname = \"fx_inside\"\ninside_of = \"fx\"\n\n[instances]\n"
    );
    assert!(
        made["project.toml"].contains(
            "fx = { slot = { inputs = { in = \"audio\" }, outputs = { out = \"audio\" } }, load = \"fx_inside\" }   # to be built"
        ),
        "{}",
        made["project.toml"]
    );
    assert_eq!(patched.files.len(), 1);
    assert_eq!(patched.files[0].before, None);
    assert!(
        patched
            .notes
            .iter()
            .any(|n| n.contains("composites/fx_inside.toml"))
    );
    assert_eq!(
        plan_dump(&made),
        before,
        "an empty inside plans as the empty slot"
    );
    // Assembled inside: a gain, its input and output on the frame's pins.
    let (with_gain, _) = round_trip(
        &made,
        "fx_inside",
        &[op(
            serde_json::json!({"op": "instance.add", "name": "amp", "module": "std.gain"}),
        )],
    );
    let (wired, _) = round_trip(
        &with_gain,
        "fx_inside",
        &[
            op(
                serde_json::json!({"op": "export.connect", "role": "input", "name": "in", "port": "amp.in"}),
            ),
            op(
                serde_json::json!({"op": "export.connect", "role": "output", "name": "out", "port": "amp.out"}),
            ),
        ],
    );
    assert!(
        wired["composites/fx_inside.toml"].ends_with(
            "[exports]\ninputs = { in = \"amp.in\" }\noutputs = { out = \"amp.out\" }\n"
        ),
        "{}",
        wired["composites/fx_inside.toml"]
    );
    let planned = project(&wired).plan(&registry()).unwrap();
    let fx = planned.slots.iter().find(|s| s.slot == "/fx").unwrap();
    assert!(
        fx.matches
            .iter()
            .all(|m| m.content.as_deref() == Some(m.port.as_str()))
    );
    assert!(
        planned
            .plan
            .instances
            .iter()
            .any(|i| i.address.as_str() == "/fx/amp")
    );
    // A second input on the frame fans out; a second source for an output is refused.
    let (fanned, _) = round_trip(
        &wired,
        "fx_inside",
        &[
            op(serde_json::json!({"op": "instance.add", "name": "amp2", "module": "std.gain"})),
            op(
                serde_json::json!({"op": "export.connect", "role": "input", "name": "in", "port": "amp2.in"}),
            ),
        ],
    );
    assert!(
        fanned["composites/fx_inside.toml"].contains("inputs = { in = [\"amp.in\", \"amp2.in\"] }")
    );
    let refused = run(
        &fanned,
        "fx_inside",
        &[op(serde_json::json!({"op": "export.connect", "role": "output", "name": "out", "port": "amp2.out"}))],
    )
    .unwrap_err();
    assert_eq!(refused.code, "bad-value");
    // Disconnected again, undone byte for byte.
    let (single, _) = round_trip(
        &fanned,
        "fx_inside",
        &[op(
            serde_json::json!({"op": "export.disconnect", "role": "input", "name": "in", "port": "amp2.in"}),
        )],
    );
    assert!(single["composites/fx_inside.toml"].contains("inputs = { in = \"amp.in\" }"));
}

const VOICE: &str = r#"schema_version = 1

[composite]
name = "voice"
description = "A sine through a gain."

[instances]
osc = { module = "std.sine_osc" }   # the source
amp = { module = "std.gain", db = -6 }

[wiring]
connections = [{ from = "osc.out", to = "amp.in" }]

[exports]
outputs = { out = "amp.out" }

# Its controls.
[exports.parameters]
freq = { to = "osc.freq", min = 20, max = 2000, group = "pitch" }
db = "amp.db"
"#;

const PAD: &str = r#"schema_version = 1

[project]
name = "pads"
sample_rate = 48000

[instances]
pad = { slot = { outputs = { l = "audio", r = "audio" }, parameters = { pitch = {} } }, load = "voice", map = { l = "out", r = "out", pitch = "freq" }, pitch = 330 }
other = { module = "voice" }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "pad.l", to = "out.l" },
  { from = "pad.r", to = "out.r" },
  { from = "other.out", to = "out.l", scale = 0.5 },
]
"#;

/// A project composite's copy: its text kept (comments, ranges, groups),
/// renamed, owned, its exports named after the slot's ports (one output
/// fanned to `l` and `r`, `freq` as `pitch`), the slot's `map` gone; the
/// original and its other user untouched; the plan byte-identical.
#[test]
fn a_project_composite_is_copied_and_plans_the_same() {
    let start = files(PAD, &[("composites/voice.toml", VOICE)]);
    let before = plan_dump(&start);
    let (made, _) = round_trip(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "pad"}))],
    );
    let copy = &made["composites/pad_inside.toml"];
    assert!(copy.contains("name = \"pad_inside\"\n"), "{copy}");
    assert!(copy.contains("inside_of = \"pad\""), "{copy}");
    assert!(
        copy.contains("osc = { module = \"std.sine_osc\" }   # the source"),
        "{copy}"
    );
    assert!(
        copy.contains("outputs = { l = \"amp.out\", r = \"amp.out\" }"),
        "{copy}"
    );
    assert!(
        copy.contains("# Its controls.\n[exports.parameters]"),
        "{copy}"
    );
    assert!(
        copy.contains("pitch = { to = \"osc.freq\", min = 20, max = 2000, group = \"pitch\" }"),
        "{copy}"
    );
    assert!(copy.contains("db = \"amp.db\""), "{copy}");
    assert_eq!(made["composites/voice.toml"], VOICE);
    assert!(
        made["project.toml"].contains(
            "pad = { slot = { outputs = { l = \"audio\", r = \"audio\" }, parameters = { pitch = {} } }, load = \"pad_inside\", pitch = 330 }"
        ),
        "{}",
        made["project.toml"]
    );
    assert_eq!(plan_dump(&made), before);
    // Its own now: a second `slot.inside` is refused.
    let again = run(
        &made,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "pad"}))],
    )
    .unwrap_err();
    assert_eq!(again.code, "bad-value");
    assert!(
        again.message.contains("own inside already"),
        "{}",
        again.message
    );
}

/// The interface kept in step with the own inside: a rename renames the
/// inside's export (no `map` written) and plans the same; a removal (with
/// its wires) removes it; both files undone byte for byte.
#[test]
fn the_interface_and_the_own_inside_move_together() {
    let start = files(PAD, &[("composites/voice.toml", VOICE)]);
    let (own, _) = run(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "pad"}))],
    )
    .unwrap();
    let before = plan_dump(&own);
    let (renamed, patched) = round_trip(
        &own,
        "",
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "pad", "rename": {"l": "left", "pitch": "hz"}}),
        )],
    );
    let inside = &renamed["composites/pad_inside.toml"];
    assert!(
        inside.contains("outputs = { left = \"amp.out\", r = \"amp.out\" }"),
        "{inside}"
    );
    assert!(inside.contains("hz = { to = \"osc.freq\""), "{inside}");
    assert!(
        !renamed["project.toml"].contains("map"),
        "{}",
        renamed["project.toml"]
    );
    assert!(renamed["project.toml"].contains("{ from = \"pad.left\", to = \"out.l\" }"));
    assert!(renamed["project.toml"].contains("hz = 330"));
    assert_eq!(patched.files.len(), 1);
    assert_eq!(plan_dump(&renamed), before);
    let (removed, _) = round_trip(
        &own,
        "",
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "pad", "remove": ["r"], "removeWires": true}),
        )],
    );
    let inside = &removed["composites/pad_inside.toml"];
    assert!(inside.contains("outputs = { l = \"amp.out\" }"), "{inside}");
    project(&removed).plan(&registry()).unwrap();
    // An added port changes nothing inside: it is unmatched until wired there.
    let (added, patched) = round_trip(
        &own,
        "",
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "pad", "add": [{"name": "fm", "role": "input", "kind": "audio"}]}),
        )],
    );
    assert!(patched.files.is_empty());
    assert_eq!(
        added["composites/pad_inside.toml"],
        own["composites/pad_inside.toml"]
    );
}

/// A library composite's copy: its text into the project as the slot's own,
/// exports by the slot's names; the plan byte-identical.
#[test]
fn a_library_composite_is_copied_and_plans_the_same() {
    let host = r#"schema_version = 1

[project]
name = "echoes"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc" }
echo = { slot = { inputs = { x = "audio" }, outputs = { out = "audio" }, parameters = { time_ms = {} } }, load = "lib.tape_echo", map = { x = "in" }, time_ms = 250 }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "echo.x" },
  { from = "echo.out", to = "out.l" },
]
"#;
    let start = files(host, &[]);
    let before = plan_dump(&start);
    let (made, _) = round_trip(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "echo"}))],
    );
    let copy = &made["composites/echo_inside.toml"];
    let library = crate::Library::builtin().get("tape_echo").unwrap();
    assert!(
        copy.contains("inputs = { x = [\"delay.in\", \"dry.a\"] }"),
        "{copy}"
    );
    assert!(copy.contains("inside_of = \"echo\""), "{copy}");
    assert_eq!(
        copy.lines().filter(|l| l.starts_with('#')).count(),
        library.lines().filter(|l| l.starts_with('#')).count(),
        "its comments kept"
    );
    assert_eq!(plan_dump(&made), before);
}

/// A primitive wrapped: one instance of it named after its type, every
/// port and parameter exported (the slot's matched ones by the slot's
/// names); the slot's `map` gone; it plans one level down (`/fx/gain`).
#[test]
fn a_primitive_is_wrapped() {
    let host = r#"schema_version = 1

[project]
name = "wrap"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc" }
fx = { slot = { inputs = { x = "audio" }, outputs = { y = "audio" } }, load = "std.gain", map = { x = "in", y = "out" }, set = { db = -3 } }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "fx.x" },
  { from = "fx.y", to = "out.l" },
]
"#;
    let start = files(host, &[]);
    let (made, _) = round_trip(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "fx"}))],
    );
    assert_eq!(
        made["composites/fx_inside.toml"],
        "schema_version = 1\n\n[composite]\nname = \"fx_inside\"\ninside_of = \"fx\"\n\n[instances]\ngain = { module = \"std.gain\" }\n\n[exports]\ninputs = { x = \"gain.in\" }\noutputs = { y = \"gain.out\" }\nparameters = { db = \"gain.db\" }\n"
    );
    assert!(made["project.toml"].contains("load = \"fx_inside\", set = { db = -3 } }"));
    let planned = project(&made).plan(&registry()).unwrap();
    let addresses: Vec<&str> = planned
        .plan
        .instances
        .iter()
        .map(|i| i.address.as_str())
        .collect();
    assert!(addresses.contains(&"/fx/gain"), "{addresses:?}");
    assert!(!addresses.contains(&"/fx"), "{addresses:?}");
}

/// Names are deterministic and free: `<slot>_inside`, then `_2`; a slot in
/// a composite `<composite>_<slot>_inside`; a given name that is taken (or a
/// library composite's) is refused.
#[test]
fn inside_names_are_deterministic_and_free() {
    let taken = |n: &str| ["fx_inside", "fx_inside_2", "tape_echo"].contains(&n);
    assert_eq!(inside::default_name("", "fx", &taken), "fx_inside_3");
    assert_eq!(inside::default_name("rack", "fx", &taken), "rack_fx_inside");
    assert_eq!(inside::owner("rack", "fx"), "rack/fx");
    assert!(inside::valid_owner("rack/fx") && inside::valid_owner("fx"));
    assert!(!inside::valid_owner("/fx") && !inside::valid_owner("a/b/c"));
    let start = files(
        EMPTY,
        &[(
            "composites/fx_inside.toml",
            "schema_version = 1\n\n[composite]\nname = \"fx_inside\"\n",
        )],
    );
    let (made, _) = run(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "fx"}))],
    )
    .unwrap();
    assert!(made.contains_key("composites/fx_inside_2.toml"));
    for name in ["fx_inside", "tape_echo", "Bad"] {
        let refused = run(
            &start,
            "",
            &[op(
                serde_json::json!({"op": "slot.inside", "slot": "fx", "name": name}),
            )],
        )
        .unwrap_err();
        assert_eq!(refused.code, "bad-value", "{name}");
    }
    let (named, _) = run(
        &start,
        "",
        &[op(
            serde_json::json!({"op": "slot.inside", "slot": "fx", "name": "my_fx"}),
        )],
    )
    .unwrap();
    assert!(named["composites/my_fx.toml"].contains("inside_of = \"fx\""));
}

/// The refusals: a project in a slot is not copied; an untrusted
/// `composite.file` writes only an own inside, and only while it holds
/// `was`; an export op at the top level.
#[test]
fn what_is_refused() {
    let project_slot = EMPTY.replace(
        "fx = { slot = { inputs = { in = \"audio\" }, outputs = { out = \"audio\" } } }",
        "fx = { slot = { inputs = { in = \"audio\" }, outputs = { out = \"audio\" } }, load = \"../other\" }",
    );
    let start = files(&project_slot, &[("composites/voice.toml", VOICE)]);
    let refused = run(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "fx"}))],
    )
    .unwrap_err();
    assert_eq!(refused.code, "bad-value");
    assert!(
        refused.message.contains("its own folder"),
        "{}",
        refused.message
    );
    let start = files(EMPTY, &[("composites/voice.toml", VOICE)]);
    for json in [
        serde_json::json!({"op": "composite.file", "name": "voice", "was": VOICE}),
        serde_json::json!({"op": "composite.file", "name": "voice", "text": "x", "was": VOICE}),
        serde_json::json!({"op": "composite.file", "name": "new_one", "text": VOICE}),
        serde_json::json!({"op": "composite.file", "name": "voice", "text": "schema_version = 1\n[composite]\nname = \"voice\"\ninside_of = \"fx\"\n", "was": "other"}),
        // An inside of a slot elsewhere, of no slot here, under a library name, or handed to another slot.
        serde_json::json!({"op": "composite.file", "name": "other_inside", "text": "schema_version = 1\n[composite]\nname = \"other_inside\"\ninside_of = \"rack/fx\"\n"}),
        serde_json::json!({"op": "composite.file", "name": "other_inside", "text": "schema_version = 1\n[composite]\nname = \"other_inside\"\ninside_of = \"osc_x\"\n"}),
        serde_json::json!({"op": "composite.file", "name": "tape_echo", "text": "schema_version = 1\n[composite]\nname = \"tape_echo\"\ninside_of = \"fx\"\n"}),
        serde_json::json!({"op": "export.connect", "role": "input", "name": "in", "port": "osc.pm"}),
    ] {
        let refused = run(&start, "", &[op(json.clone())]).unwrap_err();
        assert_eq!(refused.code, "bad-value", "{json}");
    }
    // At a composite level, an export op writes only `[exports]`; an unknown instance is refused.
    let refused = run(
        &start,
        "voice",
        &[op(serde_json::json!({"op": "export.connect", "role": "input", "name": "pm", "port": "nobody.pm"}))],
    )
    .unwrap_err();
    assert_eq!(refused.code, "unknown-instance");
    let refused = run(
        &start,
        "voice",
        &[op(serde_json::json!({"op": "export.restore", "changes": [{"path": ["instances", "osc"], "value": "{ module = \"std.gain\" }"}]}))],
    )
    .unwrap_err();
    assert_eq!(refused.code, "bad-value");
}

/// Export ops on a section-written `[exports.parameters]` and on a file
/// without `[exports]` are undone byte for byte.
#[test]
fn export_ops_round_trip_on_every_form() {
    let start = files(PAD, &[("composites/voice.toml", VOICE)]);
    let (added, _) = round_trip(
        &start,
        "voice",
        &[op(
            serde_json::json!({"op": "export.connect", "role": "parameter", "name": "db", "port": "osc.freq"}),
        )],
    );
    assert!(added["composites/voice.toml"].contains("db = [\"amp.db\", \"osc.freq\"]"));
    round_trip(
        &start,
        "voice",
        &[op(
            serde_json::json!({"op": "export.disconnect", "role": "parameter", "name": "freq", "port": "osc.freq"}),
        )],
    );
    round_trip(
        &start,
        "voice",
        &[op(
            serde_json::json!({"op": "export.connect", "role": "input", "name": "pm", "port": "osc.pm"}),
        )],
    );
    let bare = VOICE.split("\n[exports]").next().unwrap().to_owned() + "\n";
    let start = files(
        &PAD.replace(
            ", load = \"voice\", map = { l = \"out\", r = \"out\", pitch = \"freq\" }, pitch = 330",
            "",
        )
        .replace("other = { module = \"voice\" }\n", "")
        .replace(
            "  { from = \"other.out\", to = \"out.l\", scale = 0.5 },\n",
            "",
        ),
        &[("composites/voice.toml", &bare)],
    );
    let (wired, _) = round_trip(
        &start,
        "voice",
        &[op(
            serde_json::json!({"op": "export.connect", "role": "output", "name": "out", "port": "amp.out"}),
        )],
    );
    assert!(
        wired["composites/voice.toml"].contains("\n[exports]\noutputs = { out = \"amp.out\" }\n"),
        "{}",
        wired["composites/voice.toml"]
    );
}

fn graph(files: &Files) -> serde_json::Value {
    let project = project(files);
    let planned = project.plan(&registry()).unwrap();
    project.structure(&planned.plan, &registry())
}

fn slot_box<'g>(graph: &'g serde_json::Value, name: &str) -> &'g serde_json::Value {
    graph["top"]["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == name)
        .unwrap()
}

/// `graph` (additive): each slot box's `slot.inside`; a read-only inside a
/// slot would get (`insides`, by `/slot`), with the slot's interface on its
/// frame; a slot's own inside in `composites` with that frame, `slot` its key.
#[test]
fn graph_gives_each_slot_its_inside() {
    let empty = graph(&files(EMPTY, &[]));
    assert_eq!(
        slot_box(&empty, "fx")["slot"]["inside"],
        serde_json::json!({"level": "/fx", "own": false, "make": "empty", "name": "fx_inside"})
    );
    let inside = &empty["insides"]["/fx"];
    assert_eq!(inside["instances"], serde_json::json!([]));
    assert_eq!(inside["inputs"][0]["name"], "in");
    assert_eq!(inside["inputs"][0]["to"], serde_json::json!([]));
    assert_eq!(inside["inputs"][0]["unmatched"], true);
    assert_eq!(inside["outputs"][0]["name"], "out");
    assert_eq!(inside["slot"], "/fx");

    let shared = files(PAD, &[("composites/voice.toml", VOICE)]);
    let before = graph(&shared);
    assert_eq!(slot_box(&before, "pad")["slot"]["inside"]["make"], "copy");
    let preview = &before["insides"]["/pad"];
    let names = |list: &serde_json::Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names(&preview["outputs"]), ["l", "r"]);
    assert_eq!(preview["outputs"][0]["from"], "amp.out");
    // What the copy will be: its exports by the slot's names (no `export`).
    assert!(preview["outputs"][0].get("export").is_none());
    // `db`, which no port of the slot reaches, follows them, marked.
    assert_eq!(names(&preview["parameters"]), ["pitch", "db"]);
    assert_eq!(
        preview["parameters"][0]["to"],
        serde_json::json!(["osc.freq"])
    );
    assert_eq!(preview["parameters"][1]["extra"], true);
    // The shared composite's own level keeps its exports.
    assert_eq!(names(&before["composites"]["voice"]["outputs"]), ["out"]);

    let (own, _) = run(
        &shared,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "pad"}))],
    )
    .unwrap();
    let after = graph(&own);
    assert_eq!(
        slot_box(&after, "pad")["slot"]["inside"],
        serde_json::json!({"level": "pad_inside", "own": true, "name": "pad_inside"})
    );
    let level = &after["composites"]["pad_inside"];
    assert_eq!(level["slot"], "/pad");
    assert_eq!(names(&level["outputs"]), ["l", "r"]);
    assert!(level["outputs"][0].get("export").is_none());
    assert!(after.get("insides").is_none_or(|i| i.get("/pad").is_none()));

    let gain = EMPTY.replace(
        "fx = { slot = { inputs = { in = \"audio\" }, outputs = { out = \"audio\" } } }",
        "fx = { slot = { inputs = { in = \"audio\" }, outputs = { out = \"audio\" } }, load = \"std.gain\" }",
    );
    let wrapped = graph(&files(&gain, &[]));
    assert_eq!(slot_box(&wrapped, "fx")["slot"]["inside"]["make"], "wrap");
    let inside = &wrapped["insides"]["/fx"];
    assert_eq!(inside["instances"][0]["name"], "gain");
    assert_eq!(inside["inputs"][0]["to"], serde_json::json!(["gain.in"]));
}

/// `[composite] inside_of` is checked for its form only: `<slot>` or
/// `<composite>/<slot>`; another form is an error at the value. A marker
/// naming no slot (an Unload keeps the file) is valid.
#[test]
fn inside_of_is_checked_for_its_form() {
    let composite = |owner: &str| {
        format!(
            "schema_version = 1\n\n[composite]\nname = \"spare\"\ninside_of = \"{owner}\"\n\n[instances]\n"
        )
    };
    for owner in ["fx", "rack/fx", "nobody"] {
        let files = files(EMPTY, &[("composites/spare.toml", &composite(owner))]);
        Project::from_sources(sources(&files)).unwrap_or_else(|d| panic!("{owner}: {d:#?}"));
    }
    for owner in ["/fx", "a/b/c", "Fx", ""] {
        let files = files(EMPTY, &[("composites/spare.toml", &composite(owner))]);
        let errors = Project::from_sources(sources(&files))
            .err()
            .unwrap_or_default();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("invalid `inside_of`")
                    && d.file == "composites/spare.toml"),
            "{owner}: {errors:#?}"
        );
    }
}

/// Review follow-up 1: renaming a slot port onto another export of its own
/// inside (`pitch` → `db`, an export no port reaches) is refused, both files
/// unchanged; `rename_export` never overwrites a key.
#[test]
fn a_rename_onto_an_existing_export_is_refused() {
    let start = files(PAD, &[("composites/voice.toml", VOICE)]);
    let (own, _) = run(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "pad"}))],
    )
    .unwrap();
    let refused = run(
        &own,
        "",
        &[op(
            serde_json::json!({"op": "slot.interface", "slot": "pad", "rename": {"pitch": "db"}}),
        )],
    )
    .unwrap_err();
    assert_eq!(refused.code, "bad-value");
    assert!(
        refused.message.contains("`db` is an export"),
        "{}",
        refused.message
    );
    let inside = &own["composites/pad_inside.toml"];
    assert!(inside::has_export(inside, "db"));
    assert_eq!(inside::rename_export(inside, "pitch", "db"), None);
    assert_eq!(
        inside::rename_export(inside, "pitch", "l"),
        None,
        "across roles too"
    );
}

/// Review follow-up 2: the read-only preview of a project composite loaded
/// through a `map` is the copy (its frame names are the copy's exports), so
/// the first edits the schematic derives from it (a frame wire disconnected;
/// a module deleted with the frame wires on it) apply to the copy.
#[test]
fn first_edits_from_a_mapped_preview_apply_to_the_copy() {
    let start = files(PAD, &[("composites/voice.toml", VOICE)]);
    let preview = graph(&start)["insides"]["/pad"].clone();
    let (own, _) = run(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "pad"}))],
    )
    .unwrap();
    // A frame wire: export name and port as the preview's frame gives them.
    let l = &preview["outputs"][0];
    let wire = serde_json::json!({"op": "export.disconnect", "role": "output",
        "name": l["export"].as_str().or(l["name"].as_str()), "port": l["from"]});
    let (cut, _) = round_trip(&own, "pad_inside", &[op(wire)]);
    project(&cut).plan(&registry()).unwrap();
    // A module deleted: every frame wire on it first, as `insideOps` writes them.
    let mut ops = Vec::new();
    for (list, role) in [("inputs", "input"), ("parameters", "parameter")] {
        for e in preview[list].as_array().unwrap() {
            for target in e["to"].as_array().unwrap() {
                if target.as_str().unwrap().starts_with("amp.") {
                    ops.push(op(
                        serde_json::json!({"op": "export.disconnect", "role": role,
                        "name": e["export"].as_str().or(e["name"].as_str()), "port": target}),
                    ));
                }
            }
        }
    }
    for e in preview["outputs"].as_array().unwrap() {
        if e["from"].as_str().is_some_and(|f| f.starts_with("amp.")) {
            ops.push(op(
                serde_json::json!({"op": "export.disconnect", "role": "output",
                "name": e["export"].as_str().or(e["name"].as_str()), "port": e["from"]}),
            ));
        }
    }
    assert_eq!(ops.len(), 3, "l, r and db reach amp");
    ops.push(op(
        serde_json::json!({"op": "instance.remove", "name": "amp"}),
    ));
    let (gone, _) = round_trip(&own, "pad_inside", &ops);
    project(&gone).plan(&registry()).unwrap();
}

/// Review follow-up 3: a slot's own inside is loaded by that slot only:
/// another slot loading it, or a plain instance of it, is an error at the
/// reference; the owner loading it is fine.
#[test]
fn an_own_inside_is_its_slots_only() {
    let start = files(PAD, &[("composites/voice.toml", VOICE)]);
    let (own, _) = run(
        &start,
        "",
        &[op(serde_json::json!({"op": "slot.inside", "slot": "pad"}))],
    )
    .unwrap();
    // `modules` says whose it is, so the Load dialog lists it for its slot only.
    let modules = project(&own).modules(&registry());
    let entry = |name: &str| {
        modules["composites"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["module"] == name)
            .cloned()
            .unwrap()
    };
    assert_eq!(entry("pad_inside")["inside_of"], "pad");
    assert!(entry("voice").get("inside_of").is_none());
    for (entry, what) in [
        (
            "thief = { slot = { outputs = { l = \"audio\" } }, load = \"pad_inside\" }\n",
            "slot `pad`",
        ),
        ("plain = { module = \"pad_inside\" }\n", "slot `pad`"),
    ] {
        let mut taken = own.clone();
        let text =
            taken["project.toml"].replace("out = { module", &format!("{entry}out = {{ module"));
        taken.insert("project.toml".to_owned(), text);
        let errors = Project::from_sources(sources(&taken))
            .err()
            .unwrap_or_default();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("is the own inside of") && d.message.contains(what)),
            "{entry}: {errors:#?}"
        );
    }
    // Through `slot.load` the edit is refused when the project is loaded.
    let (loaded, _) = run(
        &own,
        "",
        &[op(serde_json::json!({"op": "slot.add", "name": "thief", "slot": {"outputs": {"l": "audio"}}, "load": "pad_inside"}))],
    )
    .unwrap();
    assert!(Project::from_sources(sources(&loaded)).is_err());
}
