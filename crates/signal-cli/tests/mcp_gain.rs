//! The gain ceiling of `signal mcp` (ADR 0029 §3, AC7): a set, ramp,
//! commit, revert, trigger, patch or write that would put a `std.audio_out`
//! gain above `--max-gain` (default 0.25) is refused with a clear error and
//! writes nothing. The rules (R1, static after; R2, modulated after) are in
//! `crates/signal-cli/src/mcp/gain.rs`; each case here names the one that
//! refuses it. On copies, through the built binary, on the null driver.

mod mcp_common;

use std::thread;
use std::time::{Duration, Instant};

use mcp_common::{Mcp, fixture, project};
use serde_json::{Value, json};

const HEAD: &str =
    "schema_version = 1\n\n[project]\nname = \"gain\"\nsample_rate = 48000\nseed = 1\n\n";

/// A sine into a sink at `gain` (static: nothing into the gain).
fn tone(gain: &str) -> String {
    format!(
        "{HEAD}[instances]\nosc = {{ module = \"std.sine_osc\", freq = 440 }}\nout = {{ module = \"std.audio_out\", gain = {gain} }}\n\n[wiring]\nconnections = [\n  {{ from = \"osc.out\", to = \"out.l\" }},\n  {{ from = \"osc.out\", to = \"out.r\" }},\n]\n"
    )
}

/// A sink at `gain` whose gain a `std.const` of `value` modulates.
fn modulated(gain: &str, value: &str) -> String {
    format!(
        "{HEAD}[instances]\nosc = {{ module = \"std.sine_osc\", freq = 440 }}\nm = {{ module = \"std.const\", value = {value} }}\nout = {{ module = \"std.audio_out\", gain = {gain} }}\n\n[wiring]\nconnections = [\n  {{ from = \"osc.out\", to = \"out.l\" }},\n  {{ from = \"osc.out\", to = \"out.r\" }},\n  {{ from = \"m.out\", to = \"out.gain\" }},\n]\n"
    )
}

/// Refused, naming the ceiling; the project's text unchanged.
fn refused(mcp: &mut Mcp, tool: &str, arguments: Value) -> String {
    let before = mcp.file("project.toml");
    let message = mcp.refused(tool, arguments);
    assert!(message.contains("--max-gain"), "{tool}: {message}");
    assert_eq!(mcp.file("project.toml"), before, "{tool} wrote nothing");
    message
}

fn base(mcp: &mut Mcp) -> Value {
    mcp.ok("read_file", json!({"file": "project.toml"}))["base"].clone()
}

fn write(mcp: &mut Mcp, text: &str) -> Value {
    let base = base(mcp);
    mcp.call(
        "write_file",
        json!({"file": "project.toml", "text": text, "base": base}),
    )
}

/// The live base of `/out.gain`.
fn live_gain(mcp: &mut Mcp) -> f64 {
    let info = mcp.ok("project_info", json!({}));
    info["gain"][0]["live"].as_f64().unwrap()
}

/// ADR 0041: restoring a snapshot is a commit of each of its values, so a
/// gain above the ceiling and above the file's is refused (R1) and nothing
/// is written; one within it is restored.
#[test]
fn a_snapshot_above_the_ceiling_is_not_restored() {
    let dir = project(&tone("0.2"), "gain-snapshot");
    std::fs::write(
        dir.join("snapshots.toml"),
        "schema_version = 1\n\n[snapshots.loud]\n\"/out.gain\" = 0.9\n\n[snapshots.quiet]\n\"/out.gain\" = 0.1\n",
    )
    .unwrap();
    let mut mcp = Mcp::started(&dir, &[]);
    refused(&mut mcp, "snapshot_restore", json!({"name": "loud"}));
    mcp.ok("snapshot_restore", json!({"name": "quiet"}));
    assert!(mcp.file("project.toml").contains("gain = 0.1"));
    assert!(mcp.finish().success());
}

#[test]
fn set_and_ramp_above_the_ceiling_are_refused() {
    let mut mcp = Mcp::started(&project(&tone("0.2"), "gain-set"), &[]);
    // R1: static, 0.5 > 0.25 and above the live 0.2.
    refused(
        &mut mcp,
        "set_parameter",
        json!({"address": "/out.gain", "value": 0.5}),
    );
    refused(
        &mut mcp,
        "ramp_parameter",
        json!({"address": "/out.gain", "to": 0.3, "ms": 5}),
    );
    mcp.ok(
        "set_parameter",
        json!({"address": "/out.gain", "value": 0.25}),
    );
    mcp.ok(
        "ramp_parameter",
        json!({"address": "/out.gain", "to": 0.1, "ms": 5}),
    );
    assert!(mcp.finish().success());
    // A higher --max-gain allows it.
    let mut mcp = Mcp::started(
        &project(&tone("0.2"), "gain-set-higher"),
        &["--max-gain", "0.5"],
    );
    mcp.ok(
        "set_parameter",
        json!({"address": "/out.gain", "value": 0.4}),
    );
    assert!(mcp.finish().success());
}

/// A1 (R1): the file's 0.5 lowered live to 0.1, then the file rewritten to
/// 0.4: below the old file value, but the reload raises the live gain from
/// 0.1 to 0.4. A2: a comment-only write is allowed and keeps the live 0.1
/// (the reload rule the guard predicts with). A3 (R1): a revert back to the
/// file's 0.5 is refused.
#[test]
fn a_lowered_live_gain_is_not_raised_by_a_write_or_a_revert() {
    let mut mcp = Mcp::started(&project(&tone("0.5"), "gain-a1"), &[]);
    mcp.ok(
        "set_parameter",
        json!({"address": "/out.gain", "value": 0.1}),
    );
    let text = mcp.file("project.toml");
    let a1 = write(&mut mcp, &text.replace("gain = 0.5", "gain = 0.4"));
    assert_eq!(a1["isError"], true, "{a1}");
    assert!(
        a1["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("from 0.1 to 0.4"),
        "{a1}"
    );
    assert_eq!(mcp.file("project.toml"), text);

    let commented = format!("{text}# a comment\n");
    let a2 = write(&mut mcp, &commented);
    assert_eq!(a2["isError"], false, "{a2}");
    // The reload took the file (no value changed): the live gain stays 0.1.
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        thread::sleep(Duration::from_millis(100));
        assert_eq!(live_gain(&mut mcp), 0.1, "a reload of other keys keeps it");
    }

    refused(&mut mcp, "revert", json!({"address": "/out.gain"}));
    assert!(mcp.finish().success());
}

/// A4 (the file baseline): a commit never writes an over-ceiling value the
/// file does not hold; committing the file's own value is allowed.
#[test]
fn commit_takes_the_file_value_as_its_baseline() {
    let mut mcp = Mcp::started(&project(&tone("0.2"), "gain-a4"), &[]);
    let message = refused(
        &mut mcp,
        "commit",
        json!({"address": "/out.gain", "value": 0.3}),
    );
    assert!(
        message.contains("into project.toml"),
        "the file baseline: {message}"
    );
    mcp.ok("commit", json!({"address": "/out.gain", "value": 0.1}));
    assert!(mcp.file("project.toml").contains("gain = 0.1"));
    assert!(mcp.finish().success());
    let mut held = Mcp::started(&project(&tone("0.5"), "gain-a4-held"), &[]);
    held.ok("commit", json!({"address": "/out.gain"}));
    assert!(held.finish().success());
}

/// B1 (R1, modulated before): base 1 modulated by a const 0.1 (effective
/// 0.1); removing the connection while the const stays would make it static
/// at 1. B2: the same through write_file. B3 (R2): lowering the base is
/// allowed. B4 (R2): an unrelated module is allowed.
#[test]
fn a_modulated_gain_does_not_become_static_above_the_ceiling() {
    let mut mcp = Mcp::started(&project(&modulated("1", "0.1"), "gain-b"), &[]);
    let b1 = refused(
        &mut mcp,
        "patch",
        json!({"ops": [{"op": "connection.remove", "from": "m.out", "to": "out.gain"}]}),
    );
    assert!(b1.contains("modulated before"), "{b1}");
    let text = mcp.file("project.toml");
    let without = text.replace("  { from = \"m.out\", to = \"out.gain\" },\n", "");
    assert_ne!(without, text);
    let b2 = write(&mut mcp, &without);
    assert_eq!(b2["isError"], true, "{b2}");
    mcp.ok(
        "set_parameter",
        json!({"address": "/out.gain", "value": 0.5}),
    );
    mcp.ok(
        "patch",
        json!({"ops": [{"op": "instance.add", "name": "spare", "module": "std.sine_osc"}]}),
    );
    // Removing the modulator's instance leaves its modulation held (R2).
    refused(
        &mut mcp,
        "patch",
        json!({"ops": [{"op": "instance.remove", "name": "m"}]}),
    );
    assert!(mcp.finish().success());
}

/// R2: with an existing modulation of 2 and base 0.1 (effective 0.2),
/// raising the base to 0.2 doubles the effective gain to 0.4; so does a
/// change upstream (the const), live or in the file, a rescaled connection,
/// or a new connection into the gain. --max-gain 4 (the range) allows them.
#[test]
fn what_modulates_a_gain_is_not_changed_below_the_clamp() {
    let mut mcp = Mcp::started(&project(&modulated("0.1", "2"), "gain-r2"), &[]);
    refused(
        &mut mcp,
        "set_parameter",
        json!({"address": "/out.gain", "value": 0.2}),
    );
    refused(
        &mut mcp,
        "set_parameter",
        json!({"address": "/m.value", "value": 3}),
    );
    let text = mcp.file("project.toml");
    let upstream = write(&mut mcp, &text.replace("value = 2", "value = 3"));
    assert_eq!(upstream["isError"], true, "{upstream}");
    refused(
        &mut mcp,
        "patch",
        json!({"ops": [{"op": "connection.scale", "from": "m.out", "to": "out.gain", "scale": 2}]}),
    );
    mcp.ok(
        "set_parameter",
        json!({"address": "/out.gain", "value": 0.05}),
    );
    assert!(mcp.finish().success());

    let mut open = Mcp::started(
        &project(&modulated("0.1", "2"), "gain-r2-open"),
        &["--max-gain", "4"],
    );
    open.ok("set_parameter", json!({"address": "/m.value", "value": 3}));
    assert!(open.finish().success());

    let mut fresh = Mcp::started(&project(&tone("0.2"), "gain-new-connection"), &[]);
    refused(
        &mut fresh,
        "patch",
        json!({"ops": [
            {"op": "instance.add", "name": "lfo", "module": "std.lfo"},
            {"op": "connection.add", "from": "lfo.out", "to": "out.gain"},
        ]}),
    );
    assert!(fresh.finish().success());
}

/// A trigger reaching what modulates a gain changes it (R2).
#[test]
fn a_trigger_into_a_gain_s_modulation_is_refused() {
    let text = format!(
        "{HEAD}[instances]\nosc = {{ module = \"std.sine_osc\" }}\nenv = {{ module = \"std.exp_env\", decay_ms = 50 }}\nout = {{ module = \"std.audio_out\", gain = 0.1 }}\n\n[wiring]\nconnections = [\n  {{ from = \"osc.out\", to = \"out.l\" }},\n  {{ from = \"env.out\", to = \"out.gain\" }},\n]\n"
    );
    let mut mcp = Mcp::started(&project(&text, "gain-trigger"), &[]);
    refused(&mut mcp, "trigger", json!({"address": "/env.trigger"}));
    assert!(mcp.finish().success());
}

/// write_file of project.toml above the ceiling (R1), and a new sink
/// without a gain key: the declared default 1.0 (R1).
#[test]
fn write_file_above_the_ceiling_and_a_new_sink_are_refused() {
    let mut mcp = Mcp::started(&project(&tone("0.2"), "gain-write"), &[]);
    let text = mcp.file("project.toml");
    let raised = write(&mut mcp, &text.replace("gain = 0.2", "gain = 0.5"));
    assert_eq!(raised["isError"], true, "{raised}");
    let unkeyed = write(&mut mcp, &text.replace(", gain = 0.2 }", " }"));
    assert_eq!(
        unkeyed["isError"], true,
        "no key: the default 1.0: {unkeyed}"
    );
    let renamed = text
        .replace("out = {", "speaker = {")
        .replace("\"out.l\"", "\"speaker.l\"")
        .replace("\"out.r\"", "\"speaker.r\"")
        .replace(", gain = 0.2 }", " }");
    let renamed = write(&mut mcp, &renamed);
    assert_eq!(renamed["isError"], true, "a new sink at 1.0: {renamed}");
    assert!(
        renamed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("/speaker.gain"),
        "{renamed}"
    );
    let lowered = write(&mut mcp, &text.replace("gain = 0.2", "gain = 0.1"));
    assert_eq!(lowered["isError"], false, "{lowered}");
    assert!(mcp.finish().success());
}

/// A pattern a gain's modulation reads is part of what modulates it (R2).
#[test]
fn a_pattern_feeding_a_gain_is_refused() {
    let text = format!(
        "{HEAD}[instances]\nclock = {{ module = \"std.transport\", bpm = 120 }}\nseq = {{ module = \"std.note_seq\", pattern = \"melody\" }}\nosc = {{ module = \"std.sine_osc\" }}\nout = {{ module = \"std.audio_out\", gain = 0.1 }}\n\n[wiring]\nconnections = [\n  {{ from = \"clock.tick\", to = \"seq.tick\" }},\n  {{ from = \"seq.hz\", to = \"osc.freq\" }},\n  {{ from = \"osc.out\", to = \"out.l\" }},\n  {{ from = \"seq.velocity\", to = \"out.gain\" }},\n]\n"
    );
    let melody = "schema_version = 1\nkind = \"notes\"\nlength = 4\nnotes = [\n  { key = 60, start = 0, length = 1, velocity = 0.5 },\n]\n";
    let dir = project(&text, "gain-pattern");
    std::fs::create_dir_all(dir.join("patterns")).unwrap();
    std::fs::write(dir.join("patterns/melody.toml"), melody).unwrap();
    let mut mcp = Mcp::started(&dir, &[]);
    let read = mcp.ok("read_file", json!({"file": "patterns/melody.toml"}));
    let louder = melody.replace("velocity = 0.5", "velocity = 1");
    let message = mcp.refused(
        "write_pattern",
        json!({"name": "melody", "text": louder, "base": read["base"]}),
    );
    assert!(message.contains("--max-gain"), "{message}");
    assert_eq!(mcp.file("patterns/melody.toml"), melody);
    assert!(mcp.finish().success());
}

/// Vendoring changes no gain: allowed (its guard runs on what it writes).
#[test]
fn vendor_is_checked_and_allowed_when_no_gain_changes() {
    let mut mcp = Mcp::started(&fixture("sine", "gain-vendor"), &[]);
    mcp.ok("vendor", json!({"composite": "lib.tape_echo"}));
    assert!(mcp.finish().success());
}

/// review H1 (fail closed): with project files that do not load (a person
/// or another tool mid-edit; the watch keeps the last good version
/// playing), no change can be checked, so every changing tool is refused
/// and nothing is applied or written.
#[test]
fn files_that_do_not_load_refuse_every_change() {
    let mut mcp = Mcp::started(&project(&tone("0.2"), "gain-bogus"), &[]);
    let good = base(&mut mcp);
    let broken = format!("{}\n[bogus\n", mcp.file("project.toml"));
    std::fs::write(mcp.dir.join("project.toml"), &broken).unwrap();
    for (tool, arguments) in [
        (
            "set_parameter",
            json!({"address": "/out.gain", "value": 4.0}),
        ),
        ("ramp_parameter", json!({"address": "/out.gain", "to": 3.0})),
        ("commit", json!({"address": "/out.gain", "value": 4.0})),
        ("revert", json!({"address": "/out.gain"})),
        ("trigger", json!({"address": "/osc.freq"})),
        ("transport", json!({"action": "start"})),
        ("transport", json!({"action": "tempo", "bpm": 90})),
        (
            "patch",
            json!({"ops": [{"op": "connection.scale", "from": "osc.out", "to": "out.l", "scale": 2}]}),
        ),
        (
            "write_file",
            json!({"file": "project.toml", "text": tone("4"), "base": good}),
        ),
        (
            "write_pattern",
            json!({"name": "p", "text": "kind = \"steps\"\nsteps = [1]\n", "base": null}),
        ),
        ("vendor", json!({"composite": "lib.tape_echo"})),
    ] {
        let message = mcp.refused(tool, arguments);
        assert!(
            message.contains("do not load") || message.contains("std.transport"),
            "{tool}: {message}"
        );
    }
    assert_eq!(mcp.file("project.toml"), broken, "nothing was written");
    // The running gain is still the last good version's.
    std::fs::write(mcp.dir.join("project.toml"), tone("0.2")).unwrap();
    thread::sleep(Duration::from_millis(400));
    assert_eq!(live_gain(&mut mcp), 0.2);
    assert!(mcp.finish().success());
}

/// review M1: playing the project again from its files (`transport`
/// start, `session_start` with `discard`) sends every unsaved value back
/// to the file's, so it raises a lowered gain as a revert would: refused
/// like the revert, and allowed once the lowered value is committed.
#[test]
fn a_restart_does_not_raise_a_lowered_gain() {
    let mut mcp = Mcp::started(&project(&tone("0.5"), "gain-restart"), &[]);
    mcp.ok(
        "set_parameter",
        json!({"address": "/out.gain", "value": 0.1}),
    );
    refused(&mut mcp, "revert", json!({"address": "/out.gain"}));
    refused(&mut mcp, "transport", json!({"action": "start"}));
    refused(&mut mcp, "session_start", json!({"discard": true}));
    assert_eq!(live_gain(&mut mcp), 0.1);
    mcp.ok("commit", json!({"address": "/out.gain"}));
    assert!(mcp.file("project.toml").contains("gain = 0.1"));
    mcp.ok("transport", json!({"action": "start"}));
    thread::sleep(Duration::from_millis(200));
    assert_eq!(live_gain(&mut mcp), 0.1);
    assert!(mcp.finish().success());
}

/// review H2 (operator's decision): `--max-gain` is the output's ceiling.
/// A connection into the sink scaled ×20 raises no gain, so the guard
/// allows it, but what goes to the device (the null driver's buffer, the
/// meters) is held within ±0.25 and counted; the offline render is the
/// project as written, unheld.
#[test]
fn the_device_output_is_held_at_the_ceiling() {
    let mut mcp = Mcp::started(&project(&tone("0.2"), "gain-output"), &[]);
    let before = mcp.ok("meter", json!({"window_ms": 200}));
    let peak = before["channels"][0]["peak"].as_f64().unwrap();
    assert!((peak - 0.2).abs() < 1e-3, "{before}");
    let ops: Vec<Value> = ["out.l", "out.r"]
        .iter()
        .map(|to| json!({"op": "connection.scale", "from": "osc.out", "to": to, "scale": 20}))
        .collect();
    mcp.ok("patch", json!({"ops": ops}));
    let until = Instant::now() + Duration::from_secs(10);
    let held = loop {
        let stats = mcp.ok("stats", json!({}));
        if stats["ceilinged"].as_u64().unwrap_or(0) > 0 || Instant::now() > until {
            break stats;
        }
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(held["ceiling"], 0.25, "{held}");
    assert!(held["ceilinged"].as_u64().unwrap() > 0, "{held}");
    let after = mcp.ok("meter", json!({"window_ms": 300}));
    for channel in after["channels"].as_array().unwrap() {
        assert_eq!(channel["peak"].as_f64().unwrap(), 0.25, "{after}");
    }
    let info = mcp.ok("project_info", json!({}));
    assert_eq!(info["outputCeiling"], 0.25);
    let rendered = mcp.ok("render", json!({"seconds": 0.1}));
    assert_eq!(rendered["played"], false);
    assert!(
        rendered["metrics"]["channels"][0]["peak"].as_f64().unwrap() > 1.0,
        "the render is the project as written: {rendered}"
    );
    assert!(mcp.finish().success());
}
