//! M2.8-1 AC3: slow or disconnected clients never block the control thread.
//! A client whose writer is held (the test hook: as if its socket took
//! nothing) fills its bounded queue: its data frames are dropped and counted,
//! and a control reply that does not fit evicts it; a client that drops its
//! socket is removed; another client's edits are answered throughout, and no
//! control step waits. M2.8-9: the number of connections is bounded, those
//! past it are refused with a reason, and shutdown stays prompt. A
//! connection that has not said `hello` by the admission deadline, however
//! slowly it sends, gives its place back.

mod common;

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use common::{Options, Stream, Transport, connect, rig, send};
use serde_json::{Value, json};
use signal_ui::{Host, LocalConnector, MAX_CLIENTS};
use tungstenite::Message;

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

/// Where a test opens extra connections from other threads: the host's
/// WebSocket port, or (in process) its connector.
#[derive(Clone)]
enum Dial {
    Tcp(SocketAddr),
    Local(LocalConnector),
}

impl Dial {
    fn of(host: &Host) -> Dial {
        if host.ws_addr().port() == 0 {
            Dial::Local(host.local_connector())
        } else {
            Dial::Tcp(host.ws_addr())
        }
    }

    /// A connection that never sends its handshake.
    fn silent(&self) -> Stream {
        match self {
            Dial::Tcp(addr) => Stream::Tcp(TcpStream::connect(addr).unwrap()),
            Dial::Local(connector) => Stream::Local(connector.connect()),
        }
    }

    /// A connection past the limit: its handshake, then what it was told
    /// before the close (the error's code) and the close (code, reason).
    fn refused(&self) -> (Value, u16, String) {
        let (mut ws, _) = tungstenite::client("ws://localhost/", self.silent()).unwrap();
        let poll = Some(Duration::from_millis(20));
        match ws.get_mut() {
            Stream::Tcp(s) => s.set_read_timeout(poll).unwrap(),
            Stream::Local(s) => s.set_read_timeout(poll),
        }
        let until = Instant::now() + Duration::from_secs(20);
        let mut error = Value::Null;
        loop {
            assert!(Instant::now() < until, "no close");
            match ws.read() {
                Ok(Message::Text(text)) => error = serde_json::from_str(&text).unwrap(),
                Ok(Message::Close(Some(frame))) => {
                    return (error, u16::from(frame.code), frame.reason.to_string());
                }
                Ok(other) => panic!("unexpected {other:?}"),
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => panic!("no close frame: {e}"),
            }
        }
    }
}

/// M2.8-9 AC3: at most `MAX_CLIENTS` connections have a thread. Every
/// connection past it that sends its handshake is told `too-many-clients`
/// and closed with code 1013 and that reason, also past what the refusal
/// threads and their queue hold at once (2 + 16): here two connections that
/// never send their handshake hold both refusal threads first, and 36 more
/// are all refused with the reason. Once a client leaves, one is admitted.
fn connections_past_the_limit_are_refused_with_a_reason(transport: Transport) {
    let mut rig = rig(
        "client-limit",
        Options {
            transport,
            ..Options::default()
        },
    );
    let mut admitted: Vec<_> = (0..MAX_CLIENTS).map(|_| rig.client().0).collect();
    let dial = Dial::of(rig.host());
    let _silent: Vec<_> = (0..2).map(|_| dial.silent()).collect();
    let refused: Vec<_> = (0..2 * (2 + 16))
        .map(|_| {
            let dial = dial.clone();
            thread::spawn(move || dial.refused())
        })
        .collect();
    for client in refused {
        let (error, code, reason) = client.join().unwrap();
        assert_eq!(error["type"], "error");
        assert_eq!(error["code"], "too-many-clients", "{error}");
        assert_eq!((code, reason.as_str()), (1013, "too-many-clients"));
    }
    // A client leaves: its place is given back once its thread ends.
    drop(admitted.pop());
    let _back = admit(&mut rig);
    drop(admitted);
    rig.finish();
}

/// A client that says `hello` with the token and is welcomed, once a place
/// is free: until then it is refused (`too-many-clients`) and tries again.
fn admit(rig: &mut common::Rig) -> common::Ws {
    let token = rig.host().token().to_owned();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < until, "no place was given back");
        let mut ws = connect(rig.host());
        // A refused connection may be closed before its hello goes out.
        let hello = json!({"type": "hello", "protocol": 1, "token": token});
        let _ = ws.send(Message::text(hello.to_string()));
        let got = rig.wait(&mut ws, "welcome or refusal", |got| {
            got.of("welcome").or_else(|| got.of("error")).cloned()
        });
        if got["type"] == "welcome" {
            return ws;
        }
        assert_eq!(got["code"], "too-many-clients");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Whether the server closed `stream` within `within`: its end of the
/// stream (or a reset), not a read that times out.
fn closed(stream: &mut Stream, within: Duration) -> bool {
    let poll = Some(Duration::from_millis(20));
    match stream {
        Stream::Tcp(s) => s.set_read_timeout(poll).unwrap(),
        Stream::Local(s) => s.set_read_timeout(poll),
    }
    let until = Instant::now() + within;
    let mut buf = [0u8; 256];
    while Instant::now() < until {
        match stream.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return true,
        }
    }
    false
}

/// P2 audit (slow handshakes): a connection that has not said `hello` with
/// the token loses its place at the admission deadline, counted from its
/// acceptance across the handshake and `hello`, not per read. Every place is
/// taken by connections that are silent or that trickle their handshake a
/// byte every 20 ms (no read ever waits long); a newcomer is refused. Once
/// the deadline (1 s here) has passed, every one of them is closed while
/// the trickling goes on, and a healthy client is admitted, as are as many
/// as there are places.
fn connections_that_do_not_say_hello_in_time_give_their_place_back(transport: Transport) {
    let mut rig = rig(
        "admission-deadline",
        Options {
            transport,
            ..Options::default()
        },
    );
    rig.host().set_admission_deadline(Duration::from_secs(1));
    let dial = Dial::of(rig.host());
    let began = Instant::now();
    let mut silent: Vec<_> = (0..MAX_CLIENTS / 2).map(|_| dial.silent()).collect();
    let trickling = Arc::new(AtomicBool::new(true));
    let trickles: Vec<_> = (MAX_CLIENTS / 2..MAX_CLIENTS)
        .map(|_| {
            let mut stream = dial.silent();
            let trickling = Arc::clone(&trickling);
            thread::spawn(move || {
                let mut sent = stream.write_all(b"GET / HTTP/1.1\r\nX-Slow: ");
                while sent.is_ok() && trickling.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(20));
                    sent = stream.write_all(b"a");
                }
                // Closed by the server, else still trickling at the end.
                sent.is_err()
            })
        })
        .collect();
    let (error, code, _) = dial.refused();
    assert_eq!(error["code"], "too-many-clients", "every place is taken");
    assert_eq!(code, 1013);

    let mut first = admit(&mut rig);
    let admitted = began.elapsed();
    assert!(
        admitted < Duration::from_secs(4),
        "admitted after {admitted:?}, not at the 1 s deadline"
    );
    for stream in &mut silent {
        assert!(closed(stream, Duration::from_secs(2)), "a silent one stays");
    }
    // Each trickle's writes fail once the server has closed it.
    thread::sleep(Duration::from_millis(500));
    trickling.store(false, Ordering::Relaxed);
    for trickle in trickles {
        assert!(trickle.join().unwrap(), "a trickling one stays");
    }

    // Every place is free again: as many clients as there are places.
    let mut healthy: Vec<_> = (1..MAX_CLIENTS).map(|_| admit(&mut rig)).collect();
    rig.request(&mut first, 1, json!({"type": "ping"}), "pong");
    healthy.push(first);
    drop(healthy);
    rig.finish();
}

/// P2 audit (slow handshakes): stopping the servers ends the threads of
/// connections that have not said `hello` at once, not at their deadline
/// (5 s by default): each is closed within 1 s of the host's drop.
fn connections_before_hello_end_at_shutdown(transport: Transport) {
    let mut rig = rig(
        "admission-shutdown",
        Options {
            transport,
            ..Options::default()
        },
    );
    let dial = Dial::of(rig.host());
    let mut silent: Vec<_> = (0..4).map(|_| dial.silent()).collect();
    for stream in &mut silent {
        assert!(!closed(stream, Duration::from_millis(100)), "closed early");
    }
    drop(rig.host.take());
    for stream in &mut silent {
        assert!(
            closed(stream, Duration::from_secs(1)),
            "open after the stop"
        );
    }
    rig.finish();
}

/// M2.8-9 AC4: with every client place taken by a client whose socket takes
/// nothing (its writer held), both refusal threads held by connections that
/// never send their handshake and more waiting for them, dropping the host
/// (its servers stop) takes less than 2 s.
fn shutdown_with_stalled_clients_at_the_limit_is_prompt(transport: Transport) {
    let mut rig = rig(
        "client-limit-shutdown",
        Options {
            transport,
            ..Options::default()
        },
    );
    let mut stalled = Vec::new();
    for _ in 0..MAX_CLIENTS {
        let (mut ws, id) = rig.client();
        assert!(rig.host().hold_writer(id, true));
        send(
            &mut ws,
            json!({"type": "subscribe", "stream": "meters", "rate": 60, "id": 1}),
        );
        stalled.push(ws);
    }
    let dial = Dial::of(rig.host());
    let silent: Vec<_> = (0..2 + 16 + 4)
        .map(|_| {
            let dial = dial.clone();
            thread::spawn(move || dial.silent())
        })
        .collect();
    for _ in 0..20 {
        let _ = rig.host().run_null(256, |_| {});
        rig.host().step();
    }
    let host = rig.host.take().unwrap();
    let began = Instant::now();
    drop(host);
    let dropped = began.elapsed();
    assert!(dropped < Duration::from_secs(2), "dropped in {dropped:?}");
    drop(stalled);
    drop(silent);
    rig.finish();
}

both!(
    connections_past_the_limit_are_refused_with_a_reason,
    shutdown_with_stalled_clients_at_the_limit_is_prompt,
    connections_that_do_not_say_hello_in_time_give_their_place_back,
    connections_before_hello_end_at_shutdown,
);
