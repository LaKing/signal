//! M2.9-1 (ADR 0026 §1, §3): `graph` and `schematic.move` / `schematic.reset`
//! in `signal ui`, over both transports, on copies of the committed projects:
//! kick's nested composites, pulse, physical-av's converter on its rate
//! change, the push after a structural reload, positions written to
//! `ui/schematic.toml` and read back, and an edit of that file pushed.
//! (Stopped and switched off, in app mode: `app_mode.rs`.)

mod common;

use std::fs;

use common::{Options, Rig, Transport, Ws, rig, root};
use serde_json::{Value, json};

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

fn names(level: &Value) -> Vec<&str> {
    level["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect()
}

/// The next pushed `graph` (no `id`) that `wanted` accepts.
fn pushed(rig: &mut Rig, ws: &mut Ws, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    rig.wait(ws, what, |got| {
        got.of("graph")
            .filter(|g| g.get("id").is_none() && wanted(g))
            .cloned()
    })
}

fn kick_pulse_and_physical_av_are_described_as_written(transport: Transport) {
    let mut rig = project_rig("graph-kick", transport, "kick");
    let (mut ws, _) = rig.client();
    let g = rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    assert_eq!(g["project"], "kick");
    assert_eq!(
        names(&g["top"]),
        ["transport", "drums", "lfo", "accent", "out"]
    );
    assert_eq!(g["top"]["instances"][1]["composite"], true);
    let keys: Vec<_> = g["composites"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["drums", "kick"]);
    assert_eq!(names(&g["composites"]["drums"]), ["kick"]);
    assert_eq!(g["composites"]["drums"]["instances"][0]["composite"], true);
    assert_eq!(g["composites"]["kick"]["outputs"][0]["from"], "dc.out");
    assert_eq!(g["top"]["connections"][0]["converters"], json!([]));
    assert_eq!(
        g["layout"],
        json!({"file": "ui/schematic.toml", "positions": {}, "error": null})
    );
    rig.finish();

    let mut rig = project_rig("graph-pulse", transport, "pulse");
    let (mut ws, _) = rig.client();
    let g = rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    assert_eq!(names(&g["top"]).len(), 17);
    let connections = g["top"]["connections"].as_array().unwrap();
    assert_eq!(connections.len(), 20);
    assert!(connections.iter().all(|c| c["converters"] == json!([])));
    assert_eq!(g["composites"], json!({}));
    rig.finish();

    let mut rig = project_rig("graph-physical-av", transport, "physical-av");
    let (mut ws, _) = rig.client();
    let g = rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    let converted: Vec<_> = g["top"]["connections"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["converters"] != json!([]))
        .collect();
    assert_eq!(converted.len(), 1, "{converted:?}");
    assert_eq!(converted[0]["from"], "/env.out");
    assert_eq!(converted[0]["to"], "/sparks.rate");
    let converter = &converted[0]["converters"][0];
    assert_eq!(converter["conversion"], "decimate");
    assert_eq!(
        (converter["fromRate"].clone(), converter["toRate"].clone()),
        (json!(1), json!(800))
    );
    assert!(
        converter["module"]
            .as_str()
            .unwrap()
            .starts_with("core.decimate.d1.d800.")
    );
    rig.finish();
}

fn a_structural_reload_pushes_graph(transport: Transport) {
    let mut rig = project_rig("graph-reload", transport, "kick");
    let (mut ws, _) = rig.client();
    rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    let path = rig.dir.join("project.toml");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace(
            "out = { module",
            "extra = { module = \"std.sine_osc\", freq = 330 }\nout = { module",
        )
        .replace(
            "  { from = \"drums.out\", to = \"out.r\" },",
            "  { from = \"drums.out\", to = \"out.r\" },\n  { from = \"extra.out\", to = \"out.r\", scale = 0.1 },",
        );
    fs::write(&path, text).unwrap();
    let g = pushed(&mut rig, &mut ws, "graph after the swap", |g| {
        names(&g["top"]).contains(&"extra")
    });
    assert!(
        g["planVersion"].as_u64() > Some(0),
        "pushed once the swap committed: {}",
        g["planVersion"]
    );
    assert_eq!(names(&g["top"])[4], "extra", "in source order");
    let last = g["top"]["connections"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["from"], "/extra.out");
    rig.finish();
}

fn schematic_move_writes_positions_and_pushes_graph(transport: Transport) {
    let mut rig = project_rig("schematic-move", transport, "kick");
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("ui/schematic.toml");
    let moved = rig.request(
        &mut ws,
        1,
        json!({"type": "schematic.move", "level": "", "instance": "drums", "x": 10.4, "y": -3.6}),
        "schematic.moved",
    );
    assert_eq!(
        (moved["x"].clone(), moved["y"].clone()),
        (json!(10), json!(-4)),
        "rounded"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "[top]\ndrums = { x = 10, y = -4 }\n"
    );
    let g = pushed(&mut rig, &mut ws, "graph with the position", |g| {
        g["layout"]["positions"][""]["drums"] == json!({"x": 10, "y": -4})
    });
    assert_eq!(g["layout"]["error"], Value::Null);
    // The watch takes the write as seen: no reload of it.
    rig.no_reload(&mut ws, 300);

    rig.request(
        &mut ws,
        2,
        json!({"type": "schematic.move", "level": "kick", "instance": "osc", "x": 0, "y": 30}),
        "schematic.moved",
    );
    // A hand-written comment survives a later move.
    let text = format!("# by hand\n{}", fs::read_to_string(&file).unwrap());
    fs::write(&file, &text).unwrap();
    rig.request(
        &mut ws,
        3,
        json!({"type": "schematic.move", "level": "", "instance": "drums", "x": 7, "y": 8}),
        "schematic.moved",
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "# by hand\n[top]\ndrums = { x = 7, y = 8 }\n\n[composite.kick]\nosc = { x = 0, y = 30 }\n"
    );

    for (id, message, code) in [
        (
            4,
            json!({"type": "schematic.move", "level": "nope", "instance": "osc", "x": 0, "y": 0}),
            "unknown-level",
        ),
        (
            5,
            json!({"type": "schematic.move", "level": "kick", "instance": "drums", "x": 0, "y": 0}),
            "unknown-instance",
        ),
        (
            6,
            json!({"type": "schematic.move", "level": "", "instance": "drums", "x": "a", "y": 0}),
            "bad-value",
        ),
        (
            7,
            json!({"type": "schematic.move", "level": "", "instance": "drums", "x": 1e10, "y": 0}),
            "bad-value",
        ),
        (
            8,
            json!({"type": "schematic.move", "instance": "drums", "x": 0, "y": 0}),
            "bad-message",
        ),
        (
            9,
            json!({"type": "schematic.reset", "level": "nope"}),
            "unknown-level",
        ),
    ] {
        let refused = rig.request(&mut ws, id, message, "error");
        assert_eq!(refused["code"], code, "{refused}");
    }
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("drums = { x = 7, y = 8 }")
    );

    // Reset layout: one level's table, then the whole file.
    rig.request(
        &mut ws,
        10,
        json!({"type": "schematic.reset", "level": "kick"}),
        "schematic.reset",
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "# by hand\n[top]\ndrums = { x = 7, y = 8 }\n"
    );
    pushed(&mut rig, &mut ws, "graph without kick's positions", |g| {
        g["layout"]["positions"].get("kick").is_none() && g["layout"]["positions"].get("").is_some()
    });
    rig.request(
        &mut ws,
        11,
        json!({"type": "schematic.reset"}),
        "schematic.reset",
    );
    assert!(!file.exists());
    pushed(&mut rig, &mut ws, "graph without positions", |g| {
        g["layout"]["positions"] == json!({})
    });
    rig.finish();
}

fn an_edit_of_the_positions_file_pushes_graph(transport: Transport) {
    let mut rig = project_rig("schematic-edit", transport, "kick");
    let (mut ws, _) = rig.client();
    rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    fs::write(
        rig.dir.join("ui/schematic.toml"),
        "[composite.drums]\nkick = { x = 3, y = 4 }\n",
    )
    .unwrap();
    pushed(&mut rig, &mut ws, "graph after the edit", |g| {
        g["layout"]["positions"]["drums"]["kick"] == json!({"x": 3, "y": 4})
    });
    // Broken, it is reported, never a reason not to describe the project.
    fs::write(rig.dir.join("ui/schematic.toml"), "[top\n").unwrap();
    let g = pushed(&mut rig, &mut ws, "graph with the file's error", |g| {
        g["layout"]["error"].is_string()
    });
    assert_eq!(g["layout"]["positions"], json!({}));
    assert_eq!(names(&g["top"]).len(), 5);
    rig.finish();
}

both!(
    kick_pulse_and_physical_av_are_described_as_written,
    a_structural_reload_pushes_graph,
    schematic_move_writes_positions_and_pushes_graph,
    an_edit_of_the_positions_file_pushes_graph,
);
