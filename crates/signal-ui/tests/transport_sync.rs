//! External review, finding 4: Space starting several transports. A
//! separate `commit` per transport is stamped when the host handles it, so
//! two handled a callback apart start their playheads 256 frames apart for
//! good. The batch form of `commit` (`values`) stamps every value one frame
//! and writes them in one write, so they start on the same frame however
//! the message is delivered. On the null driver, over both transports.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use common::{Options, Rig, Transport, Ws, receive, rig, send};
use serde_json::{Value, json};
use signal_engine::ControlSink;
use signal_ui::frames::SCOPE;

const PROJECT: &str = r#"schema_version = 1

[project]
name = "two-transports"
sample_rate = 48000
seed = 1

[instances]
a = { module = "std.playhead", run = 0 }
b = { module = "std.playhead", run = 0 }
osc = { module = "std.sine_osc" }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "osc.out", to = "out.l" },
]
"#;

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "signal-ui-transport-sync-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("project.toml"), PROJECT).unwrap();
    dir
}

fn two(name: &str, transport: Transport) -> Rig {
    rig(
        name,
        Options {
            transport,
            project: Some(fixture(&format!("{name}-{transport:?}"))),
            ..Options::default()
        },
    )
}

/// Subscribes `ws` to both playheads' `running` at the full rate; their subs.
fn watch(rig: &mut Rig, ws: &mut Ws) -> [u64; 2] {
    ["a", "b"].map(|name| {
        let id = if name == "a" { 900 } else { 901 };
        let reply = rig.request(
            ws,
            id,
            json!({"type": "subscribe", "stream": "scope", "port": format!("/{name}.running"), "rate": 48000}),
            "subscribed",
        );
        reply["sub"].as_u64().unwrap()
    })
}

/// Steps until both playheads run; the first frame each did.
fn started(rig: &mut Rig, ws: &mut Ws, subs: [u64; 2]) -> [u64; 2] {
    let mut first: BTreeMap<u64, u64> = BTreeMap::new();
    for _ in 0..400 {
        rig.pump();
        while let Some(got) = receive(ws) {
            for sub in subs {
                if let Some(frame) = got.frame(SCOPE, sub)
                    && !first.contains_key(&sub)
                    && let Some(k) = frame.f32s().iter().position(|&v| v >= 0.5)
                {
                    first.insert(sub, frame.first + k as u64);
                }
            }
        }
        if first.len() == 2 {
            return subs.map(|s| first[&s]);
        }
    }
    panic!("the playheads did not both start: {first:?}");
}

fn run_both(at: Option<u64>) -> Value {
    let mut message = json!({"type": "commit", "values": [
        {"address": "/a.run", "value": 1},
        {"address": "/b.run", "value": 1},
    ]});
    if let Some(at) = at {
        message["at"] = json!(at);
    }
    message
}

/// The flaw: one `commit` per transport, the second handled after the first
/// has started, starts them apart, by whole callbacks of 256 frames.
fn separate_commits_a_callback_apart_drift(transport: Transport) {
    let mut rig = two("separate", transport);
    let (mut ws, _) = rig.client();
    let subs = watch(&mut rig, &mut ws);
    rig.pump();
    send(
        &mut ws,
        json!({"type": "commit", "id": 1, "address": "/a.run", "value": 1}),
    );
    let a = first_start(&mut rig, &mut ws, subs[0]);
    send(
        &mut ws,
        json!({"type": "commit", "id": 2, "address": "/b.run", "value": 1}),
    );
    let b = first_start(&mut rig, &mut ws, subs[1]);
    assert!(
        b > a && (b - a).is_multiple_of(256),
        "whole callbacks apart: {}",
        b - a
    );
    rig.finish();
}

/// The first frame at which `sub`'s playhead runs.
fn first_start(rig: &mut Rig, ws: &mut Ws, sub: u64) -> u64 {
    for _ in 0..400 {
        rig.pump();
        while let Some(got) = receive(ws) {
            if let Some(frame) = got.frame(SCOPE, sub)
                && let Some(k) = frame.f32s().iter().position(|&v| v >= 0.5)
            {
                return frame.first + k as u64;
            }
        }
    }
    panic!("the playhead {sub} did not start");
}

/// The batch: whatever the frames run before the host handles it (none,
/// part of a callback, one or more callbacks), both start on one frame,
/// the reply names it, and the file holds both values from one write.
fn a_batch_commit_starts_both_on_one_frame(transport: Transport) {
    for (k, before) in [0u64, 64, 100, 256, 700].into_iter().enumerate() {
        let mut rig = two(&format!("batch-{k}"), transport);
        let (mut ws, _) = rig.client();
        let subs = watch(&mut rig, &mut ws);
        rig.pump();
        let writes = rig.request(&mut ws, 2, json!({"type": "stats"}), "stats")["writes"]
            .as_u64()
            .unwrap();
        let mut message = run_both(None);
        message["id"] = json!(1);
        send(&mut ws, message);
        if before > 0 {
            let _ = rig.host().run_null(before, |_| {});
        }
        let reply = rig.reply(&mut ws, 1, "committed");
        let frame = reply["frame"].as_u64().expect("the frame both are stamped");
        assert_eq!(reply["values"].as_array().unwrap().len(), 2, "{reply}");
        assert_eq!(reply["writes"].as_u64(), Some(writes + 1), "one write");
        let text = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
        assert!(
            text.contains("a = { module = \"std.playhead\", run = 1 }"),
            "{text}"
        );
        assert!(
            text.contains("b = { module = \"std.playhead\", run = 1 }"),
            "{text}"
        );
        let [a, b] = started(&mut rig, &mut ws, subs);
        assert_eq!(a, b, "{before} frames before it was handled: one frame");
        assert!(a >= frame, "started at {a}, stamped {frame}");
        rig.finish();
    }
    // An explicit frame is the one both take.
    let mut rig = two("batch-at", transport);
    let (mut ws, _) = rig.client();
    let subs = watch(&mut rig, &mut ws);
    rig.pump();
    let at = 48_000;
    let reply = rig.request(&mut ws, 1, run_both(Some(at)), "committed");
    assert_eq!(reply["frame"], at);
    let [a, b] = started(&mut rig, &mut ws, subs);
    assert_eq!(a, b);
    assert!(a >= at);
    rig.finish();
}

/// Refused, writing nothing: `address` beside `values`, an empty list, a
/// control named twice, one that is no control.
fn a_bad_batch_writes_nothing(transport: Transport) {
    let mut rig = two("batch-bad", transport);
    let (mut ws, _) = rig.client();
    let before = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    for (id, message, code) in [
        (
            1,
            json!({"type": "commit", "address": "/a.run", "values": [{"address": "/b.run", "value": 1}]}),
            "bad-message",
        ),
        (2, json!({"type": "commit", "values": []}), "bad-message"),
        (
            3,
            json!({"type": "commit", "values": [{"address": "/a.run", "value": 1}, {"address": "/a.run", "value": 0}]}),
            "bad-value",
        ),
        (
            4,
            json!({"type": "commit", "values": [{"address": "/a.run", "value": 1}, {"address": "/nope.run", "value": 1}]}),
            "not-editable",
        ),
    ] {
        let error = rig.request(&mut ws, id, message, "error");
        assert_eq!(error["code"], code, "{error}");
    }
    assert_eq!(
        fs::read_to_string(rig.dir.join("project.toml")).unwrap(),
        before
    );
    rig.finish();
}

/// Second review, item 4: every entry of a batch is sent stamped its frame,
/// also one whose value is the one last sent live (here `/a.run` set to 1
/// before, applied): no entry is left out with its earlier stamp. The
/// recorded session's commands show a ramp of both runs at the frame.
fn a_batch_sends_every_value_at_its_frame(transport: Transport) {
    let mut rig = rig(
        &format!("batch-resend-{transport:?}"),
        Options {
            transport,
            recording: true,
            project: Some(fixture(&format!("batch-resend-{transport:?}"))),
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "set", "address": "/a.run", "value": 1}),
        "ack",
    );
    let at = 48_000;
    let reply = rig.request(&mut ws, 2, run_both(Some(at)), "committed");
    assert_eq!(reply["frame"], at);
    // Past the frame, every report in.
    for _ in 0..(at / 256 + 16) {
        rig.pump();
    }
    let finished = rig.host.take().unwrap().finish().unwrap();
    let mut live = finished.live;
    live.status(&finished.status);
    live.finish(finished.status.next_frame).unwrap();
    let dir = rig.dir.parent().unwrap().join("session");
    let _ = fs::remove_dir_all(&dir);
    live.into_recorder().write(&dir).unwrap();
    let edits = fs::read_to_string(dir.join("edits.jsonl")).unwrap();
    for port in ["/a.run", "/b.run"] {
        let at_frame = edits
            .lines()
            .filter(|l| l.contains(&format!("\"target\":\"{port}\"")))
            .filter(|l| l.contains("\"kind\":\"ramp\""))
            .filter(|l| l.starts_with(&format!("{{\"frame\":{at},")))
            .count();
        assert_eq!(at_frame, 1, "{port} ramped at {at}:\n{edits}");
    }
}

both!(
    separate_commits_a_callback_apart_drift,
    a_batch_commit_starts_both_on_one_frame,
    a_bad_batch_writes_nothing,
    a_batch_sends_every_value_at_its_frame,
);
