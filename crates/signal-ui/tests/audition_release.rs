//! External review, B (P1, reproduced): a client that goes, or is
//! evicted, while it holds an audition note (ADR 0039) has the note ended
//! even when the engine's command queue is full at that moment: the
//! note-off waits on the control side and is sent again each step until
//! the Controller takes it, so the output returns to silence once capacity
//! returns. Null driver only; the tone plays at most at the sink's 0.5.

mod common;

use std::fs;
use std::time::{Duration, Instant};

use common::{Got, Options, Rig, Transport, Ws, receive, rig, send};
use serde_json::{Value, json};
use signal_ui::frames::SCOPE;

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

fn audition_rig(name: &str, transport: Transport, client_queue: usize) -> Rig {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-{name}-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("project.toml"), AUDITION).unwrap();
    rig(
        name,
        Options {
            transport,
            project: Some(source),
            client_queue,
            ..Options::default()
        },
    )
}

/// Steps the host without running any frame (the engine takes nothing
/// from its command queue) until `ws` gets the reply to `id`.
fn reply_unplayed(rig: &mut Rig, ws: &mut Ws, id: u64) -> Value {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < until, "no reply to {id}");
        rig.host().step();
        while let Some(got) = receive(ws) {
            assert!(!matches!(got, Got::Closed), "closed waiting for {id}");
            if let Some(v) = got.json().filter(|v| v.get("id") == Some(&json!(id))) {
                return v.clone();
            }
        }
    }
}

/// Reads what `ws` has received so far, ignoring it.
fn drain(ws: &mut Ws) {
    while let Some(got) = receive(ws) {
        assert!(!matches!(got, Got::Closed), "a client was closed");
    }
}

/// Fills the engine's command queue with live edits from `ws`, no frame
/// running, until the Controller refuses commands; `watcher` is read
/// meanwhile.
fn fill_the_command_queue(rig: &mut Rig, ws: &mut Ws, watcher: &mut Ws) {
    let mut id = 1000;
    for _ in 0..64 {
        // In small batches, each read by a step, so no socket fills while
        // this thread (the control thread too) sends.
        for _ in 0..16 {
            for _ in 0..16 {
                id += 1;
                send(
                    ws,
                    json!({"type": "set", "address": "/out.gain", "value": 0.25, "id": id}),
                );
            }
            rig.host().step();
            drain(ws);
            drain(watcher);
        }
        id += 1;
        send(ws, json!({"type": "stats", "id": id}));
        let stats = reply_unplayed(rig, ws, id);
        if stats["commandOverflows"].as_u64().unwrap_or(0) > 0 {
            return;
        }
    }
    panic!("the command queue never filled");
}

/// Waits (playing) until a scope block of the gate is all `level`.
fn gate(rig: &mut Rig, ws: &mut Ws, sub: u64, level: f32, what: &str) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        rig.pump();
        while let Some(got) = receive(ws) {
            if got
                .frame(SCOPE, sub)
                .is_some_and(|f| f.f32s().iter().all(|&g| g == level))
            {
                return;
            }
        }
    }
}

/// A watcher of the gate, a holder with key 72 down, and the command
/// queue full.
fn held_with_a_full_queue(rig: &mut Rig) -> (Ws, u64, Ws, u64) {
    let (mut watcher, _) = rig.client();
    let scope = rig.request(
        &mut watcher,
        1,
        json!({"type": "subscribe", "stream": "scope", "port": "/melody.gate", "rate": 4800}),
        "subscribed",
    );
    let sub = scope["sub"].as_u64().unwrap();
    let (mut holder, holder_id) = rig.client();
    let ack = rig.request(
        &mut holder,
        1,
        json!({"type": "note", "address": "/melody.audition", "key": 72, "velocity": 0.5, "on": true}),
        "ack",
    );
    assert_eq!(ack["outcome"], "scheduled");
    gate(rig, &mut watcher, sub, 1.0, "the note held");
    let (mut flooder, _) = rig.client();
    fill_the_command_queue(rig, &mut flooder, &mut watcher);
    (watcher, sub, holder, holder_id)
}

fn a_client_gone_with_a_note_held_ends_it_once_the_queue_has_room(transport: Transport) {
    let mut rig = audition_rig("release-gone", transport, 256);
    let (mut watcher, sub, holder, _) = held_with_a_full_queue(&mut rig);
    drop(holder);
    // Gone while the queue is full: the note-off waits.
    let until = Instant::now() + Duration::from_secs(10);
    while rig.host().releases_waiting() == 0 {
        assert!(Instant::now() < until, "the client was not removed");
        rig.host().step();
    }
    assert_eq!(rig.host().releases_waiting(), 1);
    // Frames run, the queue drains, the note-off is taken: silence.
    gate(&mut rig, &mut watcher, sub, 0.0, "the note ended");
    assert_eq!(rig.host().releases_waiting(), 0);
    rig.finish();
}

fn a_client_evicted_with_a_note_held_ends_it_once_the_queue_has_room(transport: Transport) {
    let mut rig = audition_rig("release-evicted", transport, 64);
    let (mut watcher, sub, mut holder, holder_id) = held_with_a_full_queue(&mut rig);
    // Its socket takes nothing: its replies overflow its queue.
    assert!(rig.host().hold_writer(holder_id, true));
    for id in 10..150 {
        send(&mut holder, json!({"type": "ping", "id": id}));
    }
    let until = Instant::now() + Duration::from_secs(10);
    while rig.host().releases_waiting() == 0 {
        assert!(Instant::now() < until, "the client was not evicted");
        rig.host().step();
    }
    gate(&mut rig, &mut watcher, sub, 0.0, "the note ended");
    assert_eq!(rig.host().releases_waiting(), 0);
    drop(holder);
    rig.finish();
}

both!(
    a_client_gone_with_a_note_held_ends_it_once_the_queue_has_room,
    a_client_evicted_with_a_note_held_ends_it_once_the_queue_has_room,
);
