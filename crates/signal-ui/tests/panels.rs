//! M2.8-3: panels as text. AC2: the kick's panel against the null driver;
//! its decay knob moved through the protocol as the knob does it (a ramp
//! per move, a commit on release) changes exactly one value of
//! `project.toml`, the panel file not at all, and the watcher's reload
//! applies nothing. AC5: the binding check a panel asks for when it loads
//! (`panel.check`), which refuses an editable control on a parameter that
//! `commit` cannot write (AC8). AC6: a project without `ui/main.html` is
//! served the generated panel, whose scope is on the audio sink's input: a
//! scope on a signal input shows the sum of its sources. Each scenario runs
//! in process (here) and over TCP (the operator's twin: a sandbox cannot
//! bind loopback).

mod common;

use std::collections::BTreeMap;
use std::fs;

use common::{Options, Transport, rig, root};
use serde_json::json;
use signal_ui::bindings::elements;
use signal_ui::frames::SCOPE;

/// Every value of a TOML document by its dotted path.
fn values(text: &str) -> BTreeMap<String, String> {
    fn walk(prefix: &str, value: &toml::Value, out: &mut BTreeMap<String, String>) {
        match value {
            toml::Value::Table(table) => {
                for (key, value) in table {
                    walk(&format!("{prefix}{key}."), value, out);
                }
            }
            toml::Value::Array(items) => {
                for (i, value) in items.iter().enumerate() {
                    walk(&format!("{prefix}{i}."), value, out);
                }
            }
            other => {
                out.insert(prefix.trim_end_matches('.').to_owned(), other.to_string());
            }
        }
    }
    let mut out = BTreeMap::new();
    let table: toml::Table = text.parse().unwrap();
    walk("", &toml::Value::Table(table), &mut out);
    out
}

fn kick(name: &str, transport: Transport) -> common::Rig {
    rig(
        name,
        Options {
            transport,
            project: Some(root().join("projects/kick")),
            ..Options::default()
        },
    )
}

fn a_knob_of_the_kick_panel_changes_one_value_of_the_toml(transport: Transport) {
    let mut rig = kick("kick-panel", transport);
    let toml_path = rig.dir.join("project.toml");
    let panel_path = rig.dir.join("ui/main.html");
    let before = fs::read_to_string(&toml_path).unwrap();
    let panel = fs::read_to_string(&panel_path).unwrap();
    let (mut ws, _) = rig.client();

    // The panel loads: describe, then its binding check, clean.
    let d = rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    assert_eq!(d["project"], "kick");
    let checked = rig.request(
        &mut ws,
        2,
        json!({"type": "panel.check", "file": "main.html"}),
        "panel.check",
    );
    assert_eq!(checked["ok"], true, "{checked}");
    assert_eq!(checked["diagnostics"], json!([]), "{checked}");

    // The decay knob, as the panel file binds it.
    let knob = elements(&panel)
        .into_iter()
        .find(|e| {
            e.name == "signal-knob"
                && e.attribute("label")
                    .and_then(|a| a.value.as_ref())
                    .map(|v| v.0.as_str())
                    == Some("decay")
        })
        .expect("the decay knob");
    let address = knob.addresses()[0].0.to_owned();
    assert_eq!(address, "/drums.kick_decay");

    // A drag: a 5 ms ramp per move, then the commit on release.
    for (id, to) in [(3, 400.0), (4, 350.0), (5, 300.0)] {
        let ack = rig.request(
            &mut ws,
            id,
            json!({"type": "ramp", "address": address, "to": to, "ms": 5}),
            "ack",
        );
        assert_eq!(ack["outcome"], "scheduled", "{ack}");
    }
    let committed = rig.request(
        &mut ws,
        6,
        json!({"type": "commit", "address": address, "value": 300.0}),
        "committed",
    );
    assert_eq!(committed["sent"], true, "the last ramp already sent 300");
    assert_eq!(committed["file"], "project.toml");

    // Exactly one value of the TOML changed; the panel file did not.
    let after = fs::read_to_string(&toml_path).unwrap();
    let (old, new) = (values(&before), values(&after));
    let changed: Vec<&String> = old
        .keys()
        .chain(new.keys())
        .filter(|k| old.get(*k) != new.get(*k))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(changed, ["instances.drums.kick_decay"], "{after}");
    assert_eq!(new["instances.drums.kick_decay"], "300");
    let lines = |t: &str| t.lines().map(str::to_owned).collect::<Vec<_>>();
    let (old_lines, new_lines) = (lines(&before), lines(&after));
    assert_eq!(old_lines.len(), new_lines.len(), "one line edited in place");
    let edited = old_lines
        .iter()
        .zip(&new_lines)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(edited, 1, "{after}");
    assert_eq!(fs::read_to_string(&panel_path).unwrap(), panel);

    // The running project already holds the written file: no reload.
    rig.no_reload(&mut ws, 300);
    let d = rig.request(&mut ws, 7, json!({"type": "describe"}), "describe");
    let decay = d["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == address)
        .unwrap();
    assert_eq!(decay["base"], 300.0);
    assert_eq!(decay["inFile"], true);
    rig.finish();
}

fn a_panel_that_binds_what_the_plan_lacks_is_told_where(transport: Transport) {
    let mut rig = kick("panel-check", transport);
    let broken = "<!doctype html>\n<signal-panel>\n  <signal-knob bind=\"/drums.kick_decya\"></signal-knob>\n  <signal-button bind=\"/out.gain\"></signal-button>\n  <signal-scope bind=\"/drums/kick/dc.out\"></signal-scope>\n  <signal-fader bind=\"/drums/kick/amp_env.decay_ms\"></signal-fader>\n</signal-panel>\n";
    fs::write(rig.dir.join("ui/broken.html"), broken).unwrap();
    let (mut ws, _) = rig.client();
    let checked = rig.request(
        &mut ws,
        1,
        json!({"type": "panel.check", "file": "broken.html"}),
        "panel.check",
    );
    assert_eq!(checked["file"], "ui/broken.html");
    assert_eq!(checked["ok"], false);
    assert_eq!(checked["controls"], 4);
    assert_eq!(
        checked["diagnostics"],
        json!([
            {"severity": "error", "file": "ui/broken.html", "line": 3, "column": 22, "control": 0,
             "message": "unknown port /drums.kick_decya: /drums has kick_decay, kick_drive"},
            {"severity": "error", "file": "ui/broken.html", "line": 4, "column": 24, "control": 1,
             "message": "wrong kind: /out.gain is a parameter; <signal-button> binds an event input of the plan"},
            {"severity": "error", "file": "ui/broken.html", "line": 6, "column": 23, "control": 3,
             "message": "not editable: /drums/kick/amp_env.decay_ms is a parameter inside the composite /drums, which `commit` cannot write; <signal-fader> binds a control (a top-level parameter, /<instance>.<parameter>): export it from the composite, or show it with <signal-value>"},
        ])
    );
    // What the check refuses, `commit` refuses too.
    let refused = rig.request(
        &mut ws,
        5,
        json!({"type": "commit", "address": "/drums/kick/amp_env.decay_ms", "value": 300}),
        "error",
    );
    assert_eq!(refused["code"], "not-editable");
    // Only HTML files of the project's ui/ folder are checked.
    for (id, file) in [
        (2, "../project.toml"),
        (3, "missing.html"),
        (4, ".hidden.html"),
    ] {
        let refused = rig.request(
            &mut ws,
            id,
            json!({"type": "panel.check", "file": file}),
            "error",
        );
        assert_eq!(refused["code"], "unknown-panel", "{file}");
    }
    rig.finish();
}

/// M2.8-7: `<signal-canvas>` binds vector outputs, up to three: the demo's
/// own panel checks, and a signal output or a fourth address is an error.
fn a_canvas_binds_vector_outputs(transport: Transport) {
    let mut rig = rig(
        "canvas-check",
        Options {
            transport,
            project: Some(common::root().join("projects/physical-av")),
            ..Options::default()
        },
    );
    let panel = "<!doctype html>\n<signal-panel>\n  <signal-canvas renderer=\"points\" bind=\"/sparks.pos /sparks.life\"></signal-canvas>\n  <signal-canvas bind=\"/membrane_a.out\"></signal-canvas>\n  <signal-canvas bind=\"/membrane_v.z /sparks.pos /sparks.vel /sparks.life\"></signal-canvas>\n</signal-panel>\n";
    fs::write(rig.dir.join("ui/canvas.html"), panel).unwrap();
    let (mut ws, _) = rig.client();
    let main = rig.request(
        &mut ws,
        1,
        json!({"type": "panel.check", "file": "main.html"}),
        "panel.check",
    );
    assert_eq!(main["ok"], true, "{main}");
    let checked = rig.request(
        &mut ws,
        2,
        json!({"type": "panel.check", "file": "canvas.html"}),
        "panel.check",
    );
    assert_eq!(checked["ok"], false);
    let diagnostics = checked["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 2, "{checked}");
    let message = |i: usize| diagnostics[i]["message"].as_str().unwrap().to_owned();
    assert_eq!(diagnostics[0]["control"], 1);
    assert!(
        message(0).contains("/membrane_a.out is a signal output"),
        "{}",
        message(0)
    );
    assert!(
        message(0).contains("a vector output of the plan"),
        "{}",
        message(0)
    );
    assert_eq!(diagnostics[1]["control"], 2);
    rig.finish();
}

both!(
    a_knob_of_the_kick_panel_changes_one_value_of_the_toml,
    a_panel_that_binds_what_the_plan_lacks_is_told_where,
    a_canvas_binds_vector_outputs,
);

/// The generated panel's scope is on the audio sink's input: in the tone
/// project `/out.l` receives `/a.out` + `/b.out`, and a scope on it shows
/// that sum, not either source.
fn a_scope_on_the_sinks_input_shows_the_sum_of_its_sources(transport: Transport) {
    let mut rig = rig(
        "sink-scope",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let mut subs = Vec::new();
    for (id, port) in [(1, "/out.l"), (2, "/a.out"), (3, "/b.out")] {
        let reply = rig.request(
            &mut ws,
            id,
            json!({"type": "subscribe", "stream": "scope", "port": port, "rate": 48000}),
            "subscribed",
        );
        assert_eq!(reply["decimation"], 1);
        subs.push(reply["sub"].as_u64().unwrap());
    }
    // Each stream's points by frame, until 512 frames are in all three.
    let mut points: [BTreeMap<u64, f32>; 3] = Default::default();
    let common = rig.wait(&mut ws, "overlapping scope blocks", |got| {
        for (i, &sub) in subs.iter().enumerate() {
            if let Some(frame) = got.frame(SCOPE, sub) {
                for (k, v) in frame.f32s().into_iter().enumerate() {
                    points[i].insert(frame.first + k as u64 * u64::from(frame.extra), v);
                }
            }
        }
        let common: Vec<u64> = points[0]
            .keys()
            .filter(|f| points[1].contains_key(f) && points[2].contains_key(f))
            .copied()
            .collect();
        (common.len() >= 512).then_some(common)
    });
    let mut differs = false;
    for f in common {
        let (l, a, b) = (points[0][&f], points[1][&f], points[2][&f]);
        assert!(
            (l - (a + b)).abs() <= 1e-6,
            "frame {f}: {l} against {a} + {b}"
        );
        differs |= (l - a).abs() > 1e-3;
    }
    assert!(differs, "the sum is not the first source");
    rig.finish();
}

both!(a_scope_on_the_sinks_input_shows_the_sum_of_its_sources);

/// AC6 over HTTP (TCP only, the operator's): `/project/main.html` is the
/// project's panel, else the generated one from the UI library.
#[test]
fn a_project_without_a_main_panel_is_served_the_generated_one() {
    use std::io::{Read, Write};
    let get = |addr: std::net::SocketAddr, path: &str| {
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            addr.port()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let mut tone = rig("default-panel", Options::default());
    let page = get(tone.host().http_addr(), "/project/main.html");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("<signal-panel generated>"), "{page}");
    assert!(get(tone.host().http_addr(), "/project/other.html").starts_with("HTTP/1.1 404"));
    assert!(tone.host().panel_url().contains("/project/main.html#ws="));
    tone.finish();
    let mut kick = kick("main-panel", Transport::Tcp);
    let page = get(kick.host().http_addr(), "/project/main.html");
    assert!(
        page.contains("<signal-knob bind=\"/drums.kick_decay\""),
        "{page}"
    );
    kick.finish();
}
