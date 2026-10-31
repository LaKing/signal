//! External review (Codex), item 1 (ADR 0048):
//! another writer holding the project's lock never stalls the control
//! thread. A file request (`commit`) that finds the lock held waits in the
//! host's queue; meanwhile every step stays short, and a client pings,
//! changes a live parameter and reads the engine's status. Once the lock is
//! released the waiting requests run in the order they came, holding it;
//! one that waits longer than the host's wait fails (`write-failed`), and
//! its value stays live and unsaved.

mod common;

use std::fs;
use std::time::{Duration, Instant};

use common::{Got, Options, Rig, Transport, Ws, receive, rig, send};
use serde_json::{Value, json};
use signal_project::transaction::ProjectLock;

/// What the steps took at most so far.
struct Timed {
    slowest: Duration,
}

impl Timed {
    /// Steps (timing each) until `ws` receives what `wanted` accepts.
    fn wait<T>(
        &mut self,
        rig: &mut Rig,
        ws: &mut Ws,
        what: &str,
        mut wanted: impl FnMut(&Got) -> Option<T>,
    ) -> T {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            self.pump(rig);
            while let Some(got) = receive(ws) {
                if let Some(found) = wanted(&got) {
                    return found;
                }
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn pump(&mut self, rig: &mut Rig) {
        let started = Instant::now();
        rig.pump();
        self.slowest = self.slowest.max(started.elapsed());
    }

    /// The reply to `id`, whatever its type.
    fn reply(&mut self, rig: &mut Rig, ws: &mut Ws, id: u64) -> Value {
        self.wait(rig, ws, &format!("the reply to {id}"), |got| {
            got.json()
                .filter(|v| v.get("id") == Some(&json!(id)))
                .cloned()
        })
    }

    /// Steps for `ms`, asserting that no reply to `ids` comes meanwhile.
    fn quiet(&mut self, rig: &mut Rig, ws: &mut Ws, ms: u64, ids: &[u64]) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            self.pump(rig);
            while let Some(got) = receive(ws) {
                if let Some(v) = got.json() {
                    assert!(
                        !ids.iter().any(|id| v.get("id") == Some(&json!(id))),
                        "answered while the lock is held: {v}"
                    );
                }
            }
        }
    }
}

fn another_writer_s_lock_never_stalls_the_control_thread(transport: Transport) {
    let mut rig = rig(
        "writer-lock",
        Options {
            transport,
            ..Options::default()
        },
    );
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let (mut ws, _) = rig.client();
    let sub = rig.watch_parameter(&mut ws, 1, "/a.freq");
    let mut timed = Timed {
        slowest: Duration::ZERO,
    };

    // Another writer holds the lock (another open file: another writer,
    // in this process too, ADR 0040).
    let lock = ProjectLock::acquire(&rig.dir).unwrap();
    let commit = |value: f64| json!({"type": "commit", "address": "/out.gain", "value": value});
    let mut c = commit(0.3);
    c["id"] = json!(2);
    send(&mut ws, c);
    let mut c = commit(0.35);
    c["id"] = json!(3);
    send(&mut ws, c);
    timed.quiet(&mut rig, &mut ws, 100, &[2, 3]);
    // Ping, a live parameter, the engine's status: answered meanwhile.
    send(&mut ws, json!({"type": "ping", "id": 4}));
    let pong = timed.reply(&mut rig, &mut ws, 4);
    assert_eq!(pong["type"], "pong", "{pong}");
    send(
        &mut ws,
        json!({"type": "set", "address": "/a.freq", "value": 330, "id": 5}),
    );
    let ack = timed.reply(&mut rig, &mut ws, 5);
    assert_eq!(ack["type"], "ack", "{ack}");
    timed.wait(&mut rig, &mut ws, "the live value", |got| {
        got.frame(3, sub)
            .map(|f| f.f64s()[0])
            .filter(|b| *b == 330.0)
    });
    send(&mut ws, json!({"type": "stats", "id": 6}));
    let stats = timed.reply(&mut rig, &mut ws, 6);
    assert_eq!(stats["type"], "stats", "{stats}");
    timed.quiet(&mut rig, &mut ws, 300, &[2, 3]);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        before,
        "nothing written"
    );
    assert!(
        timed.slowest < Duration::from_millis(100),
        "a step took {:?} while another writer held the lock",
        timed.slowest
    );

    // Released: the commits run, in order, holding the lock.
    drop(lock);
    let first = timed.reply(&mut rig, &mut ws, 2);
    assert_eq!(first["type"], "committed", "{first}");
    assert_eq!(first["value"], 0.3);
    let second = timed.reply(&mut rig, &mut ws, 3);
    assert_eq!(second["type"], "committed", "{second}");
    assert_eq!(second["value"], 0.35);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("gain = 0.35"), "{text}");

    // Second review (ADR 0048): a live edit of the value a queued commit
    // writes waits behind it, so the client's order holds (the later 0.7
    // stays live, not the older 0.5); one of another value goes on at once.
    let gain = rig.watch_parameter(&mut ws, 20, "/out.gain");
    let lock = ProjectLock::acquire(&rig.dir).unwrap();
    let mut c = commit(0.5);
    c["id"] = json!(21);
    send(&mut ws, c);
    send(
        &mut ws,
        json!({"type": "set", "address": "/out.gain", "value": 0.7, "id": 22}),
    );
    send(
        &mut ws,
        json!({"type": "set", "address": "/a.freq", "value": 220, "id": 23}),
    );
    let ack = timed.reply(&mut rig, &mut ws, 23);
    assert_eq!(ack["type"], "ack", "another value's edit goes on: {ack}");
    timed.quiet(&mut rig, &mut ws, 200, &[21, 22]);
    drop(lock);
    let committed = timed.reply(&mut rig, &mut ws, 21);
    assert_eq!(committed["type"], "committed", "{committed}");
    let ack = timed.reply(&mut rig, &mut ws, 22);
    assert_eq!(ack["type"], "ack", "{ack}");
    // Settled: the newest value plays.
    let mut last = None;
    let settle = Instant::now() + Duration::from_millis(400);
    while Instant::now() < settle {
        timed.pump(&mut rig);
        while let Some(got) = receive(&mut ws) {
            if let Some(f) = got.frame(3, gain) {
                last = Some(f.f64s()[0]);
            }
        }
    }
    assert_eq!(last, Some(0.7), "the client's last value plays");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("gain = 0.5"), "{text}");

    // One that waits longer than the host's wait fails, written nowhere,
    // and the value stays live and unsaved.
    rig.host().set_lock_wait(Duration::from_millis(200));
    let lock = ProjectLock::acquire(&rig.dir).unwrap();
    let mut c = commit(0.25);
    c["id"] = json!(7);
    let sent = Instant::now();
    send(&mut ws, c);
    let failed = timed.reply(&mut rig, &mut ws, 7);
    assert!(sent.elapsed() >= Duration::from_millis(200));
    assert_eq!(failed["type"], "error", "{failed}");
    assert_eq!(failed["code"], "write-failed", "{failed}");
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .contains("another writer holds the project's lock"),
        "{failed}"
    );
    drop(lock);
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    let described = rig.request(&mut ws, 8, json!({"type": "describe"}), "describe");
    let unsaved = &described["unsaved"];
    // The live `/a.freq` above is unsaved too.
    let gain = unsaved["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["address"] == "/out.gain")
        .unwrap_or_else(|| panic!("{unsaved}"));
    assert!(
        gain["error"]
            .as_str()
            .unwrap()
            .contains("holds the project's lock")
    );
    assert!(
        timed.slowest < Duration::from_millis(100),
        "{:?}",
        timed.slowest
    );
    rig.finish();
}

both!(another_writer_s_lock_never_stalls_the_control_thread);
