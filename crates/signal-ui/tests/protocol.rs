//! M2.8-1 AC1: the protocol with a `tungstenite` client against the null
//! driver: describe; meters and a scope stream; set and ramp, then the
//! effective value; trigger; commit, then the file; a plan swap updates
//! describe; a bad token, a missing hello and another version are refused.

mod common;

use std::fs;

use common::{Got, Options, Rig, Transport, connect, receive, rig, send};
use serde_json::{Value, json};
use signal_ui::frames::{METERS, PARAMETERS, SCOPE};
use tungstenite::Message;

fn find<'a>(list: &'a Value, address: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|v| v["address"] == address)
        .unwrap_or_else(|| panic!("{address} in {list}"))
}

fn describe_gives_the_plan_its_controls_sinks_and_meters(transport: Transport) {
    let mut rig = rig(
        "describe",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let d = rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    assert_eq!(d["planVersion"], 0);
    assert_eq!(d["sampleRate"], 48000);
    let osc = find(&d["instances"], "/a");
    assert_eq!(osc["module"], "std.sine_osc");
    let out = find(&osc["ports"], "/a.out");
    assert_eq!(
        (out["direction"].as_str(), out["shape"].as_str()),
        (Some("out"), Some("signal"))
    );
    assert!(out.get("kind").is_some() && out.get("unit").is_some() && out.get("range").is_some());
    let freq = find(&osc["parameters"], "/a.freq");
    assert_eq!(freq["base"], 440.0);
    for key in ["kind", "unit", "range", "default", "combine"] {
        assert!(freq.get(key).is_some(), "{key} in {freq}");
    }
    let gain = find(&d["controls"], "/out.gain");
    assert_eq!(gain["base"], 0.5);
    assert_eq!(gain["inFile"], true);
    assert_eq!(gain["ports"], json!(["/out.gain"]));
    assert!(gain["combine"].is_string());
    assert_eq!(
        d["sinks"],
        json!([{"index": 0, "address": "/out", "kind": "audio", "inputs": 2}])
    );
    assert_eq!(d["meters"]["output"].as_array().unwrap().len(), 2);
    assert_eq!(d["scopes"]["slots"], 8);
    rig.finish();
}

fn meters_and_a_scope_stream_arrive(transport: Transport) {
    let mut rig = rig(
        "streams",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let meters = rig.request(
        &mut ws,
        1,
        json!({"type": "subscribe", "stream": "meters", "rate": 30}),
        "subscribed",
    );
    let sub = meters["sub"].as_u64().unwrap();
    for _ in 0..3 {
        let frame = rig.wait(&mut ws, "a meter frame", |got| got.frame(METERS, sub));
        assert_eq!(frame.version, 1);
        assert_eq!(frame.count, 2);
        let levels = frame.f32s();
        assert!(levels[0] > 0.0 && levels[0] <= 1.0, "peak {levels:?}");
        assert!(levels[1] > 0.0, "rms {levels:?}");
    }
    let scope = rig.request(
        &mut ws,
        2,
        json!({"type": "subscribe", "stream": "scope", "port": "/a.out", "rate": 4800}),
        "subscribed",
    );
    assert_eq!(scope["decimation"], 10);
    let sub = scope["sub"].as_u64().unwrap();
    let first = rig.wait(&mut ws, "a scope frame", |got| got.frame(SCOPE, sub));
    let second = rig.wait(&mut ws, "a second scope frame", |got| got.frame(SCOPE, sub));
    assert_eq!(first.extra, 10);
    assert_eq!(first.count, 256);
    assert_eq!(first.first % 10, 0, "points at multiples of the decimation");
    assert_eq!(second.first, first.first + 256 * 10, "consecutive blocks");
    let points = first.f32s();
    assert!(points.iter().all(|p| p.abs() <= 1.0));
    assert!(
        points.iter().any(|p| p.abs() > 0.5),
        "a sine at full scale: {points:?}"
    );
    // Only signal outputs can be scoped.
    let refused = rig.request(
        &mut ws,
        3,
        json!({"type": "subscribe", "stream": "scope", "port": "/out.gain", "rate": 4800}),
        "error",
    );
    assert_eq!(refused["code"], "not-a-signal-output");
    let gone = rig.request(
        &mut ws,
        4,
        json!({"type": "unsubscribe", "sub": sub}),
        "unsubscribed",
    );
    assert_eq!(gone["sub"], sub);
    rig.finish();
}

/// The `(base, effective)` of the first address of a parameter frame.
fn value(frame: &common::Frame) -> (f64, f64) {
    let values = frame.f64s();
    (values[0], values[1])
}

fn set_ramp_and_trigger_are_acknowledged_with_their_frame(transport: Transport) {
    let mut rig = rig(
        "edits",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let params = rig.request(
        &mut ws,
        1,
        json!({"type": "subscribe", "stream": "parameters", "addresses": ["/out.gain"], "rate": 60}),
        "subscribed",
    );
    let sub = params["sub"].as_u64().unwrap();
    let ack = rig.request(
        &mut ws,
        2,
        json!({"type": "set", "address": "/out.gain", "value": 0.25}),
        "ack",
    );
    assert_eq!(ack["outcome"], "scheduled");
    assert!(ack["frame"].as_u64().unwrap() >= ack["requested"].as_u64().unwrap());
    let set_at = ack["frame"].as_u64().unwrap();
    let frame = rig.wait(&mut ws, "gain 0.25", |got| {
        got.frame(PARAMETERS, sub)
            .filter(|f| f.first > set_at && value(f) == (0.25, 0.25))
    });
    assert_eq!(frame.count, 1);

    let ack = rig.request(
        &mut ws,
        3,
        json!({"type": "ramp", "address": "/out.gain", "to": 0.75, "frames": 4800}),
        "ack",
    );
    assert_eq!(ack["outcome"], "scheduled");
    let ramp_at = ack["frame"].as_u64().unwrap();
    rig.wait(&mut ws, "the ramp's end", |got| {
        got.frame(PARAMETERS, sub)
            .filter(|f| f.first > ramp_at + 4800 && value(f).1 == 0.75)
    });

    // Out of range: clamped, as the panel does.
    rig.request(
        &mut ws,
        4,
        json!({"type": "set", "address": "/out.gain", "value": 1e9}),
        "ack",
    );

    let ack = rig.request(
        &mut ws,
        5,
        json!({"type": "trigger", "address": "/env.trigger"}),
        "ack",
    );
    assert_eq!(ack["outcome"], "scheduled");
    let refused = rig.request(
        &mut ws,
        6,
        json!({"type": "trigger", "address": "/out.gain"}),
        "error",
    );
    assert_eq!(refused["code"], "unknown-address");
    let refused = rig.request(
        &mut ws,
        7,
        json!({"type": "set", "address": "/nowhere.x", "value": 1}),
        "error",
    );
    assert_eq!(refused["code"], "unknown-address");
    rig.finish();
}

fn commit_writes_the_file_once_and_the_reload_applies_nothing(transport: Transport) {
    let mut rig = rig(
        "commit",
        Options {
            transport,
            ..Options::default()
        },
    );
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "set", "address": "/out.gain", "value": 0.4}),
        "ack",
    );
    let committed = rig.request(
        &mut ws,
        2,
        json!({"type": "commit", "address": "/out.gain", "value": 0.4}),
        "committed",
    );
    assert_eq!(committed["sent"], true);
    assert_eq!(committed["writes"], 1);
    let expected =
        signal_project::write_parameter(&before, "out", "gain", Some(0.4), &["gain"]).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        expected,
        "only the value changed"
    );
    let reload = rig.wait(&mut ws, "the reload of the written file", |got| {
        got.of("reload").cloned()
    });
    assert_eq!(reload["sets"], 0, "{reload}");
    assert_eq!(reload["swap"], Value::Null);
    assert_eq!(reload["message"], "no change to apply");
    let d = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    assert_eq!(find(&d["controls"], "/out.gain")["base"], 0.4);
    // A control inside a composite is not editable; neither is a plan port
    // that is no top-level parameter.
    let refused = rig.request(
        &mut ws,
        4,
        json!({"type": "commit", "address": "/out.l", "value": 1}),
        "error",
    );
    assert_eq!(refused["code"], "not-editable");
    rig.finish();
}

fn a_plan_swap_updates_describe(transport: Transport) {
    let mut rig = rig(
        "swap",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let d = rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    assert_eq!(d["planVersion"], 0);
    let path = rig.dir.join("project.toml");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace(
            "out = { module",
            "c = { module = \"std.sine_osc\", freq = 330 }\nout = { module",
        )
        .replace(
            "  { from = \"a.out\", to = \"out.r\" },",
            "  { from = \"a.out\", to = \"out.r\" },\n  { from = \"c.out\", to = \"out.r\" },",
        );
    fs::write(&path, text).unwrap();
    let swapped = rig.wait(&mut ws, "describe after the swap", |got| {
        got.of("describe")
            .filter(|d| d["planVersion"].as_u64() > Some(0))
            .cloned()
    });
    find(&swapped["instances"], "/c");
    rig.finish();
}

fn a_bad_token_a_missing_hello_and_another_version_are_refused(transport: Transport) {
    let mut rig = rig(
        "refused",
        Options {
            transport,
            ..Options::default()
        },
    );
    let token = rig.host().token().to_owned();
    for (hello, code) in [
        (
            json!({"type": "hello", "protocol": 1, "token": "0".repeat(64)}),
            "unauthorized",
        ),
        (json!({"type": "hello", "protocol": 1}), "unauthorized"),
        (json!({"type": "describe"}), "unauthorized"),
        (
            json!({"type": "hello", "protocol": 2, "token": token}),
            "protocol",
        ),
    ] {
        let mut ws = connect(rig.host());
        send(&mut ws, hello.clone());
        let error = rig.wait(&mut ws, "the refusal", |got| got.of("error").cloned());
        assert_eq!(error["code"], code, "{hello}");
        // Then the connection is closed: nothing else is answered.
        let _ = ws.send(Message::text(
            json!({"type": "describe", "id": 1}).to_string(),
        ));
        let mut closed = false;
        for _ in 0..500 {
            rig.pump();
            match receive(&mut ws) {
                Some(Got::Closed) => {
                    closed = true;
                    break;
                }
                Some(got) => assert!(got.of("describe").is_none(), "answered after a refusal"),
                None => {}
            }
        }
        assert!(closed, "{hello}: the connection is closed");
    }
    // The stats count no client.
    let (mut ws, _) = rig.client();
    let stats = rig.request(&mut ws, 1, json!({"type": "stats"}), "stats");
    assert_eq!(stats["clients"].as_array().unwrap().len(), 1);
    rig.finish();
}

fn unknown_messages_are_answered_with_an_error(transport: Transport) {
    let mut rig: Rig = rig(
        "unknown",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let error = rig.request(&mut ws, 1, json!({"type": "dance"}), "error");
    assert_eq!(error["code"], "unknown-type");
    let pong = rig.request(&mut ws, 2, json!({"type": "ping"}), "pong");
    assert!(pong["frame"].is_u64());
    rig.finish();
}

both!(
    describe_gives_the_plan_its_controls_sinks_and_meters,
    meters_and_a_scope_stream_arrive,
    set_ramp_and_trigger_are_acknowledged_with_their_frame,
    commit_writes_the_file_once_and_the_reload_applies_nothing,
    a_plan_swap_updates_describe,
    a_bad_token_a_missing_hello_and_another_version_are_refused,
    unknown_messages_are_answered_with_an_error
);
