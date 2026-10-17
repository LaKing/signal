//! M2.9-3 (ADR 0026 §6): `patch`, `modules` and `graph.files` in `signal
//! ui`, over both transports, on copies of the committed projects (never
//! the projects themselves): every op in one patch, its inverse and that
//! inverse's inverse byte for byte on kick, pulse, physical-av and the kick
//! composite; the errors; `in-use` for an export and for a panel binding;
//! the live reload of a patch heard on the meters; the size refusal; the
//! `graph` push after a commit. (Stopped, and a `.signal` project edited:
//! `app_mode.rs`; the text edit itself: `signal-project`'s patch tests.)

mod common;

use std::fs;

use common::{Options, Rig, Transport, Ws, receive, rig, root, send};
use serde_json::{Value, json};
use signal_ui::panel_write::sha256_hex;

fn project_rig(name: &str, transport: Transport, project: &str) -> Rig {
    rig(
        name,
        Options {
            transport,
            project: Some(root().join("projects").join(project)),
            ..Options::default()
        },
    )
}

/// The reply to `id`, whatever its type (a `patched` or an `error`).
fn answer(rig: &mut Rig, ws: &mut Ws, id: u64, message: Value) -> Value {
    let mut message = message;
    message["id"] = json!(id);
    send(ws, message);
    rig.wait(ws, "the reply", |got| {
        got.json()
            .filter(|v| v.get("id") == Some(&json!(id)))
            .cloned()
    })
}

fn base(rig: &mut Rig, ws: &mut Ws, id: u64, level: &str) -> String {
    let graph = rig.request(ws, id, json!({"type": "graph"}), "graph");
    graph["files"][level].as_str().unwrap().to_owned()
}

fn patch(level: &str, base: &str, ops: Value) -> Value {
    json!({"type": "patch", "level": level, "base": base, "ops": ops})
}

/// Every op of the issue in one patch on a copy of `project` at `level`,
/// then its inverse, then that inverse's inverse: the file byte for byte,
/// and `ui/schematic.toml` (absent in the copies) gone again after the undo.
fn round_trip(transport: Transport, project: &str, level: &str, ops: Value) {
    let mut rig = project_rig(&format!("patch-{project}-{level}"), transport, project);
    let (mut ws, _) = rig.client();
    let file = if level.is_empty() {
        rig.dir.join("project.toml")
    } else {
        rig.dir.join(format!("composites/{level}.toml"))
    };
    let original = fs::read_to_string(&file).unwrap();
    let project_toml = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    let schematic = rig.dir.join("ui/schematic.toml");
    assert!(!schematic.exists());
    let first = base(&mut rig, &mut ws, 1, level);
    assert_eq!(first, sha256_hex(original.as_bytes()));
    let patched = rig.request(&mut ws, 2, patch(level, &first, ops), "patched");
    let after = fs::read_to_string(&file).unwrap();
    assert_ne!(after, original);
    assert_eq!(patched["base"], sha256_hex(after.as_bytes()));
    assert_eq!(patched["level"], level);
    assert!(
        fs::read_to_string(&schematic)
            .unwrap()
            .contains("svf = { x = 100, y = -40 }")
    );
    if !level.is_empty() {
        assert_eq!(
            fs::read_to_string(rig.dir.join("project.toml")).unwrap(),
            project_toml,
            "a composite patch writes its own file only"
        );
    }
    // Undo: the inverse, with the new base.
    let undone = rig.request(
        &mut ws,
        3,
        patch(
            level,
            patched["base"].as_str().unwrap(),
            patched["inverse"].clone(),
        ),
        "patched",
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        original,
        "{project} {level}: undo"
    );
    assert!(
        !schematic.exists(),
        "the position file the patch made is gone again"
    );
    // Redo: the undo's inverse.
    rig.request(
        &mut ws,
        4,
        patch(
            level,
            undone["base"].as_str().unwrap(),
            undone["inverse"].clone(),
        ),
        "patched",
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        after,
        "{project} {level}: redo"
    );
    rig.finish();
}

fn every_op_round_trips_on_kick_pulse_physical_av_and_a_composite(transport: Transport) {
    let svf = json!({"op": "instance.add", "name": "svf", "module": "std.svf", "params": {"cutoff_hz": 800}, "x": 100, "y": -40});
    round_trip(
        transport,
        "kick",
        "",
        json!([svf, {"op": "connection.add", "from": "svf.lp", "to": "out.l", "scale": 0.5},
            {"op": "connection.scale", "from": "lfo.out", "to": "drums.kick_decay", "scale": 60},
            {"op": "connection.scale", "from": "accent.out", "to": "drums.kick_drive", "scale": 1},
            {"op": "instance.remove", "name": "accent"},
            {"op": "connection.remove", "from": "transport.beat", "to": "drums.trigger"}]),
    );
    round_trip(
        transport,
        "pulse",
        "",
        json!([svf, {"op": "connection.add", "from": "svf.lp", "to": "out.r"},
            {"op": "connection.scale", "from": "env.out", "to": "filter.cutoff_hz", "scale": 1000},
            {"op": "instance.remove", "name": "drift"},
            {"op": "connection.remove", "from": "osc.out", "to": "filter.in"}]),
    );
    round_trip(
        transport,
        "physical-av",
        "",
        json!([svf, {"op": "connection.add", "from": "svf.lp", "to": "out.l"},
            {"op": "connection.scale", "from": "env.out", "to": "sparks.rate", "scale": 2000},
            {"op": "instance.remove", "name": "kicks"},
            {"op": "connection.remove", "from": "transport.beat", "to": "hub.in"}]),
    );
    round_trip(
        transport,
        "kick",
        "kick",
        json!([svf, {"op": "connection.add", "from": "svf.lp", "to": "vca.a"},
            {"op": "connection.scale", "from": "pitch_env.out", "to": "osc.freq", "scale": 100},
            {"op": "instance.remove", "name": "feedback"},
            {"op": "connection.remove", "from": "amp_env.out", "to": "vca.b"}]),
    );
}

/// The listed errors, each writing nothing.
fn the_errors_write_nothing(transport: Transport) {
    let mut rig = rig(
        "patch-errors",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("project.toml");
    let original = fs::read_to_string(&file).unwrap();
    let current = base(&mut rig, &mut ws, 1, "");
    let cases = [
        (
            patch(
                "",
                &"0".repeat(64),
                json!([{"op": "instance.remove", "name": "b"}]),
            ),
            "conflict",
        ),
        (
            patch(
                "nope",
                &current,
                json!([{"op": "instance.remove", "name": "b"}]),
            ),
            "unknown-level",
        ),
        (
            patch(
                "",
                &current,
                json!([{"op": "instance.remove", "name": "nobody"}]),
            ),
            "unknown-instance",
        ),
        (
            patch(
                "",
                &current,
                json!([{"op": "connection.add", "from": "nobody.out", "to": "out.l"}]),
            ),
            "unknown-instance",
        ),
        (
            patch(
                "",
                &current,
                json!([{"op": "instance.add", "name": "x", "module": "std.nothing"}]),
            ),
            "unknown-module",
        ),
        (
            patch(
                "",
                &current,
                json!([{"op": "instance.add", "name": "X", "module": "std.svf"}]),
            ),
            "bad-value",
        ),
        (
            patch(
                "",
                &current,
                json!([{"op": "instance.add", "name": "a", "module": "std.svf"}]),
            ),
            "bad-value",
        ),
        (
            patch(
                "",
                &current,
                json!([{"op": "connection.add", "from": "a.out", "to": "out.l"}]),
            ),
            "bad-value",
        ),
        (
            patch(
                "",
                &current,
                json!([{"op": "connection.remove", "from": "b.out", "to": "out.r"}]),
            ),
            "bad-value",
        ),
        (patch("", &current, json!([])), "bad-message"),
        (
            patch(
                "",
                &current,
                json!([{"op": "instance.rename", "name": "a"}]),
            ),
            "bad-message",
        ),
        (
            json!({"type": "patch", "level": "", "ops": []}),
            "bad-message",
        ),
        // The compiler's verdict: no such port. Nothing is written.
        (
            patch(
                "",
                &current,
                json!([{"op": "connection.add", "from": "a.out", "to": "out.nothing"}]),
            ),
            "does-not-load",
        ),
        // The audio sink stays while playing, as a reload requires.
        (
            patch(
                "",
                &current,
                json!([{"op": "instance.remove", "name": "out"}]),
            ),
            "does-not-load",
        ),
    ];
    for (id, (message, code)) in cases.into_iter().enumerate() {
        let reply = answer(&mut rig, &mut ws, 10 + id as u64, message.clone());
        assert_eq!(reply["type"], "error", "{message}: {reply}");
        assert_eq!(reply["code"], code, "{message}: {reply}");
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            original,
            "{message} wrote"
        );
        assert!(!rig.dir.join("ui/schematic.toml").exists());
    }
    let conflict = answer(
        &mut rig,
        &mut ws,
        50,
        patch(
            "",
            &"0".repeat(64),
            json!([{"op": "instance.remove", "name": "b"}]),
        ),
    );
    assert_eq!(
        conflict["base"], current,
        "a conflict names the current SHA-256"
    );
    let refused = answer(
        &mut rig,
        &mut ws,
        51,
        patch(
            "",
            &current,
            json!([{"op": "connection.add", "from": "a.out", "to": "out.nothing"}]),
        ),
    );
    assert!(
        refused["diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("unknown port"),
        "{refused}"
    );
    rig.finish();
}

/// `in-use`: an instance a composite exports, and instances panel bindings
/// name (at the top level, and inside a composite through every instance
/// of it). Nothing is written, and the references are named.
fn in_use_names_exports_and_panel_bindings(transport: Transport) {
    let mut rig = project_rig("patch-in-use", transport, "kick");
    let (mut ws, _) = rig.client();
    let graph = rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    let (top, kick) = (
        graph["files"][""].as_str().unwrap().to_owned(),
        graph["files"]["kick"].as_str().unwrap().to_owned(),
    );
    let before = (
        fs::read_to_string(rig.dir.join("project.toml")).unwrap(),
        fs::read_to_string(rig.dir.join("composites/kick.toml")).unwrap(),
    );
    let refused = answer(
        &mut rig,
        &mut ws,
        2,
        patch(
            "kick",
            &kick,
            json!([{"op": "instance.remove", "name": "amp_env"}]),
        ),
    );
    assert_eq!(refused["code"], "in-use", "{refused}");
    let texts: Vec<&str> = refused["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["text"].as_str().unwrap())
        .collect();
    assert!(
        texts.contains(&"[exports] inputs.trigger = amp_env.trigger"),
        "{texts:?}"
    );
    assert!(
        texts.contains(&"[exports] parameters.decay = amp_env.decay_ms"),
        "{texts:?}"
    );
    let refused = answer(
        &mut rig,
        &mut ws,
        3,
        patch(
            "",
            &top,
            json!([{"op": "instance.remove", "name": "drums"}]),
        ),
    );
    assert_eq!(refused["code"], "in-use", "{refused}");
    let knob = &refused["references"][0];
    assert_eq!(knob["file"], "ui/main.html");
    assert_eq!(knob["text"], "/drums.kick_decay");
    assert!(knob["line"].as_u64().unwrap() > 1);
    let refused = answer(
        &mut rig,
        &mut ws,
        4,
        patch(
            "kick",
            &kick,
            json!([{"op": "instance.remove", "name": "dc"}]),
        ),
    );
    assert_eq!(refused["code"], "in-use");
    let texts: Vec<&str> = refused["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["text"].as_str().unwrap())
        .collect();
    assert!(
        texts.contains(&"/drums/kick/dc.out"),
        "the scope binds it through /drums/kick: {texts:?}"
    );
    assert_eq!(
        (
            fs::read_to_string(rig.dir.join("project.toml")).unwrap(),
            fs::read_to_string(rig.dir.join("composites/kick.toml")).unwrap(),
        ),
        before
    );
    rig.finish();
}

/// Positions are written only into the project's own `ui/`: a linked `ui/`
/// or `ui/schematic.toml` refuses a patch that moves one and
/// `schematic.move`, `write-failed`, writing nothing.
fn positions_are_never_written_through_a_link(transport: Transport) {
    let mut rig = project_rig("patch-linked-ui", transport, "kick");
    let (mut ws, _) = rig.client();
    let top = base(&mut rig, &mut ws, 1, "");
    let file = rig.dir.join("project.toml");
    let original = fs::read_to_string(&file).unwrap();
    let outside = rig.dir.parent().unwrap().join("outside-ui");
    fs::rename(rig.dir.join("ui"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, rig.dir.join("ui")).unwrap();
    let svf = json!([{"op": "instance.add", "name": "svf", "module": "std.svf", "x": 10, "y": 20}]);
    let refused = answer(&mut rig, &mut ws, 2, patch("", &top, svf.clone()));
    assert_eq!(refused["code"], "write-failed", "{refused}");
    let moved = json!({"type": "schematic.move", "level": "", "instance": "lfo", "x": 1, "y": 2});
    let refused = answer(&mut rig, &mut ws, 3, moved.clone());
    assert_eq!(refused["code"], "write-failed", "{refused}");
    let reset = json!({"type": "schematic.reset"});
    let refused = answer(&mut rig, &mut ws, 4, reset);
    assert_eq!(refused["code"], "write-failed", "{refused}");
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
    assert!(!outside.join("schematic.toml").exists());
    // A real `ui/` holding a linked `ui/schematic.toml`.
    fs::remove_file(rig.dir.join("ui")).unwrap();
    fs::rename(&outside, rig.dir.join("ui")).unwrap();
    let target = rig.dir.parent().unwrap().join("outside-schematic.toml");
    let positions = "[top]\nlfo = { x = 5, y = 6 }\n";
    fs::write(&target, positions).unwrap();
    std::os::unix::fs::symlink(&target, rig.dir.join("ui/schematic.toml")).unwrap();
    let refused = answer(&mut rig, &mut ws, 5, patch("", &top, svf));
    assert_eq!(refused["code"], "write-failed", "{refused}");
    let refused = answer(&mut rig, &mut ws, 6, moved);
    assert_eq!(refused["code"], "write-failed", "{refused}");
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
    assert_eq!(fs::read_to_string(&target).unwrap(), positions);
    // A patch that moves no position still writes.
    let scaled = json!([{"op": "connection.scale", "from": "lfo.out", "to": "drums.kick_decay", "scale": 60}]);
    rig.request(&mut ws, 7, patch("", &top, scaled), "patched");
    assert_eq!(fs::read_to_string(&target).unwrap(), positions);
    rig.finish();
}

/// At a composite level of a project that does not load now, the edited
/// project still names the removal's references: `in-use`, not written.
fn in_use_holds_when_the_project_does_not_load(transport: Transport) {
    let mut rig = project_rig("patch-in-use-unloaded", transport, "kick");
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("composites/kick.toml");
    // An empty `gpu` pin: the project does not load until `ghost` goes.
    let broken = fs::read_to_string(&file).unwrap().replace(
        "feedback = { module = \"std.unit_delay\" }\n",
        "feedback = { module = \"std.unit_delay\" }\nghost = { module = \"std.mul\", gpu = \"\" }\n",
    );
    fs::write(&file, &broken).unwrap();
    let ops = json!([{"op": "instance.remove", "name": "ghost"},
        {"op": "instance.remove", "name": "dc"}]);
    let base = sha256_hex(broken.as_bytes());
    let refused = answer(&mut rig, &mut ws, 1, patch("kick", &base, ops));
    assert_eq!(refused["code"], "in-use", "{refused}");
    let texts: Vec<&str> = refused["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["text"].as_str().unwrap())
        .collect();
    assert!(
        texts.contains(&"[exports] outputs.out = dc.out"),
        "{texts:?}"
    );
    assert!(texts.contains(&"/drums/kick/dc.out"), "{texts:?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), broken);
    rig.finish();
}

const SILENT: &str = r#"schema_version = 1

[project]
name = "silent"
sample_rate = 48000
seed = 1

[instances]
a = { module = "std.sine_osc", freq = 440 }
out = { module = "std.audio_out", gain = 0.5 }
"#;

/// A copy with nothing connected to the sink is silent; a patch connecting
/// it is applied live by the watch's reload, and the meters hear it.
fn a_patch_is_heard_after_the_reload(transport: Transport) {
    let mut rig = rig(
        "patch-live",
        Options {
            transport,
            ..Options::default()
        },
    );
    fs::write(rig.dir.join("project.toml"), SILENT).unwrap();
    let (mut ws, _) = rig.client();
    // The watch reloads the silent project first.
    rig.wait(&mut ws, "the silent project", |got| {
        got.of("graph")
            .filter(|g| {
                g["top"]["instances"]
                    .as_array()
                    .is_some_and(|i| i.len() == 2)
            })
            .cloned()
    });
    let meters = rig.request(
        &mut ws,
        1,
        json!({"type": "subscribe", "stream": "meters", "rate": 30}),
        "subscribed",
    );
    let sub = meters["sub"].as_u64().unwrap();
    for _ in 0..3 {
        let frame = rig.wait(&mut ws, "a silent meter frame", |got| got.frame(1, sub));
        assert!(
            frame.f32s().iter().all(|v| *v == 0.0),
            "silent: {:?}",
            frame.f32s()
        );
    }
    let current = base(&mut rig, &mut ws, 2, "");
    rig.request(
        &mut ws,
        3,
        patch("", &current, json!([{"op": "connection.add", "from": "a.out", "to": "out.l"}, {"op": "connection.add", "from": "a.out", "to": "out.r"}])),
        "patched",
    );
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    rig.wait(&mut ws, "a graph with the connections", |got| {
        got.of("graph")
            .filter(|g| {
                g["top"]["connections"]
                    .as_array()
                    .is_some_and(|c| c.len() == 2)
            })
            .cloned()
    });
    let heard = rig.wait(&mut ws, "a nonzero meter frame", |got| {
        got.frame(1, sub)
            .filter(|f| f.f32s()[0] > 0.0)
            .map(|f| f.f32s())
    });
    assert!(heard[0] > 0.1, "{heard:?}");
    rig.finish();
}

/// `modules`: the registry's types and the project's composites in name
/// order, ports and parameters as `graph` gives them (without `address`),
/// the same reply every time.
fn modules_is_deterministic(transport: Transport) {
    let mut rig = project_rig("patch-modules", transport, "pulse");
    let (mut ws, _) = rig.client();
    let first = rig.request(&mut ws, 1, json!({"type": "modules"}), "modules");
    let second = rig.request(&mut ws, 2, json!({"type": "modules"}), "modules");
    let strip = |mut v: Value| {
        v.as_object_mut().unwrap().remove("id");
        v
    };
    assert_eq!(strip(first.clone()), strip(second));
    let names: Vec<&str> = first["primitives"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["module"].as_str().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    assert!(
        names.contains(&"std.svf")
            && !names
                .iter()
                .any(|n| n.starts_with("lab.") || n.starts_with("core."))
    );
    assert_eq!(first["composites"], json!([]));
    // std.svf as graph shows pulse's filter, without addresses.
    let graph = rig.request(&mut ws, 3, json!({"type": "graph"}), "graph");
    let filter = graph["top"]["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "filter")
        .unwrap()
        .clone();
    let svf = first["primitives"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["module"] == "std.svf")
        .unwrap()
        .clone();
    let unaddressed = |list: &Value| -> Vec<Value> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let mut p = p.clone();
                p.as_object_mut().unwrap().remove("address");
                p
            })
            .collect()
    };
    assert_eq!(unaddressed(&filter["ports"]), unaddressed(&svf["ports"]));
    assert_eq!(
        unaddressed(&filter["parameters"]),
        unaddressed(&svf["parameters"])
    );
    assert!(svf["ports"][0].get("address").is_none());
    // ADR 0027 §1: a primitive's palette group (M2.10-1).
    assert_eq!(svf["category"], "filter");
    let div = first["primitives"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["module"] == "std.div")
        .unwrap();
    assert_eq!(div["category"], "core");
    rig.finish();
    let mut rig = project_rig("patch-modules-kick", transport, "kick");
    let (mut ws, _) = rig.client();
    let kick = rig.request(&mut ws, 1, json!({"type": "modules"}), "modules");
    let composites: Vec<&str> = kick["composites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["module"].as_str().unwrap())
        .collect();
    assert_eq!(composites, ["drums", "kick"]);
    // A composite has no category: the key is absent.
    assert!(
        kick["composites"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get("category").is_none())
    );
    rig.finish();
}

/// A removal whose undo would not fit a client message (a 70 000-byte
/// comment above it) is refused before anything is written; a commit's
/// write pushes `graph` with the file's new SHA-256.
fn an_undo_too_large_is_refused_and_a_commit_pushes_files(transport: Transport) {
    let mut rig = rig(
        "patch-size",
        Options {
            transport,
            ..Options::default()
        },
    );
    let file = rig.dir.join("project.toml");
    let big = format!("# {}\n", "x".repeat(70_000));
    let text = common::TONE.replace("b = { module", &format!("{big}b = {{ module"));
    fs::write(&file, &text).unwrap();
    let (mut ws, _) = rig.client();
    rig.wait(&mut ws, "the graph of the edited file", |got| {
        got.of("graph")
            .filter(|g| g["files"][""] == sha256_hex(text.as_bytes()))
            .cloned()
    });
    let refused = answer(
        &mut rig,
        &mut ws,
        1,
        patch(
            "",
            &sha256_hex(text.as_bytes()),
            json!([{"op": "instance.remove", "name": "b"}]),
        ),
    );
    assert_eq!(refused["code"], "bad-value", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("over the 64 KiB limit"),
        "{refused}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
    // A small removal from the same file is written.
    let patched = answer(
        &mut rig,
        &mut ws,
        2,
        patch(
            "",
            &sha256_hex(text.as_bytes()),
            json!([{"op": "instance.remove", "name": "env"}]),
        ),
    );
    assert_eq!(patched["type"], "patched", "{patched}");
    // A commit writes project.toml: graph.files follows it (pushed before
    // the commit's reply).
    let before = fs::read_to_string(&file).unwrap();
    send(
        &mut ws,
        json!({"type": "commit", "id": 3, "address": "/out.gain", "value": 0.25}),
    );
    let pushed = rig.wait(&mut ws, "graph after the commit", |got| {
        got.of("graph")
            .filter(|g| g.get("id").is_none() && g["files"][""] != sha256_hex(before.as_bytes()))
            .cloned()
    });
    let written = fs::read_to_string(&file).unwrap();
    assert_ne!(written, before);
    assert_eq!(pushed["files"][""], sha256_hex(written.as_bytes()));
    while receive(&mut ws).is_some() {}
    rig.finish();
}

/// auditor-1-008: two copies whose folders have the same name (`project`)
/// and whose projects have the same name are different projects to the
/// page: `graph.projectId` differs (and carries no path).
fn same_named_copies_have_different_project_ids(transport: Transport) {
    let ids: Vec<(String, String, String)> = ["patch-id-a", "patch-id-b"]
        .into_iter()
        .map(|name| {
            let mut rig = project_rig(name, transport, "kick");
            let (mut ws, _) = rig.client();
            let graph = rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
            let again = rig.request(&mut ws, 2, json!({"type": "graph"}), "graph");
            assert_eq!(
                graph["projectId"], again["projectId"],
                "stable for one project"
            );
            rig.finish();
            (
                graph["folder"].as_str().unwrap().to_owned(),
                graph["project"].as_str().unwrap().to_owned(),
                graph["projectId"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        (&ids[0].0, &ids[0].1),
        (&ids[1].0, &ids[1].1),
        "same folder and project name"
    );
    assert_ne!(ids[0].2, ids[1].2);
    assert_eq!(ids[0].2.len(), 64);
    assert!(
        ids[0].2.chars().all(|c| c.is_ascii_hexdigit()),
        "opaque, no path"
    );
}

both!(
    same_named_copies_have_different_project_ids,
    every_op_round_trips_on_kick_pulse_physical_av_and_a_composite,
    the_errors_write_nothing,
    in_use_names_exports_and_panel_bindings,
    a_patch_is_heard_after_the_reload,
    modules_is_deterministic,
    an_undo_too_large_is_refused_and_a_commit_pushes_files,
    positions_are_never_written_through_a_link,
    in_use_holds_when_the_project_does_not_load,
);
