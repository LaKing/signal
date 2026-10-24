//! `signal mcp` (M2.12-1, ADR 0029, `docs/agent-interface.md`): the built
//! binary spoken to over stdio as an MCP client does, on copies of test
//! projects (the committed ones are never written), on the null driver (no
//! test plays audio). MCP conformance (both eras: `initialize` and the
//! current revision's per-request `_meta`), one test per tool, read-only
//! mode, the scripted session of AC11 on a copy of `projects/studio`, and
//! (`attach`, loopback: not runnable where no port can be bound) a server
//! attached to a running `signal ui`. The gain ceiling: `mcp_gain.rs`.

mod mcp_common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use mcp_common::{Mcp, copy_of, fixture, stale};
use serde_json::{Value, json};

const TOOLS: [&str; 28] = [
    "project_info",
    "graph",
    "modules",
    "describe_instance",
    "read_file",
    "stats",
    "set_parameter",
    "ramp_parameter",
    "trigger",
    "transport",
    "commit",
    "revert",
    "patch",
    "schematic_move",
    "vendor",
    "write_pattern",
    "write_panel",
    "write_file",
    "meter",
    "scope",
    "read_stream",
    "render",
    "ui_check",
    "check",
    "library",
    "session_start",
    "session_stop",
    "replay",
];

fn names(list: &Value) -> Vec<String> {
    list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

/// The live base of a control, from `project_info`.
fn control_base(mcp: &mut Mcp, address: &str) -> f64 {
    let info = mcp.ok("project_info", json!({}));
    info["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == address)
        .unwrap_or_else(|| panic!("no control {address}: {info}"))["base"]
        .as_f64()
        .unwrap()
}

// ---------------------------------------------------------------------------
// Conformance.

#[test]
fn initialize_negotiates_a_legacy_version_and_lists_the_tools() {
    let dir = fixture("sine", "conformance-legacy");
    let mut mcp = Mcp::start(&dir, &[]);
    // Before initialize and without `_meta`, a request is refused; ping works.
    let early = mcp.rpc("tools/list", json!({}));
    assert_eq!(early["error"]["code"], -32600, "{early}");
    assert_eq!(mcp.rpc("ping", json!({}))["result"], json!({}));
    for (asked, answered) in [
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        ("2024-11-05", "2025-11-25"),
        ("1900-01-01", "2025-11-25"),
    ] {
        let init = mcp.rpc(
            "initialize",
            json!({"protocolVersion": asked, "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
        );
        assert_eq!(init["result"]["protocolVersion"], answered, "{init}");
        assert_eq!(init["result"]["serverInfo"]["name"], "signal");
        assert!(init["result"]["capabilities"]["tools"].is_object());
    }
    mcp.notify("notifications/initialized", json!({}));
    let list = mcp.rpc("tools/list", json!({}));
    assert_eq!(
        names(&list),
        TOOLS,
        "every ADR 0029 §2 tool, in a fixed order"
    );
    assert!(list["result"].get("resultType").is_none(), "legacy results");
    for tool in list["result"]["tools"].as_array().unwrap() {
        assert!(!tool["description"].as_str().unwrap().is_empty());
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }
    assert_eq!(
        names(&mcp.rpc("tools/list", json!({}))),
        TOOLS,
        "the same again"
    );
    assert!(mcp.finish().success());
}

#[test]
fn the_current_revision_needs_no_handshake() {
    let dir = fixture("sine", "conformance-modern");
    let mut mcp = Mcp::start(&dir, &[]);
    let meta = json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"}, "io.modelcontextprotocol/clientCapabilities": {}});
    let discover = mcp.rpc("server/discover", json!({"_meta": meta}));
    assert_eq!(discover["result"]["resultType"], "complete");
    assert_eq!(
        discover["result"]["supportedVersions"],
        json!(["2026-07-28", "2025-11-25", "2025-06-18"])
    );
    let list = mcp.rpc("tools/list", json!({"_meta": meta}));
    assert_eq!(list["result"]["resultType"], "complete");
    assert_eq!(names(&list), TOOLS);
    let call = mcp.rpc(
        "tools/call",
        json!({"_meta": meta, "name": "stats", "arguments": {}}),
    );
    assert_eq!(call["result"]["resultType"], "complete");
    assert_eq!(call["result"]["isError"], false);
    let unsupported = mcp.rpc(
        "tools/list",
        json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "1900-01-01"}}),
    );
    assert_eq!(unsupported["error"]["code"], -32022, "{unsupported}");
    assert_eq!(unsupported["error"]["data"]["requested"], "1900-01-01");
    assert_eq!(unsupported["error"]["data"]["supported"][0], "2026-07-28");
    assert!(mcp.finish().success());
}

#[test]
fn json_rpc_errors() {
    let dir = fixture("sine", "conformance-errors");
    let mut mcp = Mcp::started(&dir, &[]);
    mcp.write_line("{not json");
    assert_eq!(mcp.line()["error"]["code"], -32700);
    mcp.write_line("[1, 2]");
    assert_eq!(mcp.line()["error"]["code"], -32600);
    mcp.write_line(r#"{"id": 7, "method": "ping"}"#);
    let reply = mcp.line();
    assert_eq!(reply["error"]["code"], -32600, "no jsonrpc");
    assert_eq!(reply["id"], 7);
    let unknown = mcp.rpc("no/such", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
    // A notification gets no reply: the next line answers the next request.
    mcp.notify("notifications/cancelled", json!({"requestId": 1}));
    assert_eq!(mcp.rpc("ping", json!({}))["result"], json!({}));
    // An unknown tool, a missing name and arguments that are no object are
    // protocol errors; arguments the tool's schema refuses are a tool error.
    let tool = mcp.rpc(
        "tools/call",
        json!({"name": "no_such_tool", "arguments": {}}),
    );
    assert_eq!(tool["error"]["code"], -32602, "{tool}");
    assert!(
        tool["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no_such_tool")
    );
    let nameless = mcp.rpc("tools/call", json!({"arguments": {}}));
    assert_eq!(nameless["error"]["code"], -32602);
    let not_object = mcp.rpc("tools/call", json!({"name": "stats", "arguments": [1]}));
    assert_eq!(not_object["error"]["code"], -32602);
    for bad in [
        json!({"address": "/osc.freq"}),
        json!({"address": "/osc.freq", "value": "high"}),
        json!({"address": "/osc.freq", "value": 1, "extra": true}),
    ] {
        let message = mcp.refused("set_parameter", bad.clone());
        assert!(message.starts_with("invalid arguments"), "{bad}: {message}");
    }
    // `arguments` may be left out for a tool without parameters.
    let call = mcp.rpc("tools/call", json!({"name": "project_info"}));
    assert_eq!(call["result"]["isError"], false, "{call}");
    assert!(mcp.finish().success(), "stdin closed: it exits 0");
}

#[test]
fn read_only_exposes_only_inspect_observe_and_render() {
    let dir = fixture("sine", "read-only");
    let before = fs::read_to_string(dir.join("project.toml")).unwrap();
    let mut mcp = Mcp::started(&dir, &["--read-only"]);
    let list = mcp.rpc("tools/list", json!({}));
    assert_eq!(
        names(&list),
        [
            "project_info",
            "graph",
            "modules",
            "describe_instance",
            "read_file",
            "stats",
            "meter",
            "scope",
            "read_stream",
            "render",
        ]
    );
    for (name, arguments) in [
        (
            "set_parameter",
            json!({"address": "/osc.freq", "value": 220}),
        ),
        ("commit", json!({"address": "/osc.freq"})),
        (
            "write_file",
            json!({"file": "project.toml", "text": "", "base": null}),
        ),
        (
            "patch",
            json!({"ops": [{"op": "instance.remove", "name": "osc"}]}),
        ),
        ("check", json!({})),
        ("session_start", json!({})),
    ] {
        let refused = mcp.rpc("tools/call", json!({"name": name, "arguments": arguments}));
        assert_eq!(refused["error"]["code"], -32602, "{name}: {refused}");
    }
    assert_eq!(mcp.ok("project_info", json!({}))["readOnly"], true);
    let rendered = mcp.ok("render", json!({"seconds": 0.1}));
    assert!(fs::metadata(rendered["wav"].as_str().unwrap()).is_ok());
    assert!(mcp.finish().success());
    assert_eq!(
        fs::read_to_string(dir.join("project.toml")).unwrap(),
        before
    );
}

#[test]
fn help_names_the_client_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["mcp", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for wanted in [
        "claude mcp add signal -- <path>/signal mcp <project>",
        "--audio",
        "--max-gain",
        "--read-only",
        "--attach",
    ] {
        assert!(help.contains(wanted), "{wanted} in:\n{help}");
    }
}

// ---------------------------------------------------------------------------
// One test per tool.

#[test]
fn project_info() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-project-info"), &[]);
    let info = mcp.ok("project_info", json!({}));
    assert_eq!(info["project"], "sine");
    assert_eq!(info["sampleRate"], 48000);
    assert_eq!(info["mode"], "in-process");
    assert_eq!(info["driver"], "null", "no --audio: the null driver");
    assert_eq!(info["maxGain"], 0.25);
    assert_eq!(info["playing"], true);
    assert_eq!(info["files"][""].as_str().unwrap().len(), 64);
    let gain = &info["gain"][0];
    assert_eq!(gain["port"], "/out.gain");
    assert_eq!(gain["aboveCeiling"], true, "the file's 0.5: {info}");
    assert!(mcp.finish().success());
}

#[test]
fn graph() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-graph"), &[]);
    let graph = mcp.ok("graph", json!({}));
    assert_eq!(graph["top"]["instances"].as_array().unwrap().len(), 2);
    let top = mcp.ok("graph", json!({"level": ""}));
    assert_eq!(top["inside"]["connections"].as_array().unwrap().len(), 2);
    assert!(
        mcp.refused("graph", json!({"level": "nope"}))
            .contains("nope")
    );
    assert!(mcp.finish().success());
}

#[test]
fn modules() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-modules"), &[]);
    let found = mcp.ok("modules", json!({"query": "sine"}));
    let primitives = found["primitives"].as_array().unwrap();
    assert!(primitives.iter().any(|m| m["module"] == "std.sine_osc"));
    let filters = mcp.ok("modules", json!({"category": "filter"}));
    let filters = filters["primitives"].as_array().unwrap();
    assert!(!filters.is_empty() && filters.iter().all(|m| m["category"] == "filter"));
    let all = mcp.ok("modules", json!({}));
    assert!(
        all["composites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["module"] == "lib.tape_echo")
    );
    assert!(mcp.finish().success());
}

#[test]
fn describe_instance() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-describe"), &[]);
    let osc = mcp.ok("describe_instance", json!({"address": "osc"}));
    assert_eq!(osc["address"], "/osc");
    assert_eq!(osc["instances"][0]["module"], "std.sine_osc");
    assert_eq!(osc["controls"][0]["address"], "/osc.freq");
    assert!(
        mcp.refused("describe_instance", json!({"address": "/nope"}))
            .contains("/nope")
    );
    assert!(mcp.finish().success());
}

#[test]
fn read_file() {
    let dir = copy_of("studio", "tool-read-file");
    let mut mcp = Mcp::started(&dir, &[]);
    let project = mcp.ok("read_file", json!({"file": "project.toml"}));
    assert_eq!(project["text"], mcp.file("project.toml"));
    assert_eq!(project["base"].as_str().unwrap().len(), 64);
    let composite = mcp.ok("read_file", json!({"file": "composites/tape_delay.toml"}));
    assert_eq!(composite["text"], mcp.file("composites/tape_delay.toml"));
    let pattern = mcp.ok("read_file", json!({"file": "patterns/melody.toml"}));
    assert_eq!(pattern["kind"], "notes");
    let panel = mcp.ok("read_file", json!({"file": "ui/main.html"}));
    assert_eq!(panel["text"], mcp.file("ui/main.html"));
    assert!(panel["html"].is_string());
    let positions = mcp.ok("read_file", json!({"file": "ui/schematic.toml"}));
    assert_eq!(positions["exists"], false);
    assert!(
        mcp.refused("read_file", json!({"file": "../outside.toml"}))
            .contains("bad-value")
    );
    assert!(mcp.finish().success());
}

#[test]
fn stats() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-stats"), &[]);
    let stats = mcp.ok("stats", json!({}));
    assert!(stats["frame"].as_u64().is_some());
    assert_eq!(stats["clients"].as_array().unwrap().len(), 1);
    assert!(mcp.finish().success());
}

#[test]
fn set_parameter() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-set"), &[]);
    let ack = mcp.ok(
        "set_parameter",
        json!({"address": "/osc.freq", "value": 330}),
    );
    assert_eq!(ack["outcome"], "scheduled");
    assert!(ack["frame"].as_u64().is_some());
    assert_eq!(control_base(&mut mcp, "/osc.freq"), 330.0);
    assert!(
        mcp.file("project.toml").contains("freq = 440"),
        "not committed"
    );
    let unknown = mcp.refused("set_parameter", json!({"address": "/nope.x", "value": 1}));
    assert!(unknown.contains("unknown-address"), "{unknown}");
    assert!(mcp.finish().success());
}

#[test]
fn ramp_parameter() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-ramp"), &[]);
    let ack = mcp.ok(
        "ramp_parameter",
        json!({"address": "/osc.freq", "to": 220, "ms": 5}),
    );
    assert_eq!(ack["outcome"], "scheduled");
    assert_eq!(control_base(&mut mcp, "/osc.freq"), 220.0);
    assert!(mcp.finish().success());
}

#[test]
fn trigger() {
    let mut mcp = Mcp::started(&copy_of("studio", "tool-trigger"), &[]);
    let ack = mcp.ok("trigger", json!({"address": "/melody.reset"}));
    assert_eq!(ack["outcome"], "scheduled", "{ack}");
    assert!(mcp.finish().success());
}

#[test]
fn transport() {
    let mut mcp = Mcp::started(&copy_of("studio", "tool-transport"), &[]);
    let stopped = mcp.ok("transport", json!({"action": "stop"}));
    assert_eq!(stopped["playing"], false);
    assert_eq!(mcp.ok("project_info", json!({}))["playing"], false);
    let started = mcp.ok("transport", json!({"action": "start"}));
    assert_eq!(started["playing"], true);
    let tempo = mcp.ok("transport", json!({"action": "tempo", "bpm": 100}));
    assert_eq!(tempo["addresses"], json!(["/transport.bpm"]));
    assert_eq!(control_base(&mut mcp, "/transport.bpm"), 100.0);
    assert!(mcp.finish().success());
    let mut sine = Mcp::started(&fixture("sine", "tool-transport-none"), &[]);
    let none = sine.refused("transport", json!({"action": "tempo", "bpm": 100}));
    assert!(none.contains("no std.transport"), "{none}");
    assert!(none.contains("no std.playhead"), "{none}");
    assert!(sine.finish().success());
}

/// The tempo of `lib.transport` (a std.playhead, ADR 0033): studio-transport's
/// `/transport.bpm`, set live as `set_parameter` sets a control (the file
/// unchanged until a commit), and ramped.
#[test]
fn transport_tempo_on_a_playhead() {
    let mut mcp = Mcp::started(&fixture("studio-transport", "tool-tempo-playhead"), &[]);
    let file = mcp.file("project.toml");
    let tempo = mcp.ok("transport", json!({"action": "tempo", "bpm": 140}));
    assert_eq!(tempo["addresses"], json!(["/transport.bpm"]), "{tempo}");
    assert_eq!(tempo["skipped"], json!([]), "{tempo}");
    assert_eq!(tempo["acks"][0]["outcome"], "scheduled", "{tempo}");
    assert_eq!(control_base(&mut mcp, "/transport.bpm"), 140.0);
    assert_eq!(mcp.file("project.toml"), file, "a live set, not committed");
    let ramp = mcp.ok("transport", json!({"action": "tempo", "bpm": 90, "ms": 5}));
    assert_eq!(ramp["acks"][0]["outcome"], "scheduled", "{ramp}");
    assert_eq!(control_base(&mut mcp, "/transport.bpm"), 90.0);
    mcp.ok("commit", json!({"address": "/transport.bpm"}));
    assert!(
        mcp.file("project.toml").contains("bpm = 90"),
        "{}",
        mcp.file("project.toml")
    );
    let streams = mcp.ok("read_stream", json!({}));
    assert_eq!(
        streams["transport"][0]["instance"], "/transport/playhead",
        "{streams}"
    );
    assert!(streams["transport"][0]["bpm"].is_number(), "{streams}");
    assert!(mcp.finish().success());
}

/// Several transports: every top-level control reaching an unwired bpm is
/// set together; one a connection drives is skipped; only wired ones: refused.
#[test]
fn transport_tempo_on_several_playheads() {
    let text = |c_bpm: &str| {
        format!(
            "schema_version = 1\n\
             [project]\nname = \"tempi\"\nsample_rate = 48000\nseed = 1\n\
             [instances]\n\
             a = {{ module = \"lib.transport\" }}\n\
             b = {{ module = \"std.playhead\", bpm = 100 }}\n\
             c = {{ module = \"lib.transport\" }}\n\
             k = {{ module = \"std.const\", value = 128 }}\n\
             osc = {{ module = \"std.sine_osc\", freq = 440 }}\n\
             out = {{ module = \"std.audio_out\", gain = 0.5 }}\n\
             [wiring]\nconnections = [\n\
             {c_bpm}\
             {{ from = \"osc.out\", to = \"out.l\" }},\n\
             {{ from = \"osc.out\", to = \"out.r\" }},\n]\n"
        )
    };
    let mut mcp = Mcp::started(
        &mcp_common::project(
            &text("{ from = \"k.out\", to = \"c.bpm\" },\n"),
            "tool-tempo-several",
        ),
        &[],
    );
    let tempo = mcp.ok("transport", json!({"action": "tempo", "bpm": 132}));
    assert_eq!(tempo["addresses"], json!(["/a.bpm", "/b.bpm"]), "{tempo}");
    assert_eq!(tempo["skipped"], json!(["/c.bpm"]), "{tempo}");
    assert_eq!(control_base(&mut mcp, "/a.bpm"), 132.0);
    assert_eq!(control_base(&mut mcp, "/b.bpm"), 132.0);
    assert!(mcp.finish().success());

    let wired = "{ from = \"k.out\", to = \"a.bpm\" },\n\
                 { from = \"k.out\", to = \"b.bpm\" },\n\
                 { from = \"k.out\", to = \"c.bpm\" },\n";
    let mut mcp = Mcp::started(&mcp_common::project(&text(wired), "tool-tempo-wired"), &[]);
    let refused = mcp.refused("transport", json!({"action": "tempo", "bpm": 132}));
    assert!(refused.contains("a connection drives"), "{refused}");
    assert!(refused.contains("/a.bpm, /b.bpm, /c.bpm"), "{refused}");
    assert!(mcp.finish().success());
}

#[test]
fn commit() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-commit"), &[]);
    mcp.ok(
        "set_parameter",
        json!({"address": "/osc.freq", "value": 330}),
    );
    let committed = mcp.ok("commit", json!({"address": "/osc.freq"}));
    assert_eq!(committed["value"], 330.0);
    assert!(
        mcp.file("project.toml").contains("freq = 330"),
        "{}",
        mcp.file("project.toml")
    );
    mcp.ok("commit", json!({"address": "/osc.freq", "value": 220}));
    assert!(mcp.file("project.toml").contains("freq = 220"));
    assert!(mcp.finish().success());
}

#[test]
fn revert() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-revert"), &[]);
    mcp.ok(
        "set_parameter",
        json!({"address": "/osc.freq", "value": 300}),
    );
    let reverted = mcp.ok("revert", json!({"address": "/osc.freq"}));
    assert_eq!(reverted["value"], 440.0);
    assert_eq!(control_base(&mut mcp, "/osc.freq"), 440.0);
    assert!(
        mcp.refused("revert", json!({"address": "/osc.freq"}))
            .contains("not-unsaved")
    );
    assert!(mcp.finish().success());
}

#[test]
fn patch() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-patch"), &[]);
    let patched = mcp.ok(
        "patch",
        json!({"ops": [
            {"op": "instance.add", "name": "lfo", "module": "std.lfo", "params": {"freq_hz": 2}},
            {"op": "connection.add", "from": "lfo.out", "to": "osc.freq", "scale": 10},
        ]}),
    );
    assert_eq!(patched["file"], "project.toml");
    assert!(patched["inverse"].is_array());
    let text = mcp.file("project.toml");
    assert!(
        text.contains("lfo = { module = \"std.lfo\", freq_hz = 2 }"),
        "{text}"
    );
    assert!(
        text.contains("{ from = \"lfo.out\", to = \"osc.freq\", scale = 10 }"),
        "{text}"
    );
    let stale = mcp.refusal(
        "patch",
        json!({"base": stale(), "ops": [{"op": "instance.remove", "name": "lfo"}]}),
    );
    assert_eq!(stale["code"], "conflict");
    assert!(mcp.finish().success());
}

#[test]
fn schematic_move() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-move"), &[]);
    let moved = mcp.ok(
        "schematic_move",
        json!({"instance": "osc", "x": 10.4, "y": -20}),
    );
    assert_eq!(moved["x"], 10);
    let text = mcp.file("ui/schematic.toml");
    assert!(text.contains("osc"), "{text}");
    assert!(mcp.finish().success());
}

#[test]
fn vendor() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-vendor"), &[]);
    let vendored = mcp.ok(
        "vendor",
        json!({"composite": "lib.tape_echo", "as": "echo"}),
    );
    assert!(
        vendored["copied"]
            .as_array()
            .unwrap()
            .contains(&json!("composites/echo.toml"))
    );
    assert!(mcp.dir.join("composites/echo.toml").is_file());
    let again = mcp.refused(
        "vendor",
        json!({"composite": "lib.tape_echo", "as": "echo"}),
    );
    assert!(again.contains("exists"), "{again}");
    assert!(mcp.finish().success());
}

#[test]
fn write_pattern() {
    let mut mcp = Mcp::started(&copy_of("studio", "tool-pattern"), &[]);
    let read = mcp.ok("read_file", json!({"file": "patterns/melody.toml"}));
    let text = read["text"].as_str().unwrap();
    let edited = format!("{text}\n# edited by the agent\n");
    let written = mcp.ok(
        "write_pattern",
        json!({"name": "melody", "text": edited, "base": read["base"]}),
    );
    assert_eq!(written["file"], "patterns/melody.toml");
    assert_eq!(mcp.file("patterns/melody.toml"), edited);
    let conflict = mcp.refusal(
        "write_pattern",
        json!({"name": "melody", "text": text, "base": read["base"]}),
    );
    assert_eq!(conflict["code"], "conflict");
    assert_eq!(conflict["text"], edited, "the current text");
    assert!(mcp.finish().success());
}

#[test]
fn write_panel() {
    let mut mcp = Mcp::started(&copy_of("studio", "tool-panel"), &[]);
    let read = mcp.ok("read_file", json!({"file": "ui/main.html"}));
    let written = mcp.ok(
        "write_panel",
        json!({"file": "main.html", "html": read["html"], "base": read["base"]}),
    );
    assert_eq!(written["created"], false);
    let refused = mcp.refusal(
        "write_panel",
        json!({"html": "<script>alert(1)</script>", "base": written["base"]}),
    );
    assert_eq!(refused["code"], "bad-html");
    assert!(mcp.finish().success());
}

#[test]
fn write_file() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-write-file"), &[]);
    let read = mcp.ok("read_file", json!({"file": "project.toml"}));
    let text = read["text"].as_str().unwrap();
    let edited = text.replace("freq = 440", "freq = 330");
    let written = mcp.ok(
        "write_file",
        json!({"file": "project.toml", "text": edited, "base": read["base"]}),
    );
    assert_eq!(written["file"], "project.toml");
    assert_eq!(mcp.file("project.toml"), edited);
    let conflict = mcp.refusal(
        "write_file",
        json!({"file": "project.toml", "text": text, "base": read["base"]}),
    );
    assert_eq!(conflict["code"], "conflict");
    assert_eq!(conflict["text"], edited, "the current text");
    let broken = mcp.refusal(
        "write_file",
        json!({"file": "project.toml", "text": "schema_version = 1\n", "base": written["base"]}),
    );
    assert_eq!(broken["code"], "does-not-load");
    assert!(
        mcp.refused(
            "write_file",
            json!({"file": "ui/main.html", "text": "", "base": null})
        )
        .contains("write_panel")
    );
    assert_eq!(mcp.file("project.toml"), edited, "refusals write nothing");
    assert!(mcp.finish().success());
}

#[test]
fn meter() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-meter"), &[]);
    let levels = mcp.ok("meter", json!({"window_ms": 300}));
    let channels = levels["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 2, "{levels}");
    let peak = channels[0]["peak"].as_f64().unwrap();
    // A sine at gain 0.5, held at the output ceiling (--max-gain 0.25).
    assert_eq!(peak, 0.25, "a sine at gain 0.5, held at 0.25: {levels}");
    let ports = mcp.ok("meter", json!({"ports": ["/osc.out"], "window_ms": 100}));
    assert!(ports["ports"][0]["peak"].as_f64().unwrap() > 0.9, "{ports}");
    let short = mcp.refused("meter", json!({"ports": ["/osc.out"], "window_ms": 10}));
    assert!(short.contains("at least"), "{short}");
    assert!(mcp.finish().success());
}

#[test]
fn scope() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-scope"), &[]);
    let scope = mcp.ok("scope", json!({"port": "/osc.out", "points": 64}));
    let points = scope["points"].as_array().unwrap();
    assert!(!points.is_empty() && points.len() <= 64, "{scope}");
    assert!(scope["peak"].as_f64().unwrap() > 0.9);
    assert!(
        mcp.refused("scope", json!({"port": "/nope.out"}))
            .contains("not-a-signal-output")
    );
    assert!(mcp.finish().success());
}

#[test]
fn read_stream() {
    let mut mcp = Mcp::started(&copy_of("studio", "tool-read-stream"), &[]);
    let read = mcp.ok("read_stream", json!({}));
    let streams = read["streams"].as_array().unwrap();
    let step = streams.iter().find(|s| s["port"] == "/drums.step").unwrap();
    let (min, max) = (step["min"].as_f64().unwrap(), step["max"].as_f64().unwrap());
    assert!((0.0..=255.0).contains(&min) && max <= 255.0, "{step}");
    assert!(
        max > min,
        "the step moves within a second at 120 bpm: {step}"
    );
    assert_eq!(step["step"].as_f64().unwrap().fract(), 0.0);
    let position = streams
        .iter()
        .find(|s| s["port"] == "/melody.position")
        .unwrap();
    assert!(
        position["last"].as_f64().unwrap() != position["first"].as_f64().unwrap(),
        "{position}"
    );
    assert_eq!(read["transport"][0]["bpm"], 120.0);
    assert!(mcp.finish().success());
}

#[test]
fn render() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-render"), &[]);
    let rendered = mcp.ok("render", json!({"seconds": 0.5}));
    assert_eq!(rendered["frames"], 24000);
    let wav = rendered["wav"].as_str().unwrap();
    let out = mcp.out.canonicalize().unwrap();
    assert!(
        Path::new(wav).starts_with(&out),
        "under --out ({}): {wav}",
        out.display()
    );
    assert_eq!(rendered["played"], false);
    assert!(fs::metadata(wav).unwrap().len() > 24000 * 8);
    assert_eq!(
        rendered["hashes"]["audio_fnv1a64"].as_str().unwrap().len(),
        16
    );
    assert!(rendered["metrics"].is_object());
    assert!(mcp.finish().success());
}

#[test]
fn ui_check() {
    let mut mcp = Mcp::started(&copy_of("studio", "tool-ui-check"), &[]);
    let checked = mcp.ok("ui_check", json!({}));
    assert_eq!(checked["ok"], true, "{checked}");
    assert_eq!(checked["panels"][0]["file"], "ui/main.html");
    assert!(mcp.finish().success());
}

#[test]
fn check() {
    let dir = fixture("sine", "tool-check");
    let mut mcp = Mcp::started(&dir, &[]);
    assert_eq!(mcp.ok("check", json!({}))["ok"], true);
    assert!(mcp.finish().success());
    fs::write(
        dir.join("project.toml"),
        fs::read_to_string(dir.join("project.toml"))
            .unwrap()
            .replace("std.sine_osc", "std.no_such"),
    )
    .unwrap();
    // A project that does not load: the server does not start.
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .arg("mcp")
        .arg(&dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "nothing on stdout");
}

#[test]
fn library() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-library"), &[]);
    let library = mcp.ok("library", json!({}));
    let composites = library["composites"].as_array().unwrap();
    assert!(composites.iter().any(|c| c["module"] == "lib.tape_echo"));
    assert!(library["changelog"].as_str().unwrap().contains('#'));
    assert!(library["version"].is_string());
    assert!(mcp.finish().success());
}

/// The session tools: `session_start`, `session_stop`, `replay` (AC13).
#[test]
fn session_start_stop_and_replay() {
    let mut mcp = Mcp::started(&fixture("sine", "tool-session"), &[]);
    mcp.ok(
        "set_parameter",
        json!({"address": "/osc.freq", "value": 330}),
    );
    let unsaved = mcp.refused("session_start", json!({}));
    assert!(unsaved.contains("unsaved"), "{unsaved}");
    assert!(
        mcp.refused("session_stop", json!({}))
            .contains("session_start")
    );
    assert_eq!(
        mcp.ok("session_start", json!({"discard": true}))["recording"],
        true
    );
    mcp.ok(
        "set_parameter",
        json!({"address": "/osc.freq", "value": 220}),
    );
    thread::sleep(Duration::from_millis(300));
    let stopped = mcp.ok("session_stop", json!({}));
    let session = stopped["session"].as_str().unwrap();
    assert!(fs::metadata(format!("{session}/session.toml")).is_ok());
    assert!(fs::metadata(stopped["wav"].as_str().unwrap()).is_ok());
    let replayed = mcp.ok("replay", json!({}));
    assert_eq!(replayed["identical"], true, "{replayed}");
    assert_eq!(replayed["capturedFnv1a64"], stopped["capturedFnv1a64"]);
    // The sine plays at gain 0.5: the capture is the device's output, held
    // at the 0.25 ceiling, so it equals the replay held the same way, not
    // the replay's own audio.
    assert_eq!(stopped["ceiling"], 0.25);
    assert_eq!(replayed["ceiling"], 0.25);
    assert_eq!(replayed["heldFnv1a64"], stopped["capturedFnv1a64"]);
    assert_ne!(replayed["audio_fnv1a64"], replayed["heldFnv1a64"]);
    assert_eq!(
        mcp.ok("project_info", json!({}))["playing"],
        true,
        "plays on"
    );
    assert!(mcp.finish().success());
}

// ---------------------------------------------------------------------------
// AC11: a scripted agent session on a copy of projects/studio.

#[test]
fn a_scripted_session_on_studio_lands_in_text_and_replays_bit_identically() {
    let dir = copy_of("studio", "ac11-studio");
    let mut mcp = Mcp::started(&dir, &[]);
    mcp.ok("session_start", json!({}));

    // A knob: the bus's first level, set live, then committed into the text.
    mcp.ok(
        "set_parameter",
        json!({"address": "/bus.gain_a", "value": 0.4}),
    );
    mcp.ok("commit", json!({"address": "/bus.gain_a"}));
    assert!(
        mcp.file("project.toml").contains("gain_a = 0.4"),
        "{}",
        mcp.file("project.toml")
    );

    // A pattern: the melody's first note an octave up.
    let melody = mcp.ok("read_file", json!({"file": "patterns/melody.toml"}));
    let text = melody["text"].as_str().unwrap();
    let edited = text.replacen("{ key = 33, start = 0,", "{ key = 45, start = 0,", 1);
    assert_ne!(edited, text);
    mcp.ok(
        "write_pattern",
        json!({"name": "melody", "text": edited, "base": melody["base"]}),
    );
    assert_eq!(mcp.file("patterns/melody.toml"), edited);

    // A module and a wire: a low-pass after the DC blocker, into the bus.
    let graph = mcp.ok("graph", json!({}));
    let base = graph["files"][""].clone();
    mcp.ok(
        "patch",
        json!({"base": base, "ops": [
            {"op": "instance.add", "name": "air", "module": "std.onepole", "params": {"cutoff_hz": 8000}},
            {"op": "connection.add", "from": "dc.out", "to": "air.in"},
        ]}),
    );
    let text = mcp.file("project.toml");
    assert!(
        text.contains("air = { module = \"std.onepole\", cutoff_hz = 8000 }"),
        "{text}"
    );
    assert!(
        text.contains("{ from = \"dc.out\", to = \"air.in\" }"),
        "{text}"
    );
    // The reload swaps it in.
    let until = Instant::now() + Duration::from_secs(20);
    while mcp
        .call("describe_instance", json!({"address": "/air"}))
        .get("isError")
        == Some(&json!(true))
    {
        assert!(Instant::now() < until, "the patch was not applied live");
        thread::sleep(Duration::from_millis(50));
    }

    // A meter: the output plays.
    let levels = mcp.ok("meter", json!({"window_ms": 500}));
    assert!(
        levels["channels"][0]["peak"].as_f64().unwrap() > 0.0,
        "{levels}"
    );
    assert!(levels["channels"][0]["peak"].as_f64().unwrap() <= 0.25 + 1e-6);

    // A render of the text as it is now.
    let rendered = mcp.ok("render", json!({"seconds": 1}));
    assert_eq!(rendered["frames"], 48000);

    // The session: written, and its replay is the run, bit for bit.
    let stopped = mcp.ok("session_stop", json!({}));
    let replayed = mcp.ok("replay", json!({}));
    assert_eq!(replayed["identical"], true, "{replayed}");
    assert_eq!(replayed["audio_fnv1a64"], stopped["capturedFnv1a64"]);
    let session = stopped["session"].as_str().unwrap();
    let edits = fs::read_to_string(format!("{session}/edits.jsonl")).unwrap();
    assert!(!edits.is_empty(), "the session recorded the edits");
    assert!(
        fs::read_dir(format!("{session}/projects")).unwrap().count() > 1,
        "the patch and the pattern were swapped in as new versions"
    );
    assert!(mcp.finish().success());

    // Every change is in the text: what the files hold now.
    let text = fs::read_to_string(dir.join("project.toml")).unwrap();
    assert!(text.contains("gain_a = 0.4") && text.contains("air = {"));
    assert_eq!(
        fs::read_to_string(dir.join("patterns/melody.toml")).unwrap(),
        edited
    );
}

// ---------------------------------------------------------------------------
// AC12/AC13, loopback: attached to a running `signal ui` (needs a TCP port;
// refused where no port can be bound).

#[test]
fn attach_to_a_running_signal_ui() {
    let dir = fixture("sine", "attach");
    let setup = dir.parent().unwrap().join("setup.toml");
    let mut ui = Command::new(env!("CARGO_BIN_EXE_signal"))
        .arg("--setup")
        .arg(&setup)
        .args(["ui", "--driver", "null", "--seconds", "60"])
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(ui.stderr.take().unwrap());
    let mut url = None;
    let mut line = String::new();
    while url.is_none() && stderr.read_line(&mut line).unwrap() > 0 {
        url = line
            .trim()
            .strip_prefix("signal: ui at ")
            .map(str::to_owned);
        line.clear();
    }
    let url = url.expect("signal ui's URL on stderr");
    let mut mcp = Mcp::started(&dir, &["--attach", &url]);
    let names = names(&mcp.rpc("tools/list", json!({})));
    assert!(
        !names.contains(&"session_start".to_owned()),
        "no sessions attached"
    );
    let info = mcp.ok("project_info", json!({}));
    assert_eq!(info["mode"], "attached");
    assert_eq!(info["project"], "sine");
    mcp.ok(
        "set_parameter",
        json!({"address": "/osc.freq", "value": 330}),
    );
    mcp.ok("commit", json!({"address": "/osc.freq"}));
    assert!(mcp.file("project.toml").contains("freq = 330"));
    let gain = mcp.refused("set_parameter", json!({"address": "/out.gain", "value": 1}));
    assert!(gain.contains("--max-gain"), "{gain}");
    assert!(mcp.finish().success());
    let _ = ui.kill();
    let _ = ui.wait();

    // Another folder than the one the server plays is refused.
    let other = fixture("sine", "attach-other");
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["mcp", "--attach", "ws://192.168.0.1:1/#token=x"])
        .arg(&other)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success(), "loopback only");
    assert!(String::from_utf8_lossy(&output.stderr).contains("this machine only"));
}

/// review M2, L7: resources are the project's own text files, panels and
/// positions included, never a link nor a file in a linked folder (as
/// read_file refuses them).
#[test]
fn resources_are_the_project_s_own_files() {
    let dir = fixture("sine", "resources-links");
    let outside = dir.parent().unwrap().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.toml"), "secret = \"hunter2\"\n").unwrap();
    fs::create_dir_all(dir.join("ui")).unwrap();
    fs::write(dir.join("ui/main.html"), "<signal-panel></signal-panel>\n").unwrap();
    fs::write(dir.join("ui/schematic.toml"), "[positions]\n").unwrap();
    let mut mcp = Mcp::started(&dir, &[]);
    // Links made while it plays (the project would not load with them).
    fs::create_dir_all(dir.join("composites")).unwrap();
    std::os::unix::fs::symlink(
        outside.join("secret.toml"),
        dir.join("composites/evil.toml"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.join("secret.toml"), dir.join("ui/evil.html")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("patterns")).unwrap();
    let list = mcp.rpc("resources/list", json!({}));
    let uris: Vec<&str> = list["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    for uri in [
        "signal://project/project.toml",
        "signal://project/ui/main.html",
        "signal://project/ui/schematic.toml",
    ] {
        assert!(uris.contains(&uri), "{uri}: {uris:?}");
    }
    for uri in &uris {
        assert!(!uri.contains("evil") && !uri.contains("secret"), "{uris:?}");
    }
    let panel = mcp.rpc(
        "resources/read",
        json!({"uri": "signal://project/ui/main.html"}),
    );
    assert_eq!(panel["result"]["contents"][0]["mimeType"], "text/html");
    for uri in [
        "signal://project/composites/evil.toml",
        "signal://project/ui/evil.html",
        "signal://project/patterns/secret.toml",
        "signal://project/../outside/secret.toml",
    ] {
        let read = mcp.rpc("resources/read", json!({"uri": uri}));
        assert_eq!(read["error"]["code"], -32002, "{uri}: {read}");
    }
    assert!(mcp.finish().success());
}

/// review L5: an observation that receives nothing is an error, not an
/// empty success.
#[test]
fn an_observation_without_data_is_an_error() {
    let mut mcp = Mcp::started(&fixture("sine", "no-data"), &[]);
    mcp.ok("transport", json!({"action": "stop"}));
    let meter = mcp.refused("meter", json!({"window_ms": 100}));
    assert!(meter.contains("no data arrived"), "{meter}");
    mcp.ok("transport", json!({"action": "start"}));
    mcp.ok("meter", json!({"window_ms": 100}));
    assert!(mcp.finish().success());
}
