//! M2.8-4 AC1: the server in the app without a window: it starts (the page
//! URL carries the WebSocket's port and a session token in its fragment, a
//! new token per start), admits only a client with the token, starts
//! stopped on the Setup's last project, and on exit (or when dropped) stops,
//! records the window's size and the last project in the Setup, closes its
//! clients and ends its control thread.
//!
//! Each scenario runs twice: `::in_process` (servers that bind nothing,
//! clients connected in process, served by the same connection code) and
//! `::tcp` (127.0.0.1, which a sandbox that cannot bind loopback refuses;
//! the operator runs those).

use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_app::server::{AppConfig, AppServer};
use signal_app::setup::{Setup, Window};
use signal_app::state::Backend;
use signal_ui::LocalStream;
use tungstenite::{Message, WebSocket};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transport {
    Tcp,
    Local,
}

/// Runs scenario `name(transport)` as `name::tcp` and `name::in_process`.
macro_rules! both {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[test]
            fn tcp() {
                super::$name(super::Transport::Tcp);
            }
            #[test]
            fn in_process() {
                super::$name(super::Transport::Local);
            }
        }
    )*};
}

enum Stream {
    Tcp(TcpStream),
    Local(LocalStream),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            Stream::Local(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            Stream::Local(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            Stream::Local(s) => s.flush(),
        }
    }
}

type Ws = WebSocket<Stream>;

fn tone() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone")
}

/// A folder of its own and a Setup whose last project is the tone fixture.
fn setup(name: &str, transport: Transport) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "signal-app-server-{name}-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let path = base.join("setup.toml");
    Setup {
        last_project: Some(tone()),
        ..Setup::default()
    }
    .save(&path)
    .unwrap();
    (base, path)
}

fn start(transport: Transport, setup_path: &Path) -> AppServer {
    start_with(transport, setup_path, None)
}

fn start_with(transport: Transport, setup_path: &Path, project: Option<PathBuf>) -> AppServer {
    let config = AppConfig {
        setup_path: setup_path.to_owned(),
        project,
        backend: Backend::Null(Vec::new()),
        ui_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui"),
    };
    match transport {
        Transport::Tcp => AppServer::start(config),
        Transport::Local => AppServer::start_local(config),
    }
    .unwrap()
}

/// The `ws` and `token` of the page URL's fragment.
fn fragment(url: &str) -> (u16, String) {
    let (_, fragment) = url.split_once('#').unwrap();
    let mut ws = None;
    let mut token = None;
    for pair in fragment.split('&') {
        match pair.split_once('=') {
            Some(("ws", port)) => ws = port.parse().ok(),
            Some(("token", value)) => token = Some(value.to_owned()),
            _ => {}
        }
    }
    (ws.unwrap(), token.unwrap())
}

fn connect(server: &AppServer, transport: Transport) -> Ws {
    let poll = Some(Duration::from_millis(5));
    let (mut ws, _) = match transport {
        Transport::Local => {
            tungstenite::client("ws://localhost/", Stream::Local(server.connect_local())).unwrap()
        }
        Transport::Tcp => {
            let (port, _) = fragment(server.url());
            let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let url = format!("ws://127.0.0.1:{port}/");
            tungstenite::client(url.as_str(), Stream::Tcp(stream)).unwrap()
        }
    };
    match ws.get_mut() {
        Stream::Local(stream) => stream.set_read_timeout(poll),
        Stream::Tcp(stream) => stream.set_read_timeout(poll).unwrap(),
    }
    ws
}

enum Got {
    Json(Value),
    Closed,
}

/// The next text message or the close, within 10 s.
fn next(ws: &mut Ws) -> Got {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        match ws.read() {
            Ok(Message::Text(text)) => return Got::Json(serde_json::from_str(&text).unwrap()),
            Ok(Message::Close(_)) => return Got::Closed,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return Got::Closed,
        }
    }
    panic!("nothing within 10 s");
}

/// The next message of type `kind`.
fn of(ws: &mut Ws, kind: &str) -> Value {
    loop {
        match next(ws) {
            Got::Json(v) if v["type"] == kind => return v,
            Got::Json(_) => {}
            Got::Closed => panic!("closed while waiting for {kind}"),
        }
    }
}

fn hello(ws: &mut Ws, token: &str) {
    let hello = json!({"type": "hello", "protocol": 1, "token": token});
    ws.send(Message::text(hello.to_string())).unwrap();
}

fn request(ws: &mut Ws, message: Value, kind: &str) -> Value {
    ws.send(Message::text(message.to_string())).unwrap();
    of(ws, kind)
}

fn the_url_carries_a_new_token_in_its_fragment(transport: Transport) {
    let (base, path) = setup("token", transport);
    let first = start(transport, &path);
    assert!(
        first.url().contains("/ui/app/index.html#ws="),
        "{}",
        first.url()
    );
    let (_, token) = fragment(first.url());
    assert_eq!(token, first.token());
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    drop(first);
    let second = start(transport, &path);
    assert_ne!(second.token(), token, "a new token per start");
    drop(second);
    fs::remove_dir_all(&base).unwrap();
}

fn a_client_needs_the_token(transport: Transport) {
    let (base, path) = setup("hello", transport);
    let server = start(transport, &path);
    let mut stranger = connect(&server, transport);
    hello(&mut stranger, "0000");
    let refused = of(&mut stranger, "error");
    assert_eq!(refused["code"], "unauthorized");
    assert!(matches!(next(&mut stranger), Got::Closed));

    let mut page = connect(&server, transport);
    hello(&mut page, server.token());
    let welcome = of(&mut page, "welcome");
    assert_eq!(welcome["protocol"], 1);
    drop(server);
    fs::remove_dir_all(&base).unwrap();
}

fn it_starts_stopped_on_the_last_project(transport: Transport) {
    let (base, path) = setup("start", transport);
    let server = start(transport, &path);
    let mut page = connect(&server, transport);
    hello(&mut page, server.token());
    of(&mut page, "welcome");
    let app = request(&mut page, json!({"type": "app.get", "id": 1}), "app");
    assert_eq!(app["playing"], false);
    assert_eq!(app["project"]["dir"], tone().display().to_string());
    assert_eq!(app["project"]["sampleRate"], 48_000);
    assert_eq!(app["setupPath"], path.display().to_string());
    drop(server);
    fs::remove_dir_all(&base).unwrap();
}

fn exit_stops_saves_the_window_and_closes_clients(transport: Transport) {
    let (base, path) = setup("exit", transport);
    let mut server = start(transport, &path);
    let mut page = connect(&server, transport);
    hello(&mut page, server.token());
    of(&mut page, "welcome");
    let played = request(
        &mut page,
        json!({"type": "transport.play", "id": 1}),
        "transport",
    );
    assert_eq!(played["playing"], true);
    let window = Window {
        width: 1024.0,
        height: 700.0,
    };
    // Returns once the control thread has ended.
    server.exit(Some(window));
    let saved = Setup::load(&path).0;
    assert_eq!(saved.window, Some(window));
    assert_eq!(saved.last_project, Some(tone()));
    loop {
        match next(&mut page) {
            Got::Closed => break,
            Got::Json(_) => {}
        }
    }
    drop(server);
    fs::remove_dir_all(&base).unwrap();
}

fn dropping_it_saves_the_setup(transport: Transport) {
    let (base, path) = setup("drop", transport);
    fs::remove_file(&path).unwrap();
    let server = start_with(transport, &path, Some(tone()));
    drop(server);
    let saved = Setup::load(&path).0;
    assert_eq!(saved.last_project, Some(tone()), "saved without the window");
    assert_eq!(saved.window, None);
    fs::remove_dir_all(&base).unwrap();
}

both!(
    the_url_carries_a_new_token_in_its_fragment,
    a_client_needs_the_token,
    it_starts_stopped_on_the_last_project,
    exit_stops_saves_the_window_and_closes_clients,
    dropping_it_saves_the_setup,
);
