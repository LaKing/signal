//! M2.8-1 AC1: the protocol with a `tungstenite` client against the null
//! driver: describe; meters and a scope stream; set and ramp, then the
//! effective value; trigger; commit, then the file; a plan swap updates
//! describe; a bad token, a missing hello and another version are refused;
//! (M2.8-7) vector streams of the physical-av demo.

mod common;

use std::fs;

use common::{Got, Options, Rig, Transport, connect, receive, rig, send};
use serde_json::{Value, json};
use signal_ui::frames::{METERS, PARAMETERS, SCOPE, VECTOR};
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

/// M2.8-7: a vector stream of the visual membrane arrives as frames of kind
/// 4, one per snapshot, its 256 rows at multiples of the decimation (a whole
/// number of the port's 800-frame ticks); describe gives the slots and the
/// shape; a signal output, a too-fast rate and a ninth slot are refused.
fn vector_streams_arrive(transport: Transport) {
    let mut rig = rig(
        "vectors",
        Options {
            transport,
            project: Some(common::root().join("projects/physical-av")),
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let d = rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    assert_eq!(d["vectors"], json!({"slots": 8, "maxRows": 512}));
    let z = find(
        &find(&d["instances"], "/membrane_v")["ports"],
        "/membrane_v.z",
    );
    assert_eq!(
        (z["shape"].clone(), z["rate"].clone()),
        (json!({"vector": 256}), json!(800))
    );
    let reply = rig.request(
        &mut ws,
        2,
        json!({"type": "subscribe", "stream": "vector", "port": "/membrane_v.z", "rate": 30}),
        "subscribed",
    );
    assert_eq!(
        (reply["rows"].clone(), reply["decimation"].clone()),
        (json!(256), json!(1600))
    );
    let sub = reply["sub"].as_u64().unwrap();
    let first = rig.wait(&mut ws, "a vector frame", |got| got.frame(VECTOR, sub));
    let second = rig.wait(&mut ws, "a second vector frame", |got| {
        got.frame(VECTOR, sub)
    });
    assert_eq!((first.count, first.extra), (256, 1600));
    assert_eq!(
        first.first % 1600,
        0,
        "snapshots on multiples of the decimation"
    );
    assert_eq!(second.first, first.first + 1600);
    assert!(first.f32s().iter().all(|x| x.is_finite()));
    // At most the tick rate: 60 per second, every tick.
    let fast = rig.request(
        &mut ws,
        3,
        json!({"type": "subscribe", "stream": "vector", "port": "/sparks.life", "rate": 1000}),
        "subscribed",
    );
    assert_eq!(fast["decimation"], 800);
    let refused = rig.request(
        &mut ws,
        4,
        json!({"type": "subscribe", "stream": "vector", "port": "/membrane_a.out", "rate": 60}),
        "error",
    );
    assert_eq!(refused["code"], "not-a-vector-output");
    for id in 5..11 {
        rig.request(
            &mut ws,
            id,
            json!({"type": "subscribe", "stream": "vector", "port": "/bodies.bodies"}),
            "subscribed",
        );
    }
    let full = rig.request(
        &mut ws,
        11,
        json!({"type": "subscribe", "stream": "vector", "port": "/bodies.bodies"}),
        "error",
    );
    assert_eq!(full["code"], "no-vector-slot");
    let gone = rig.request(
        &mut ws,
        12,
        json!({"type": "unsubscribe", "sub": sub}),
        "unsubscribed",
    );
    assert_eq!(gone["sub"], sub);
    rig.request(
        &mut ws,
        13,
        json!({"type": "subscribe", "stream": "vector", "port": "/bodies.bodies"}),
        "subscribed",
    );
    rig.finish();
}

/// Finding auditor-1-007 and auditor-2-vector-rearm-keeps-subscription-rows:
/// a swap that retypes the subscribed port's instance to a vector of
/// another width is told to the client (`vector.reshaped` with the new
/// `rows`), and the stream carries the new width from then on.
fn a_vector_stream_follows_a_swap_that_changes_its_width(transport: Transport) {
    let mut rig = rig(
        "vector-reshape",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let tone = fs::read_to_string(&path).unwrap();
    let with = |module: &str| {
        tone.replace(
            "out = { module",
            &format!("v = {{ module = \"{module}\" }}\nout = {{ module"),
        )
    };
    fs::write(&path, with("std.particles")).unwrap();
    rig.wait(&mut ws, "the particles", |got| {
        got.of("describe")
            .filter(|d| d["instances"].to_string().contains("/v.vel"))
            .cloned()
    });
    let reply = rig.request(
        &mut ws,
        1,
        json!({"type": "subscribe", "stream": "vector", "port": "/v.vel"}),
        "subscribed",
    );
    assert_eq!(reply["rows"], 128);
    let sub = reply["sub"].as_u64().unwrap();
    let first = rig.wait(&mut ws, "a vector frame", |got| got.frame(VECTOR, sub));
    assert_eq!(first.count, 128);
    fs::write(&path, with("std.body2d")).unwrap();
    let reshaped = rig.wait(&mut ws, "vector.reshaped", |got| {
        got.of("vector.reshaped").cloned()
    });
    assert_eq!(
        (reshaped["sub"].as_u64(), reshaped["rows"].clone()),
        (Some(sub), json!(12))
    );
    let after = rig.wait(&mut ws, "a frame of 12 rows", |got| {
        got.frame(VECTOR, sub).filter(|f| f.count == 12)
    });
    assert_eq!(after.f32s().len(), 12);
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

fn commit_writes_the_file_once_and_the_watch_does_not_reload_it(transport: Transport) {
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
    // The running project already holds the written file: the watch does
    // not reload it (several watch intervals pass).
    rig.no_reload(&mut ws, 500);
    // Another writer's identical edit is still seen, and applies nothing.
    fs::write(&path, format!("{expected}\n")).unwrap();
    let reload = rig.wait(&mut ws, "the reload of another write", |got| {
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

/// Another writer's edit to `project.toml` that the watch cannot see (same
/// length, its modification time put back), followed by a commit: the
/// commit's write keeps it, and since the file it read is not what the
/// running project came from, the watch reloads the written file and applies
/// the edit (audit 2026-10-16, P2: content, not metadata, decides).
fn an_edit_the_watch_cannot_see_is_applied_by_the_next_commit(transport: Transport) {
    let mut rig = rig(
        "unseen",
        Options {
            transport,
            ..Options::default()
        },
    );
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let (mut ws, _) = rig.client();
    rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let edited = before.replace("freq = 440", "freq = 330");
    assert_eq!(edited.len(), before.len());
    fs::write(&path, &edited).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    rig.no_reload(&mut ws, 300);
    let committed = rig.request(
        &mut ws,
        2,
        json!({"type": "commit", "address": "/out.gain", "value": 0.4}),
        "committed",
    );
    assert_eq!(committed["sent"], true);
    let reload = rig.wait(&mut ws, "the reload of the written file", |got| {
        got.of("reload").cloned()
    });
    assert_eq!(reload["sets"], 1, "{reload}");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("freq = 330"), "{text}");
    let d = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    let osc = find(&d["instances"], "/a");
    assert_eq!(find(&osc["parameters"], "/a.freq")["base"], 330.0);
    assert_eq!(find(&d["controls"], "/out.gain")["base"], 0.4);
    // The running project holds the file again: the next commit is quiet.
    rig.request(
        &mut ws,
        4,
        json!({"type": "commit", "address": "/out.gain", "value": 0.3}),
        "committed",
    );
    rig.no_reload(&mut ws, 300);
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
    assert!(pong["latency"].is_u64(), "{pong}");
    rig.finish();
}

/// M2.9-1b: `describe` gives the free scope slots, and `scopes` is pushed
/// whenever their count changes, with the time since it last rose.
fn the_free_scope_count_is_described_and_pushed(transport: Transport) {
    let mut rig = rig(
        "scopes-free",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let d = rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    assert_eq!(d["scopes"]["free"], 8);
    assert_eq!(
        d["scopes"]["sinceFreedMs"],
        Value::Null,
        "nothing freed yet"
    );
    let scope = rig.request(
        &mut ws,
        2,
        json!({"type": "subscribe", "stream": "scope", "port": "/a.out", "rate": 4800}),
        "subscribed",
    );
    let taken = rig.wait(&mut ws, "scopes after a subscribe", |got| {
        got.of("scopes").filter(|s| s["free"] == 7).cloned()
    });
    assert_eq!(taken["slots"], 8);
    rig.request(
        &mut ws,
        3,
        json!({"type": "unsubscribe", "sub": scope["sub"]}),
        "unsubscribed",
    );
    let freed = rig.wait(&mut ws, "scopes after an unsubscribe", |got| {
        got.of("scopes").filter(|s| s["free"] == 8).cloned()
    });
    assert!(freed["sinceFreedMs"].as_u64().unwrap() < 1000, "{freed}");
    rig.finish();
}

/// M2.9-1b (auditor-1-net-free-count-priority): a slot taken, a scope
/// refused and a slot released in one host step leave the free count as it
/// was, and `scopes` is still pushed with the release's age, so a schematic
/// waits CLAIM_MS from that release before taking it.
fn a_release_masked_by_a_claim_still_restarts_the_release_age(transport: Transport) {
    let mut rig = rig(
        "scopes-masked",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let scope = json!({"type": "subscribe", "stream": "scope", "port": "/a.out", "rate": 4800});
    let mut subs = Vec::new();
    for id in 1..=7 {
        let reply = rig.request(&mut ws, id, scope.clone(), "subscribed");
        subs.push(reply["sub"].clone());
    }
    rig.wait(&mut ws, "scopes with one slot free", |got| {
        got.of("scopes").filter(|s| s["free"] == 1).cloned()
    });
    // One batch: the last slot taken, a scope refused, a slot released.
    send(&mut ws, with_id(&scope, 8));
    send(&mut ws, with_id(&scope, 9));
    send(
        &mut ws,
        json!({"type": "unsubscribe", "id": 10, "sub": subs[0]}),
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    let mut refused = false;
    let pushed = rig.wait(&mut ws, "scopes after the batch", |got| {
        if got
            .of("error")
            .is_some_and(|e| e["code"] == "no-scope-slot")
        {
            refused = true;
        }
        got.of("scopes").filter(|s| s.get("id").is_none()).cloned()
    });
    assert!(refused, "the second subscribe was refused");
    assert_eq!(pushed["free"], 1, "as it was before the batch");
    assert!(
        pushed["sinceFreedMs"].as_u64().is_some_and(|ms| ms < 1000),
        "the release's age, not an older one: {pushed}"
    );
    rig.finish();
}

/// A sequencer's voice, for `note` (ADR 0039): `std.note_seq` without a
/// pattern, its gate and hz into a sine through a multiplier.
const AUDITION: &str = r#"schema_version = 1

[project]
name = "audition"
sample_rate = 48000
seed = 1

[instances]
melody = { module = "std.note_seq" }
tone = { module = "std.sine_osc", freq = 0 }
vca = { module = "std.mul" }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "melody.hz", to = "tone.freq" },
  { from = "tone.out", to = "vca.a" },
  { from = "melody.gate", to = "vca.b" },
  { from = "vca.out", to = "out.l" },
  { from = "vca.out", to = "out.r" },
]
"#;

/// ADR 0039: `note` plays a live note on a sequencer's `audition` input
/// while it is held, is acknowledged like `trigger`, refuses a bad key or
/// target, and a client that goes with a note held has it ended.
fn a_note_sounds_while_held_and_ends_when_its_client_goes(transport: Transport) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-audition-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("project.toml"), AUDITION).unwrap();
    let mut rig = rig(
        "audition",
        Options {
            transport,
            project: Some(source.clone()),
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let scope = rig.request(
        &mut ws,
        1,
        json!({"type": "subscribe", "stream": "scope", "port": "/melody.gate", "rate": 4800}),
        "subscribed",
    );
    let sub = scope["sub"].as_u64().unwrap();
    let gate = |rig: &mut Rig, ws: &mut common::Ws, after: u64, level: f32, what: &str| {
        rig.wait(ws, what, |got| {
            got.frame(SCOPE, sub)
                .filter(|f| f.first > after && f.f32s().iter().all(|&g| g == level))
        });
    };
    gate(&mut rig, &mut ws, 0, 0.0, "silent at first");
    let ack = rig.request(
        &mut ws,
        2,
        json!({"type": "note", "address": "/melody.audition", "key": 72, "velocity": 1.0, "on": true}),
        "ack",
    );
    assert_eq!(ack["outcome"], "scheduled");
    gate(
        &mut rig,
        &mut ws,
        ack["frame"].as_u64().unwrap(),
        1.0,
        "held",
    );
    let ack = rig.request(
        &mut ws,
        3,
        json!({"type": "note", "address": "/melody.audition", "key": 72, "on": false}),
        "ack",
    );
    gate(
        &mut rig,
        &mut ws,
        ack["frame"].as_u64().unwrap(),
        0.0,
        "released",
    );

    for (id, message, code) in [
        (
            4,
            json!({"type": "note", "address": "/melody.audition", "key": 128, "on": true}),
            "bad-value",
        ),
        (
            5,
            json!({"type": "note", "address": "/melody.audition", "key": 60}),
            "bad-value",
        ),
        (
            6,
            json!({"type": "note", "address": "/melody.audition", "key": 60, "velocity": "loud", "on": true}),
            "bad-value",
        ),
        (
            7,
            json!({"type": "note", "address": "/out.gain", "key": 60, "on": true}),
            "unknown-address",
        ),
        (
            8,
            json!({"type": "note", "address": "/melody.gate", "key": 60, "on": true}),
            "unknown-address",
        ),
        // An event input that is no `audition`: a tick, a reset.
        (
            10,
            json!({"type": "note", "address": "/melody.tick", "key": 60, "on": true}),
            "not-an-audition-input",
        ),
        (
            11,
            json!({"type": "note", "address": "/melody.reset", "key": 60, "on": true}),
            "not-an-audition-input",
        ),
    ] {
        let refused = rig.request(&mut ws, id, message.clone(), "error");
        assert_eq!(refused["code"], code, "{message}");
    }

    // Another client presses a key and goes without letting it go.
    let (mut other, _) = rig.client();
    let ack = rig.request(
        &mut other,
        1,
        json!({"type": "note", "address": "/melody.audition", "key": 60, "on": true}),
        "ack",
    );
    gate(
        &mut rig,
        &mut ws,
        ack["frame"].as_u64().unwrap(),
        1.0,
        "the other's note",
    );
    drop(other);
    let ping = rig.request(&mut ws, 9, json!({"type": "ping"}), "pong");
    gate(
        &mut rig,
        &mut ws,
        ping["frame"].as_u64().unwrap(),
        0.0,
        "ended when its client went",
    );

    // A slide (60, then 62, then 60's off) and a client gone: 62 ends too.
    let (mut slider, _) = rig.client();
    for (id, key, on) in [(1, 60, true), (2, 62, true), (3, 60, false)] {
        rig.request(
            &mut slider,
            id,
            json!({"type": "note", "address": "/melody.audition", "key": key, "on": on}),
            "ack",
        );
    }
    let ping = rig.request(&mut ws, 12, json!({"type": "ping"}), "pong");
    gate(
        &mut rig,
        &mut ws,
        ping["frame"].as_u64().unwrap(),
        1.0,
        "62 still held",
    );
    drop(slider);
    let ping = rig.request(&mut ws, 13, json!({"type": "ping"}), "pong");
    gate(
        &mut rig,
        &mut ws,
        ping["frame"].as_u64().unwrap(),
        0.0,
        "the slid-to key ended with its client",
    );

    // Two rolls of one page on one port: the first's release leaves the second's key.
    let (mut page, _) = rig.client();
    for (id, key, on) in [(1, 64, true), (2, 67, true), (3, 64, false)] {
        rig.request(
            &mut page,
            id,
            json!({"type": "note", "address": "/melody.audition", "key": key, "on": on}),
            "ack",
        );
    }
    let ping = rig.request(&mut ws, 14, json!({"type": "ping"}), "pong");
    gate(
        &mut rig,
        &mut ws,
        ping["frame"].as_u64().unwrap(),
        1.0,
        "67 still held",
    );
    drop(page);
    let ping = rig.request(&mut ws, 15, json!({"type": "ping"}), "pong");
    gate(
        &mut rig,
        &mut ws,
        ping["frame"].as_u64().unwrap(),
        0.0,
        "67 ended with its client",
    );
    rig.finish();
    let _ = fs::remove_dir_all(&source);
}

fn with_id(message: &Value, id: u64) -> Value {
    let mut message = message.clone();
    message["id"] = json!(id);
    message
}

both!(
    the_free_scope_count_is_described_and_pushed,
    a_release_masked_by_a_claim_still_restarts_the_release_age,
);

both!(
    describe_gives_the_plan_its_controls_sinks_and_meters,
    meters_and_a_scope_stream_arrive,
    set_ramp_and_trigger_are_acknowledged_with_their_frame,
    a_note_sounds_while_held_and_ends_when_its_client_goes,
    commit_writes_the_file_once_and_the_watch_does_not_reload_it,
    an_edit_the_watch_cannot_see_is_applied_by_the_next_commit,
    a_plan_swap_updates_describe,
    a_bad_token_a_missing_hello_and_another_version_are_refused,
    unknown_messages_are_answered_with_an_error,
    vector_streams_arrive,
    a_vector_stream_follows_a_swap_that_changes_its_width
);
