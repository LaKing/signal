//! The local servers (`docs/ui-protocol.md`): static files over HTTP
//! (`tiny_http`) and the protocol over a WebSocket (`tungstenite`) on a second
//! port, both on 127.0.0.1 unless remote clients are allowed explicitly.
//!
//! Two ports, because `tiny_http`'s upgraded connection is one boxed stream
//! with no timeouts and no way to split it: a client could not be read and
//! written by one thread without one side blocking the other.
//!
//! Each WebSocket client has its own thread, which authenticates it (the
//! first message must be `hello` with the session token) before the control
//! thread ever hears of it, then writes what the control thread queued for
//! it and reads its messages. The control thread only ever `try_send`s to a
//! client's bounded queue, and evicts a client by setting its flag and
//! dropping its sender: it never waits for a client thread.

use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::protocol::frame::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::{Error as WsError, Message, WebSocket};

use crate::PROTOCOL;

/// How long a client has to send `hello`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// A client thread's read timeout: how often it looks at its queue.
pub(crate) const CLIENT_POLL: Duration = Duration::from_millis(5);
/// A client whose socket takes nothing for this long is disconnected.
const WRITE_STALL: Duration = Duration::from_secs(2);
/// How long one write to a client's socket may wait, so its thread goes on
/// reading while the socket takes nothing.
const WRITE_POLL: Duration = Duration::from_millis(100);
/// The most a client's WebSocket holds unwritten: one message beyond what
/// the socket took (`drain`), and never more than this.
const MAX_WRITE_BUFFER: usize = 4 << 20;
/// The largest message a client may send.
const MAX_MESSAGE: usize = 64 << 10;
/// Messages from all clients waiting for the control thread.
const INBOUND: usize = 1024;

/// What the servers need.
#[derive(Clone, Debug)]
pub struct ServerOptions {
    /// The HTTP port; 0 for a free one. The WebSocket takes a free port.
    pub port: u16,
    /// Bind every interface (remote clients) instead of 127.0.0.1. Off by
    /// default; the token is then sent in clear text over the network.
    pub remote: bool,
    /// The UI library: the repository's `ui/` folder.
    pub ui_dir: PathBuf,
    /// The project's own `ui/` folder (its panels and `assets/`).
    pub project_ui: PathBuf,
    /// Each client's queue of outgoing messages.
    pub client_queue: usize,
}

/// What a client thread tells the control thread.
pub(crate) enum Inbound {
    /// A client said `hello` with the token.
    Connected {
        client: u64,
        queue: SyncSender<Outbound>,
        evicted: Arc<AtomicBool>,
        gate: Arc<AtomicBool>,
    },
    /// A text message from a connected client.
    Text { client: u64, text: String },
    /// The client is gone.
    Gone { client: u64 },
}

/// A message to a client.
pub(crate) enum Outbound {
    Text(String),
    Binary(Vec<u8>),
}

/// The running servers; dropping them stops the HTTP and accept threads.
pub(crate) struct Server {
    http: SocketAddr,
    ws: SocketAddr,
    token: String,
    pub(crate) inbound: Receiver<Inbound>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// How a client's connection is made (shared with the accept thread).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    accept: Arc<Accept>,
}

impl Server {
    pub(crate) fn start(options: &ServerOptions) -> io::Result<Server> {
        let ip = if options.remote {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        };
        let token = token()?;
        let stop = Arc::new(AtomicBool::new(false));
        let http = tiny_http::Server::http(SocketAddr::new(ip, options.port))
            .map_err(|e| io::Error::other(e.to_string()))?;
        let http_addr = http
            .server_addr()
            .to_ip()
            .ok_or_else(|| io::Error::other("the HTTP server has no IP address"))?;
        let listener = TcpListener::bind(SocketAddr::new(ip, 0))?;
        listener.set_nonblocking(true)?;
        let ws_addr = listener.local_addr()?;
        let (inbound_tx, inbound) = sync_channel(INBOUND);

        let files = Files {
            ui: options.ui_dir.clone(),
            project: options.project_ui.clone(),
            port: http_addr.port(),
            remote: options.remote,
        };
        let http_stop = Arc::clone(&stop);
        let http_thread = thread::Builder::new()
            .name("signal-ui-http".to_owned())
            .spawn(move || {
                while !http_stop.load(Ordering::Relaxed) {
                    match http.recv_timeout(Duration::from_millis(100)) {
                        Ok(Some(request)) => files.serve(request),
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })?;

        let accept = Arc::new(Accept {
            token: token.clone(),
            inbound: inbound_tx,
            queue: options.client_queue.max(1),
            http_port: http_addr.port(),
            remote: options.remote,
            stop: Arc::clone(&stop),
            next: Arc::new(AtomicU64::new(1)),
        });
        let accepting = Arc::clone(&accept);
        let accept_thread = thread::Builder::new()
            .name("signal-ui-accept".to_owned())
            .spawn(move || accepting.run(&listener))?;
        Ok(Server {
            http: http_addr,
            ws: ws_addr,
            token,
            inbound,
            stop,
            threads: vec![http_thread, accept_thread],
            accept,
        })
    }

    /// Test hooks: the servers without a socket. Nothing is bound; clients
    /// connect through [`connect_local`](Server::connect_local) and are
    /// served by the same connection code as over TCP.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn start_local(options: &ServerOptions) -> io::Result<Server> {
        let token = token()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (inbound_tx, inbound) = sync_channel(INBOUND);
        let none = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        let accept = Arc::new(Accept {
            token: token.clone(),
            inbound: inbound_tx,
            queue: options.client_queue.max(1),
            http_port: 0,
            remote: false,
            stop: Arc::clone(&stop),
            next: Arc::new(AtomicU64::new(1)),
        });
        Ok(Server {
            http: none,
            ws: none,
            token,
            inbound,
            stop,
            threads: Vec::new(),
            accept,
        })
    }

    /// Test hooks: a client's end of an in-process connection, served by a
    /// client thread exactly as an accepted TCP connection is.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn connect_local(&self) -> crate::local::LocalStream {
        let (client, server) = crate::local::pair();
        self.accept.spawn(server);
        client
    }

    pub(crate) fn http_addr(&self) -> SocketAddr {
        self.http
    }

    pub(crate) fn ws_addr(&self) -> SocketAddr {
        self.ws
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    /// The URL to open: the page, with the WebSocket's port and the token
    /// in the fragment (never sent over HTTP, so in no request or log).
    pub(crate) fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/#ws={}&token={}",
            self.http.port(),
            self.ws.port(),
            self.token
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// A random session token: 32 bytes of the system's random source, as hex.
fn token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Equal in time independent of where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// `host:port` (with `[…]` for IPv6) as host and port.
fn host_port(text: &str) -> Option<(&str, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    Some((host, port.parse().ok()?))
}

fn loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// The static files.
struct Files {
    ui: PathBuf,
    project: PathBuf,
    port: u16,
    remote: bool,
}

impl Files {
    fn serve(&self, request: tiny_http::Request) {
        let response = self.respond(&request);
        let _ = request.respond(response);
    }

    fn respond(&self, request: &tiny_http::Request) -> tiny_http::Response<io::Cursor<Vec<u8>>> {
        let status =
            |code: u16, text: &str| tiny_http::Response::from_string(text).with_status_code(code);
        if !matches!(
            request.method(),
            tiny_http::Method::Get | tiny_http::Method::Head
        ) {
            return status(405, "method not allowed");
        }
        // Without remote clients only the loopback names are served, so a
        // page of another site cannot read these files through a name that
        // resolves to 127.0.0.1 (DNS rebinding).
        let host = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Host"))
            .map(|h| h.value.as_str().to_owned());
        let allowed = match host.as_deref().and_then(host_port) {
            Some((host, port)) => self.remote || (loopback(host) && port == self.port),
            None => false,
        };
        if !allowed {
            return status(403, "forbidden host");
        }
        let url = request.url();
        let path = url.split(['?', '#']).next().unwrap_or("");
        let found = match path {
            "/" => file(&self.ui, "index.html"),
            _ => match (path.strip_prefix("/ui/"), path.strip_prefix("/project/")) {
                (Some(rest), _) => file(&self.ui, rest),
                (_, Some(rest)) => file(&self.project, rest),
                _ => None,
            },
        };
        match found.and_then(|path| fs::read(&path).ok().map(|bytes| (path, bytes))) {
            Some((path, bytes)) => {
                let header = |name: &str, value: &str| {
                    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
                };
                let mut response = tiny_http::Response::from_data(bytes);
                for h in [
                    header("Content-Type", mime(&path)),
                    header("X-Content-Type-Options", "nosniff"),
                    header("Cache-Control", "no-cache"),
                    header("Referrer-Policy", "no-referrer"),
                ]
                .into_iter()
                .flatten()
                {
                    response.add_header(h);
                }
                response
            }
            None => status(404, "not found"),
        }
    }
}

/// `relative` inside `root`, an existing file: no `..`, no hidden or empty
/// segment, no escape through a link.
fn file(root: &Path, relative: &str) -> Option<PathBuf> {
    let clean = !relative.is_empty()
        && !relative.contains(['%', '\\', '\0'])
        && relative
            .split('/')
            .all(|segment| !segment.is_empty() && !segment.starts_with('.'));
    if !clean {
        return None;
    }
    let root = fs::canonicalize(root).ok()?;
    let path = fs::canonicalize(root.join(relative)).ok()?;
    (path.starts_with(&root) && path.is_file()).then_some(path)
}

/// The MIME type of a served file.
pub(crate) fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The WebSocket's accept loop.
struct Accept {
    token: String,
    inbound: SyncSender<Inbound>,
    queue: usize,
    http_port: u16,
    remote: bool,
    stop: Arc<AtomicBool>,
    next: Arc<AtomicU64>,
}

impl Accept {
    fn run(&self, listener: &TcpListener) {
        while !self.stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(false).is_ok() {
                        self.spawn(stream);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    /// A client thread for an accepted connection.
    fn spawn<S: Transport>(&self, stream: S) {
        let client = self.next.fetch_add(1, Ordering::Relaxed);
        let connection = Connection {
            client,
            token: self.token.clone(),
            inbound: self.inbound.clone(),
            queue: self.queue,
            http_port: self.http_port,
            remote: self.remote,
            stop: Arc::clone(&self.stop),
        };
        // Detached: it ends on its own (stop, eviction, a closed or stalled
        // socket).
        let _ = thread::Builder::new()
            .name(format!("signal-ui-client-{client}"))
            .spawn(move || connection.run(stream));
    }
}

/// A client connection's byte stream: TCP, or (test hooks) in-process.
pub(crate) trait Transport: Read + Write + Send + 'static {
    /// How long a read waits.
    fn read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// How long a write waits.
    fn write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl Transport for TcpStream {
    fn read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(timeout)
    }
    fn write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_write_timeout(timeout)
    }
}

/// One client's thread.
struct Connection {
    client: u64,
    token: String,
    inbound: SyncSender<Inbound>,
    queue: usize,
    http_port: u16,
    remote: bool,
    stop: Arc<AtomicBool>,
}

/// An error message as the protocol sends it.
fn error_text(code: &str, message: &str) -> String {
    json!({"type": "error", "code": code, "message": message}).to_string()
}

impl Connection {
    fn run<S: Transport>(self, stream: S) {
        if stream.read_timeout(Some(HELLO_TIMEOUT)).is_err()
            || stream.write_timeout(Some(WRITE_POLL)).is_err()
        {
            return;
        }
        let (http_port, remote) = (self.http_port, self.remote);
        // The refusal's type is tungstenite's handshake callback's.
        #[allow(clippy::result_large_err)]
        let check = move |request: &Request, response: Response| {
            let header = |name: &str| {
                request
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            if origin_allowed(
                header("origin").as_deref(),
                header("host").as_deref(),
                http_port,
                remote,
            ) {
                Ok(response)
            } else {
                let mut refusal = ErrorResponse::new(Some("origin not allowed".to_owned()));
                *refusal.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
                Err(refusal)
            }
        };
        let Ok(mut ws) = tungstenite::accept_hdr_with_config(stream, check, Some(config())) else {
            return;
        };
        if let Err((code, close, message)) = self.hello(&mut ws) {
            let _ = ws.send(Message::text(error_text(code, &message)));
            let _ = ws.close(Some(CloseFrame {
                code: close,
                reason: code.to_owned().into(),
            }));
            // Let the close go out.
            let until = Instant::now() + Duration::from_millis(500);
            while Instant::now() < until && ws.flush().is_err() {}
            return;
        }
        if ws.get_mut().read_timeout(Some(CLIENT_POLL)).is_err() {
            return;
        }
        let (queue, outgoing) = sync_channel(self.queue);
        let evicted = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(AtomicBool::new(false));
        let connected = Inbound::Connected {
            client: self.client,
            queue,
            evicted: Arc::clone(&evicted),
            gate: Arc::clone(&gate),
        };
        if self.inbound.send(connected).is_ok() {
            self.serve(&mut ws, &outgoing, &evicted, &gate);
        }
        let _ = ws.close(None);
        let _ = ws.flush();
        let _ = self.inbound.send(Inbound::Gone {
            client: self.client,
        });
    }

    /// The first message: `hello` with the protocol version and the token.
    fn hello<S: Transport>(
        &self,
        ws: &mut WebSocket<S>,
    ) -> Result<(), (&'static str, CloseCode, String)> {
        let unauthorized = |message: &str| ("unauthorized", CloseCode::Policy, message.to_owned());
        let text = match ws.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(_) => return Err(unauthorized("the first message must be hello")),
            Err(_) => return Err(unauthorized("no hello")),
        };
        let hello: Value = serde_json::from_str(&text).map_err(|_| unauthorized("not JSON"))?;
        if hello.get("type").and_then(Value::as_str) != Some("hello") {
            return Err(unauthorized("the first message must be hello"));
        }
        let token = hello.get("token").and_then(Value::as_str).unwrap_or("");
        if !same(token, &self.token) {
            return Err(unauthorized("bad token"));
        }
        if hello.get("protocol").and_then(Value::as_u64) != Some(u64::from(PROTOCOL)) {
            return Err((
                "protocol",
                CloseCode::Protocol,
                format!("this server speaks protocol {PROTOCOL}"),
            ));
        }
        Ok(())
    }

    fn serve<S: Transport>(
        &self,
        ws: &mut WebSocket<S>,
        outgoing: &Receiver<Outbound>,
        evicted: &AtomicBool,
        gate: &AtomicBool,
    ) {
        let mut stalled: Option<Instant> = None;
        loop {
            if self.stop.load(Ordering::Relaxed) || evicted.load(Ordering::Relaxed) {
                return;
            }
            // The writer gate (test hooks): hold the queue unwritten.
            if !gate.load(Ordering::Relaxed) && !drain(ws, outgoing, &mut stalled, Instant::now()) {
                return;
            }
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let message = Inbound::Text {
                        client: self.client,
                        text: text.to_string(),
                    };
                    if self.inbound.send(message).is_err() {
                        return;
                    }
                }
                Ok(Message::Close(_)) => return,
                Ok(_) => {}
                Err(WsError::Io(e)) if would_block(&e) => {}
                Err(_) => return,
            }
        }
    }
}

/// The WebSocket configuration: messages from a client up to
/// [`MAX_MESSAGE`], and at most [`MAX_WRITE_BUFFER`] held unwritten.
fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE))
        .max_write_buffer_size(MAX_WRITE_BUFFER)
}

/// Writes what is queued for a client while its socket takes it: a message,
/// then a flush, so the WebSocket holds at most one message beyond what the
/// socket took. A flush the socket does not take stops the drain: the rest
/// stays in the bounded queue, where the control thread drops data frames
/// and evicts for a reply. Whether to keep the client: not once its socket
/// has taken nothing for [`WRITE_STALL`] (as of `now`), nor after an error
/// or once its queue is dropped (an eviction).
fn drain<S: Read + Write>(
    ws: &mut WebSocket<S>,
    outgoing: &Receiver<Outbound>,
    stalled: &mut Option<Instant>,
    now: Instant,
) -> bool {
    loop {
        match ws.flush() {
            Ok(()) => *stalled = None,
            Err(WsError::Io(e)) if would_block(&e) => {
                let since = *stalled.get_or_insert(now);
                return now.duration_since(since) <= WRITE_STALL;
            }
            Err(_) => return false,
        }
        let message = match outgoing.try_recv() {
            Ok(Outbound::Text(text)) => Message::text(text),
            Ok(Outbound::Binary(bytes)) => Message::binary(bytes),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        };
        match ws.write(message) {
            // Buffered; the next flush says whether the socket took it.
            Ok(()) => {}
            Err(WsError::Io(e)) if would_block(&e) => {}
            Err(_) => return false,
        }
    }
}

fn would_block(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// A browser's `Origin` must be the HTTP server's own: a loopback name and
/// the HTTP port; with remote clients, the host this request was sent to
/// (its `Host`) and the HTTP port. A client without `Origin` is not a
/// browser page and is let through to the token check.
pub(crate) fn origin_allowed(
    origin: Option<&str>,
    host: Option<&str>,
    http_port: u16,
    remote: bool,
) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let Some((origin_host, origin_port)) = origin.strip_prefix("http://").and_then(host_port)
    else {
        return false;
    };
    if origin_port != http_port {
        return false;
    }
    if remote {
        host.and_then(host_port)
            .is_some_and(|(host, _)| host == origin_host)
    } else {
        loopback(origin_host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::TrySendError;
    use tungstenite::protocol::Role;

    /// A client socket that reads nothing and, unless `takes`, times out on
    /// every write.
    struct Socket {
        takes: bool,
        written: usize,
    }

    impl Read for Socket {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(ErrorKind::WouldBlock))
        }
    }

    impl Write for Socket {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.takes {
                self.written += buf.len();
                Ok(buf.len())
            } else {
                Err(io::Error::from(ErrorKind::TimedOut))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn socket(takes: bool) -> WebSocket<Socket> {
        WebSocket::from_raw_socket(Socket { takes, written: 0 }, Role::Server, Some(config()))
    }

    /// auditor-1-unbounded-writer: frames keep arriving for a client whose
    /// writes keep timing out. One message leaves the bounded queue, the rest
    /// stay there (the control thread's `try_send` drops them), and the
    /// client goes once its socket has taken nothing for the stall time.
    #[test]
    fn a_socket_that_takes_nothing_holds_one_message_and_is_dropped_after_the_stall() {
        let (queue, outgoing) = sync_channel(8);
        let mut ws = socket(false);
        let start = Instant::now();
        let mut stalled = None;
        let (mut sent, mut dropped) = (0usize, 0usize);
        for i in 0..10_000u64 {
            match queue.try_send(Outbound::Binary(vec![0; 1024])) {
                Ok(()) => sent += 1,
                Err(TrySendError::Full(_)) => dropped += 1,
                Err(TrySendError::Disconnected(_)) => panic!("the queue is gone"),
            }
            let now = start + Duration::from_micros(i * 150);
            assert!(
                drain(&mut ws, &outgoing, &mut stalled, now),
                "kept within the stall time"
            );
        }
        let mut left = 0;
        while outgoing.try_recv().is_ok() {
            left += 1;
        }
        assert_eq!(left, 8, "the queue stays full");
        assert_eq!(sent - left, 1, "one message taken into the WebSocket");
        assert_eq!(dropped, 10_000 - sent);
        assert_eq!(ws.get_ref().written, 0);
        let late = start + WRITE_STALL + Duration::from_millis(1);
        assert!(
            !drain(&mut ws, &outgoing, &mut stalled, late),
            "dropped after the stall"
        );
    }

    #[test]
    fn a_socket_that_takes_everything_is_drained() {
        let (queue, outgoing) = sync_channel(8);
        let mut ws = socket(true);
        for _ in 0..8 {
            queue.try_send(Outbound::Text("{}".to_owned())).unwrap();
        }
        let mut stalled = None;
        assert!(drain(&mut ws, &outgoing, &mut stalled, Instant::now()));
        assert!(outgoing.try_recv().is_err());
        assert!(ws.get_ref().written >= 8 * 4);
        assert_eq!(stalled, None);
        drop(queue);
        assert!(
            !drain(&mut ws, &outgoing, &mut stalled, Instant::now()),
            "a dropped queue (an eviction) ends the client"
        );
    }
}
