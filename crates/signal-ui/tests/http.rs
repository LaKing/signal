//! M2.8-1: the static files (the repository's `ui/` and the project's `ui/`
//! and `ui/assets/`, with their MIME types), what is refused, the loopback
//! binding, and the WebSocket's Origin check.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use common::{Options, rig};
use tungstenite::client::IntoClientRequest;

/// The status line and the Content-Type of `GET path` with `host`.
fn get(addr: SocketAddr, method: &str, path: &str, host: &str) -> (u16, Option<String>, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
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

#[test]
fn static_files_have_their_types_and_stay_in_their_folders() {
    let mut rig = rig("http", Options::default());
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
    let addr = rig.host().http_addr();
    let host = format!("127.0.0.1:{}", addr.port());

    assert!(addr.ip().is_loopback(), "HTTP on {addr}");
    assert!(
        rig.host().ws_addr().ip().is_loopback(),
        "WebSocket on {}",
        rig.host().ws_addr()
    );

    for (path, wanted) in [
        ("/", "text/html; charset=utf-8"),
        ("/ui/lib/protocol.js", "text/javascript; charset=utf-8"),
        ("/project/panel.html", "text/html; charset=utf-8"),
        ("/project/panel.js", "text/javascript; charset=utf-8"),
        ("/project/assets/knob.svg", "image/svg+xml"),
        ("/project/assets/strip.png", "image/png"),
    ] {
        let (status, content_type, _) = get(addr, "GET", path, &host);
        assert_eq!(
            (status, content_type.as_deref()),
            (200, Some(wanted)),
            "{path}"
        );
    }
    let (_, _, png) = get(addr, "GET", "/project/assets/strip.png", &host);
    assert_eq!(png, b"\x89PNG\r\n\x1a\n");
    let (_, _, page) = get(
        addr,
        "GET",
        "/ui/lib/protocol.js",
        &format!("localhost:{}", addr.port()),
    );
    assert_eq!(
        page,
        fs::read(common::root().join("ui/lib/protocol.js")).unwrap()
    );

    for path in [
        "/ui/../Cargo.toml",
        "/project/../project.toml",
        "/project/.secret",
        "/ui/lib/%2e%2e/index.html",
        "/project/",
        "/elsewhere",
    ] {
        assert_eq!(get(addr, "GET", path, &host).0, 404, "{path}");
    }
    assert_eq!(get(addr, "POST", "/", &host).0, 405);
    // DNS rebinding: another name for 127.0.0.1 is refused.
    assert_eq!(
        get(addr, "GET", "/", &format!("example.com:{}", addr.port())).0,
        403
    );
    assert_eq!(get(addr, "GET", "/", "127.0.0.1:1").0, 403);
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
