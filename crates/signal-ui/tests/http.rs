//! M2.8-1: the static files (the repository's `ui/` and the project's `ui/`
//! and `ui/assets/`, with their MIME types), what is refused, the loopback
//! binding, and the WebSocket's Origin check. M2.8-8: a client that stops
//! reading holds one HTTP worker at most, is dropped after the write
//! deadline, and never delays another request or the server's shutdown.
//! Each runs over TCP (`::tcp`) and in process (`::in_process`), served by
//! the same HTTP workers.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use common::{Options, Rig, Stream, Transport, connect_http, http_host, rig};
use signal_ui::Host;
use tungstenite::client::IntoClientRequest;

/// The HTTP workers (`crates/signal-ui/src/http.rs`).
const WORKERS: usize = 4;
/// How long a client that takes nothing keeps its worker.
const WRITE_DEADLINE: Duration = Duration::from_secs(3);
/// A large asset: far more than any socket buffer holds.
const LARGE: usize = 32 << 20;

/// Sends `method path` with `host` on a new connection.
fn request(server: &Host, method: &str, path: &str, host: &str) -> Stream {
    let mut stream = connect_http(server, Duration::from_secs(10));
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream
}

/// The status, the Content-Type and the body of `method path` with `host`.
fn get(server: &Host, method: &str, path: &str, host: &str) -> (u16, Option<String>, Vec<u8>) {
    let mut stream = request(server, method, path, host);
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a header");
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap();
    let content_type = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Type: ").map(str::to_owned));
    (status, content_type, response[split + 4..].to_vec())
}

fn rig_for(name: &str, transport: Transport) -> Rig {
    rig(
        name,
        Options {
            transport,
            ..Options::default()
        },
    )
}

fn static_files_have_their_types_and_stay_in_their_folders(transport: Transport) {
    let mut rig = rig_for("http", transport);
    let assets = rig.dir.join("ui/assets");
    fs::create_dir_all(&assets).unwrap();
    fs::write(rig.dir.join("ui/panel.html"), "<p>panel</p>").unwrap();
    fs::write(rig.dir.join("ui/panel.js"), "export const x = 1;").unwrap();
    fs::write(
        assets.join("knob.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    fs::write(assets.join("strip.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    fs::write(rig.dir.join("ui/.secret"), "no").unwrap();
    let server = rig.host.take().unwrap();
    let host = http_host(&server);
    let port = server.http_addr().port();

    if transport == Transport::Tcp {
        let addr = server.http_addr();
        assert!(addr.ip().is_loopback(), "HTTP on {addr}");
        assert!(
            server.ws_addr().ip().is_loopback(),
            "WebSocket on {}",
            server.ws_addr()
        );
    }

    for (path, wanted) in [
        ("/", "text/html; charset=utf-8"),
        ("/ui/lib/protocol.js", "text/javascript; charset=utf-8"),
        ("/project/panel.html", "text/html; charset=utf-8"),
        ("/project/panel.js", "text/javascript; charset=utf-8"),
        ("/project/assets/knob.svg", "image/svg+xml"),
        ("/project/assets/strip.png", "image/png"),
    ] {
        let (status, content_type, _) = get(&server, "GET", path, &host);
        assert_eq!(
            (status, content_type.as_deref()),
            (200, Some(wanted)),
            "{path}"
        );
    }
    let (_, _, png) = get(&server, "GET", "/project/assets/strip.png", &host);
    assert_eq!(png, b"\x89PNG\r\n\x1a\n");
    let (_, _, page) = get(
        &server,
        "GET",
        "/ui/lib/protocol.js",
        &format!("localhost:{port}"),
    );
    assert_eq!(
        page,
        fs::read(common::root().join("ui/lib/protocol.js")).unwrap()
    );
    let (status, _, body) = get(&server, "HEAD", "/project/panel.html", &host);
    assert_eq!((status, body.len()), (200, 0), "HEAD has no body");

    for path in [
        "/ui/../Cargo.toml",
        "/project/../project.toml",
        "/project/.secret",
        "/ui/lib/%2e%2e/index.html",
        "/project/",
        "/elsewhere",
    ] {
        assert_eq!(get(&server, "GET", path, &host).0, 404, "{path}");
    }
    assert_eq!(get(&server, "POST", "/", &host).0, 405);
    // DNS rebinding: another name for 127.0.0.1 is refused.
    assert_eq!(
        get(&server, "GET", "/", &format!("example.com:{port}")).0,
        403
    );
    assert_eq!(get(&server, "GET", "/", "127.0.0.1:1").0, 403);
    rig.host = Some(server);
    rig.finish();
}

/// A large asset in the project's `ui/assets/`, its bytes.
fn large_asset(rig: &Rig) -> Vec<u8> {
    let assets = rig.dir.join("ui/assets");
    fs::create_dir_all(&assets).unwrap();
    let bytes: Vec<u8> = (0..LARGE).map(|i| (i % 251) as u8).collect();
    fs::write(assets.join("large.bin"), &bytes).unwrap();
    bytes
}

/// `n` clients that ask for the large asset and never read.
fn stalled(server: &Host, n: usize) -> Vec<Stream> {
    let host = http_host(server);
    let clients = (0..n)
        .map(|_| request(server, "GET", "/project/assets/large.bin", &host))
        .collect();
    // Each worker takes one and stalls on its full socket.
    thread::sleep(Duration::from_millis(500));
    clients
}

/// AC11: clients that stop reading a large asset (one worker each) delay
/// neither another client's request (answered within 1 s) nor dropping the
/// server (within 2 s), while they are still connected.
fn a_stalled_client_blocks_neither_another_request_nor_shutdown(transport: Transport) {
    let mut rig = rig_for("http-stalled", transport);
    large_asset(&rig);
    let server = rig.host.take().unwrap();
    let clients = stalled(&server, WORKERS - 1);
    let began = Instant::now();
    let (status, content_type, _) = get(&server, "GET", "/", &http_host(&server));
    let answered = began.elapsed();
    assert_eq!(status, 200);
    assert_eq!(content_type.as_deref(), Some("text/html; charset=utf-8"));
    assert!(
        answered < Duration::from_secs(1),
        "answered in {answered:?}"
    );
    let began = Instant::now();
    drop(server);
    let dropped = began.elapsed();
    assert!(dropped < Duration::from_secs(2), "dropped in {dropped:?}");
    drop(clients);
    rig.finish();
}

/// AC9: a client that takes nothing for the write deadline is dropped and
/// its worker freed: with every worker stalled, a request waits for that,
/// then is answered.
fn a_client_that_stops_reading_is_dropped_after_the_write_deadline(transport: Transport) {
    let mut rig = rig_for("http-deadline", transport);
    large_asset(&rig);
    let server = rig.host.take().unwrap();
    let mut clients = stalled(&server, WORKERS);
    thread::sleep(WRITE_DEADLINE + Duration::from_secs(1));
    let began = Instant::now();
    let (status, _, _) = get(&server, "GET", "/", &http_host(&server));
    let answered = began.elapsed();
    assert_eq!(status, 200);
    assert!(
        answered < Duration::from_secs(1),
        "answered in {answered:?}"
    );
    // The stalled client was cut off: what it reads now ends early.
    let mut response = Vec::new();
    match clients[0].read_to_end(&mut response) {
        Ok(_) => assert!(response.len() < LARGE, "{} bytes", response.len()),
        // A reset: dropped too.
        Err(error) => eprintln!("the stalled client's read: {error}"),
    }
    rig.host = Some(server);
    rig.finish();
}

/// AC9: a large asset is streamed, and arrives whole.
fn a_large_asset_arrives_whole(transport: Transport) {
    let mut rig = rig_for("http-large", transport);
    let bytes = large_asset(&rig);
    let server = rig.host.take().unwrap();
    let (status, content_type, body) = get(
        &server,
        "GET",
        "/project/assets/large.bin",
        &http_host(&server),
    );
    assert_eq!(status, 200);
    assert_eq!(content_type.as_deref(), Some("application/octet-stream"));
    assert_eq!(body.len(), bytes.len());
    assert!(body == bytes, "the bytes differ");
    rig.host = Some(server);
    rig.finish();
}

#[test]
fn a_page_of_another_origin_cannot_open_the_websocket() {
    let mut rig = rig("origin", Options::default());
    let http = rig.host().http_addr().port();
    let url = format!("ws://{}/", rig.host().ws_addr());
    for (origin, allowed) in [
        (format!("http://127.0.0.1:{http}"), true),
        (format!("http://localhost:{http}"), true),
        ("http://example.com".to_owned(), false),
        (format!("http://example.com:{http}"), false),
        ("http://127.0.0.1:1".to_owned(), false),
    ] {
        let mut request = url.as_str().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
        assert_eq!(tungstenite::connect(request).is_ok(), allowed, "{origin}");
    }
    rig.finish();
}

both!(
    static_files_have_their_types_and_stay_in_their_folders,
    a_stalled_client_blocks_neither_another_request_nor_shutdown,
    a_client_that_stops_reading_is_dropped_after_the_write_deadline,
    a_large_asset_arrives_whole,
);
