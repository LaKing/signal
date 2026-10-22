//! M2.8-4 AC1: the server in the app without a window: it starts (the page
//! URL carries the WebSocket's port and a session token in its fragment, a
//! new token per start), admits only a client with the token, starts
//! stopped on the Setup's last project, and on exit (or when dropped) stops,
//! records the window's size and the last project in the Setup, closes its
//! clients and ends its control thread. M2.8-12: the window's requests
//! for a .signal project (its edited state, Save, Save As, Open, exit).
//!
//! Each scenario runs twice: `::in_process` (servers that bind nothing,
//! clients connected in process, served by the same connection code) and
//! `::tcp` (127.0.0.1, which a sandbox that cannot bind loopback refuses;
//! the operator runs those).

use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_app::server::{AppConfig, AppServer};
use signal_app::setup::{Setup, Window};
use signal_app::state::Backend;
use signal_project::archive;
use signal_ui::{Edits, LocalStream, ProjectStatus};
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
        open_root: setup_path.with_file_name("Open"),
        backend: Backend::Null(Vec::new()),
        ui_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui"),
        on_project: None,
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

/// ADR 0033: Signal.app has no Play or Stop; switched on, the Setup's last
/// project plays as the app starts.
fn it_plays_the_last_project_at_start(transport: Transport) {
    let (base, path) = setup("start", transport);
    let server = start(transport, &path);
    let mut page = connect(&server, transport);
    hello(&mut page, server.token());
    of(&mut page, "welcome");
    let app = request(&mut page, json!({"type": "app.get", "id": 1}), "app");
    assert_eq!(app["playing"], true);
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
    server.exit(Some(window), false);
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

/// The control addresses a `describe` lists.
fn controls(described: &Value) -> Vec<String> {
    described["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["address"].as_str().unwrap().to_owned())
        .collect()
}

/// M2.8-11 AC2 ("Parameters go empty after switching tabs"): a page that
/// connects while the app plays (as the old Transport page did on each
/// visit) is described every parameter. The server was right; the page's
/// meters threw in their `describe` listener, which kept the parameter list
/// from its own (`ui/signal-ui.test.js`).
fn a_later_client_describes_every_parameter_while_playing(transport: Transport) {
    let (base, path) = setup("later", transport);
    let server = start(transport, &path);
    let mut first = connect(&server, transport);
    hello(&mut first, server.token());
    of(&mut first, "welcome");
    // Playing from the start (ADR 0033); a restart (Project ▸ Restart from
    // Files) plays it again.
    let first_described = request(&mut first, json!({"type": "describe", "id": 1}), "describe");
    assert_eq!(first_described["playing"], true);
    let every = controls(&first_described);
    assert!(every.iter().any(|a| a == "/osc.freq"), "{every:?}");
    assert!(every.iter().any(|a| a == "/out.gain"), "{every:?}");
    let played = request(
        &mut first,
        json!({"type": "transport.play", "id": 2}),
        "transport",
    );
    assert_eq!(played["playing"], true);

    let mut later = connect(&server, transport);
    hello(&mut later, server.token());
    let welcome = of(&mut later, "welcome");
    assert_eq!(welcome["power"], "on");
    let described = request(&mut later, json!({"type": "describe", "id": 1}), "describe");
    assert_eq!(described["playing"], true);
    assert_eq!(
        controls(&described),
        every,
        "every parameter, while playing"
    );
    drop(server);
    fs::remove_dir_all(&base).unwrap();
}

/// M2.8-12: a server started on `project`, telling `seen` the project shown.
fn start_seen(
    transport: Transport,
    setup_path: &Path,
    project: PathBuf,
) -> (AppServer, mpsc::Receiver<Option<ProjectStatus>>) {
    let (seen, shown) = mpsc::channel();
    let config = AppConfig {
        setup_path: setup_path.to_owned(),
        project: Some(project),
        open_root: setup_path.with_file_name("Open"),
        backend: Backend::Null(Vec::new()),
        ui_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui"),
        on_project: Some(Box::new(move |status| {
            let _ = seen.send(status);
        })),
    };
    let server = match transport {
        Transport::Tcp => AppServer::start(config),
        Transport::Local => AppServer::start_local(config),
    }
    .unwrap();
    (server, shown)
}

/// An edit made in the working folder, as a parameter write makes one.
fn edit(dir: &Path, from: &str, to: &str) {
    let toml = dir.join("project.toml");
    let text = fs::read_to_string(&toml).unwrap();
    let edited = text.replace(&format!("gain = {from}"), &format!("gain = {to}"));
    assert_ne!(text, edited);
    fs::write(toml, edited).unwrap();
}

/// M2.8-12 AC15-AC20 through the window's requests (no window): the
/// project and its edited state, Save, Save As, and exit keeping an edited
/// working folder unless its edits are let go; the window is told of each
/// change (its title).
fn the_window_s_requests_reach_the_project(transport: Transport) {
    let (base, path) = setup("project-file", transport);
    let file = base.join("tone.signal");
    archive::write_atomic(&file, &archive::pack(&tone()).unwrap()).unwrap();
    let (mut server, shown) = start_seen(transport, &path, file.clone());
    let project = server.project().unwrap();
    assert_eq!(project.file.as_deref(), Some(file.as_path()));
    assert_eq!(project.name, "tone");
    assert!(!project.edited);
    assert!(project.dir.starts_with(base.join("Open")));
    let first = shown
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(first.file.as_deref(), Some(file.as_path()));

    edit(&project.dir, "0.5", "0.25");
    assert!(
        server.project().unwrap().edited,
        "the working folder differs"
    );
    let told = loop {
        let status = shown
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        if status.edited {
            break status;
        }
    };
    assert_eq!(told.name, "tone");
    server.save().unwrap();
    assert!(!server.project().unwrap().edited);
    let unpacked = base.join("check");
    archive::unpack(&fs::read(&file).unwrap(), &unpacked).unwrap();
    assert!(
        fs::read_to_string(unpacked.join("project.toml"))
            .unwrap()
            .contains("gain = 0.25")
    );

    let copy = base.join("copy.signal");
    server.save_as(&copy).unwrap();
    assert_eq!(
        server.project().unwrap().file.as_deref(),
        Some(copy.as_path())
    );
    assert_eq!(Setup::load(&path).0.last_project, Some(copy.clone()));

    // Exit with the edits unanswered: the working folder is kept.
    edit(&project.dir, "0.25", "0.125");
    server.exit(None, false);
    assert!(
        project.dir.join("project.toml").is_file(),
        "the edits are kept"
    );

    // Exit with Don't Save: removed.
    let (mut server, _) = start_seen(transport, &path, copy.clone());
    let again = server.project().unwrap();
    assert_ne!(again.dir, project.dir);
    edit(&again.dir, "0.25", "0.75");
    server.exit(None, true);
    assert!(!again.dir.exists(), "the edits were let go");

    // Open: another project, the edits saved first.
    let (server, _) = start_seen(transport, &path, copy.clone());
    let third = server.project().unwrap();
    edit(&third.dir, "0.25", "0.75");
    server.open(&tone(), Edits::Save).unwrap();
    let opened = server.project().unwrap();
    assert_eq!(opened.file, None);
    assert_eq!(opened.dir, tone());
    assert!(!third.dir.exists());
    let refused = server
        .open(&base.join("missing.signal"), Edits::Ask)
        .unwrap_err();
    assert!(refused.contains("missing.signal"), "{refused}");
    drop(server);
    fs::remove_dir_all(&base).unwrap();
}

/// ADR 0032: every window is one more client of the one server: each
/// gets the pushes, so what one window does (Play here) shows in all.
fn several_windows_are_clients_of_one_server(transport: Transport) {
    let (base, path) = setup("windows", transport);
    let server = start(transport, &path);
    let mut pages: Vec<Ws> = (0..3).map(|_| connect(&server, transport)).collect();
    for page in &mut pages {
        hello(page, server.token());
        of(page, "welcome");
    }
    const { assert!(signal_ui::MAX_CLIENTS >= 16, "room for many windows") };
    // It plays from the start (ADR 0033); one window's stop, then its
    // restart, reach the others.
    for (id, (kind, playing)) in [("transport.stop", false), ("transport.play", true)]
        .into_iter()
        .enumerate()
    {
        let reply = request(
            &mut pages[0],
            json!({"type": kind, "id": id + 1}),
            "transport",
        );
        assert_eq!(reply["playing"], playing);
        for page in &mut pages[1..] {
            loop {
                let app = of(page, "app");
                if app["playing"] == playing {
                    break;
                }
            }
        }
    }
    drop(server);
    fs::remove_dir_all(&base).unwrap();
}

/// Messages of type `kind` within `within`.
fn seen_within(ws: &mut Ws, kind: &str, within: Duration) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let v: Value = serde_json::from_str(&text).unwrap();
                if v["type"] == kind {
                    return true;
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return false,
        }
    }
    false
}

/// ADR 0032: the window layout is written into the open project's
/// `ui/windows.toml` by the control thread and taken as seen (nothing
/// reloads, where a write by anyone else does); a layout of another
/// project, or through a linked `ui/`, is refused and writes nothing.
fn the_layout_is_written_into_the_project_as_seen(transport: Transport) {
    let (base, path) = setup("layout", transport);
    let dir = base.join("tone");
    fs::create_dir_all(&dir).unwrap();
    fs::copy(tone().join("project.toml"), dir.join("project.toml")).unwrap();
    let server = start_with(transport, &path, Some(dir.clone()));
    let mut page = connect(&server, transport);
    hello(&mut page, server.token());
    of(&mut page, "welcome");
    let played = request(
        &mut page,
        json!({"type": "transport.play", "id": 1}),
        "transport",
    );
    assert_eq!(played["playing"], true);

    server
        .write_windows(&dir, Box::new(|text| Ok(format!("{text}# one\n"))))
        .unwrap();
    server
        .write_windows(&dir, Box::new(|text| Ok(format!("{text}# two\n"))))
        .unwrap();
    // Without waiting: the answer comes on its own.
    let answer = server.write_windows_later(&dir, Box::new(|text| Ok(format!("{text}# three\n"))));
    answer
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    let refused = server.write_windows_later(&tone(), Box::new(|_| Ok("x".to_owned())));
    assert!(
        refused
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .is_err()
    );
    let file = dir.join(signal_ui::WINDOWS_FILE);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "# one\n# two\n# three\n"
    );
    assert!(
        !seen_within(&mut page, "reload", Duration::from_millis(800)),
        "the app's own write reloads nothing"
    );
    fs::write(dir.join("ui/other.toml"), "x = 1\n").unwrap();
    assert!(
        seen_within(&mut page, "reload", Duration::from_secs(10)),
        "another writer's change is reloaded"
    );

    let refused = server
        .write_windows(&tone(), Box::new(|_| Ok("x".to_owned())))
        .unwrap_err();
    assert!(refused.contains("is not the open project"), "{refused}");
    let failed = server
        .write_windows(&dir, Box::new(|_| Err("bad".to_owned())))
        .unwrap_err();
    assert!(failed.contains("bad"), "{failed}");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "# one\n# two\n# three\n"
    );

    // A linked ui/ is served, never written.
    let elsewhere = base.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::remove_dir_all(dir.join("ui")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join("ui")).unwrap();
    let linked = server
        .write_windows(&dir, Box::new(|_| Ok("x".to_owned())))
        .unwrap_err();
    assert!(linked.contains("link"), "{linked}");
    assert!(!elsewhere.join("windows.toml").exists());
    drop(server);
    fs::remove_dir_all(&base).unwrap();
}

both!(
    several_windows_are_clients_of_one_server,
    the_layout_is_written_into_the_project_as_seen,
    a_later_client_describes_every_parameter_while_playing,
    the_url_carries_a_new_token_in_its_fragment,
    a_client_needs_the_token,
    it_plays_the_last_project_at_start,
    exit_stops_saves_the_window_and_closes_clients,
    dropping_it_saves_the_setup,
    the_window_s_requests_reach_the_project,
);
