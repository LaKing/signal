//! ADR 0035 §10 (test 12, its override half): an include's top-level
//! parameters are controls (`/<include>/<instance>.<p>`) whose `commit`
//! writes an override into the including entry's `set` table, which a
//! reload keeps; Reset removes the key and an empty table with it; an
//! exported parameter is a control on the entry itself. No committed
//! project is written.

mod common;

use std::fs;

use common::{Options, Transport, rig};
use serde_json::{Value, json};

const HOST: &str = r#"schema_version = 1

[project]
name = "host"
sample_rate = 48000
seed = 1

[instances]
inc = { project = "inc" }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "inc.l", to = "out.l" },
  { from = "inc.r", to = "out.r" },
]
"#;

const INCLUDED: &str = r#"schema_version = 1

[project]
name = "inc"
sample_rate = 48000
seed = 2

[instances]
osc = { module = "std.sine_osc", freq = 220 }
amp = { module = "std.gain", db = -6 }
out = { module = "std.audio_out", gain = 0.25 }

[wiring]
connections = [
  { from = "osc.out", to = "amp.in" },
  { from = "amp.out", to = "out.l" },
  { from = "amp.out", to = "out.r" },
]

[exports]
parameters = { level = "amp.db" }
"#;

fn control<'a>(described: &'a Value, address: &str) -> Option<&'a Value> {
    described["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == address)
}

fn an_include_s_parameters_write_its_entry_and_set(transport: Transport) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-include-panel-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(source.join("inc")).unwrap();
    fs::write(source.join("project.toml"), HOST).unwrap();
    fs::write(source.join("inc/project.toml"), INCLUDED).unwrap();
    let mut rig = rig(
        "include-panel",
        Options {
            transport,
            project: Some(source.clone()),
            ..Options::default()
        },
    );
    let _ = fs::remove_dir_all(&source);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let d = rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    assert!(control(&d, "/inc.level").is_some(), "the export: {d}");
    assert!(control(&d, "/inc/osc.freq").is_some(), "an override: {d}");
    assert!(
        control(&d, "/inc/amp.db").is_none(),
        "a parameter an export reaches is the export's"
    );
    // An override, into the entry's `set`.
    rig.request(
        &mut ws,
        2,
        json!({"type": "commit", "address": "/inc/osc.freq", "value": 330}),
        "committed",
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(r#"inc = { project = "inc", set = { "osc.freq" = 330 } }"#),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(rig.dir.join("inc/project.toml")).unwrap(),
        INCLUDED,
        "the included file is never written"
    );
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    let d = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    assert_eq!(control(&d, "/inc/osc.freq").unwrap()["base"], 330.0);
    // The export, on the entry.
    rig.request(
        &mut ws,
        4,
        json!({"type": "commit", "address": "/inc.level", "value": -12}),
        "committed",
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("level = -12"), "{text}");
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // Reset: the key goes, and the empty table with it.
    rig.request(
        &mut ws,
        5,
        json!({"type": "commit", "address": "/inc/osc.freq", "value": null}),
        "committed",
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("set ="), "{text}");
    rig.finish();
}

both!(an_include_s_parameters_write_its_entry_and_set);

/// ADR 0035 §10: an include's panel is read (`panel.read` of
/// `include/<key>/main.html`, never editable here) and served with what it
/// loads under `/include/<key>/` (TCP: the operator's HTTP).
#[test]
fn an_include_s_panel_is_read_and_served() {
    use std::io::{Read, Write};
    let source = std::env::temp_dir().join(format!(
        "signal-ui-include-served-source-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(source.join("inc/ui")).unwrap();
    fs::write(source.join("project.toml"), HOST).unwrap();
    fs::write(source.join("inc/project.toml"), INCLUDED).unwrap();
    let page = "<!doctype html>\n<link rel=\"stylesheet\" href=\"inc.css\">\n<signal-panel>\n  <div class=\"signal-cards\">\n    <section class=\"signal-card\">\n      <h2>tone</h2>\n      <signal-knob bind=\"/osc.freq\"></signal-knob>\n    </section>\n  </div>\n</signal-panel>\n";
    fs::write(source.join("inc/ui/main.html"), page).unwrap();
    fs::write(
        source.join("inc/ui/inc.css"),
        ".signal-card { color: red; }\n",
    )
    .unwrap();
    fs::create_dir_all(source.join("inc/patterns")).unwrap();
    let beat = "schema_version = 1\nkind = \"steps\"\nsteps = 16\nstep = 0.25\n\n[lanes]\nkick = \"x...x...x...x...\"\n";
    fs::write(source.join("inc/patterns/beat.toml"), beat).unwrap();
    let mut rig = rig(
        "include-served",
        Options {
            transport: Transport::Tcp,
            project: Some(source.clone()),
            ..Options::default()
        },
    );
    let _ = fs::remove_dir_all(&source);
    let (mut ws, _) = rig.client();
    let read = rig.request(
        &mut ws,
        1,
        json!({"type": "panel.read", "file": "include/inc/main.html"}),
        "panel.read",
    );
    assert_eq!(read["exists"], true, "{read}");
    assert_eq!(read["editable"], false);
    assert!(
        read["html"]
            .as_str()
            .unwrap()
            .contains("bind=\"/osc.freq\"")
    );
    let missing = rig.request(
        &mut ws,
        2,
        json!({"type": "panel.read", "file": "include/out/main.html"}),
        "error",
    );
    assert_eq!(missing["code"], "unknown-panel", "{missing}");
    // Its patterns: read from its folder, never written here.
    let pattern = rig.request(
        &mut ws,
        3,
        json!({"type": "pattern.read", "name": "include/inc/beat"}),
        "pattern.read",
    );
    assert_eq!(pattern["exists"], true, "{pattern}");
    assert!(pattern["text"].as_str().unwrap().contains("x...x"));
    let refused = rig.request(
        &mut ws,
        4,
        json!({"type": "pattern.write", "name": "include/inc/beat", "text": beat, "base": pattern["base"]}),
        "error",
    );
    assert_eq!(refused["code"], "not-editable", "{refused}");
    let addr = rig.host().http_addr();
    let get = |path: &str| {
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
    assert!(get("/include/inc/inc.css").contains("color: red"));
    assert!(get("/include/inc/main.html").starts_with("HTTP/1.1 200"));
    assert!(get("/include/inc/../project.toml").starts_with("HTTP/1.1 404"));
    assert!(get("/include/out/main.html").starts_with("HTTP/1.1 404"));
    rig.finish();
}
