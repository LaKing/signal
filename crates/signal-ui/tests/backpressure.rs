//! M2.8-1 AC3: slow or disconnected clients never block the control thread.
//! A client whose writer is held (the test hook: as if its socket took
//! nothing) fills its bounded queue: its data frames are dropped and counted,
//! and a control reply that does not fit evicts it; a client that drops its
//! socket is removed; another client's edits are answered throughout, and no
//! control step waits.

mod common;

use std::time::{Duration, Instant};

use common::{Options, Transport, rig, send};
use serde_json::{Value, json};

fn a_stalled_client_is_dropped_from_and_evicted_without_blocking_the_others(transport: Transport) {
    let mut rig = rig(
        "backpressure",
        Options {
            client_queue: 8,
            transport,
            ..Options::default()
        },
    );
    let (mut slow, slow_id) = rig.client();
    let (mut other, _) = rig.client();
    let (gone, gone_id) = rig.client();

    assert!(rig.host().hold_writer(slow_id, true));
    send(
        &mut slow,
        json!({"type": "subscribe", "stream": "meters", "rate": 60, "id": 1}),
    );

    let mut slowest = Duration::ZERO;
    let mut step = |rig: &mut common::Rig| {
        let host = rig.host();
        let _ = host.run_null(256, |_| {});
        let began = Instant::now();
        host.step();
        slowest = slowest.max(began.elapsed());
    };
    let client = |stats: &Value, id: u64| {
        stats["clients"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["client"] == id)
            .cloned()
    };

    // The held queue fills; meter frames beyond it are dropped and counted.
    let until = Instant::now() + Duration::from_secs(20);
    let mut id = 100;
    let dropped = loop {
        assert!(Instant::now() < until, "no frame was dropped");
        for _ in 0..20 {
            step(&mut rig);
        }
        id += 1;
        let stats = rig.request(&mut other, id, json!({"type": "stats"}), "stats");
        let dropped = client(&stats, slow_id).map_or(0, |c| c["dropped"].as_u64().unwrap());
        if dropped > 0 {
            break dropped;
        }
    };
    assert!(dropped > 0);

    // The other client's edits are acknowledged meanwhile.
    for n in 0..20 {
        let value = 0.1 + 0.01 * f64::from(n);
        id += 1;
        let ack = rig.request(
            &mut other,
            id,
            json!({"type": "set", "address": "/out.gain", "value": value}),
            "ack",
        );
        assert_eq!(ack["outcome"], "scheduled");
    }

    // A reply that does not fit evicts the stalled client. The other
    // client's uncommitted edits made `/out.gain` unsaved, and that push (a
    // control message too) may already have evicted it: then this send
    // finds the connection closed.
    let _ = slow.send(tungstenite::Message::text(
        json!({"type": "ping", "id": 2}).to_string(),
    ));
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < until, "the stalled client was not evicted");
        for _ in 0..5 {
            step(&mut rig);
        }
        id += 1;
        let stats = rig.request(&mut other, id, json!({"type": "stats"}), "stats");
        if stats["evicted"] == 1 && client(&stats, slow_id).is_none() {
            break;
        }
    }

    // A client that drops its socket is removed.
    drop(gone);
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            Instant::now() < until,
            "the disconnected client was not removed"
        );
        for _ in 0..5 {
            step(&mut rig);
        }
        id += 1;
        let stats = rig.request(&mut other, id, json!({"type": "stats"}), "stats");
        if client(&stats, gone_id).is_none() {
            break;
        }
    }

    assert!(
        slowest < Duration::from_millis(50),
        "a control step took {slowest:?}"
    );
    drop(slow);
    rig.finish();
}

both!(a_stalled_client_is_dropped_from_and_evicted_without_blocking_the_others);
