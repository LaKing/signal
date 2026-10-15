//! The local servers (`docs/ui-protocol.md`): static files over HTTP
//! ([`crate::http`]: a fixed pool of workers with deadlines) and the protocol
//! over a WebSocket (`tungstenite`) on a second port, both on 127.0.0.1
//! unless remote clients are allowed explicitly.
//!
//! Each WebSocket client has its own thread, which authenticates it (the
//! first message must be `hello` with the session token) before the control
//! thread ever hears of it, then writes what the control thread queued for
//! it and reads its messages. The control thread only ever `try_send`s to a
//! client's bounded queue, and evicts a client by setting its flag and
//! dropping its sender: it never waits for a client thread.
//!
//! At most [`MAX_CLIENTS`] connections have a thread, a connection that has
//! not said `hello` yet included (M2.8-9). One past that waits in a bounded
//! queue for one of [`REFUSERS`] refusal threads, which completes its
//! handshake, says `too-many-clients` and closes it with code 1013 (try
//! again later) and that reason, all within [`REFUSAL_DEADLINE`]; a client
//! that has not sent its handshake by then is closed without one (a close
//! reason needs a WebSocket). While the queue is full, accepting waits: no
//! connection is dropped for want of a refusal thread. Every one of these
//! threads ends on its own; none is joined.

use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::protocol::frame::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::{Error as WsError, Message, WebSocket};

use crate::PROTOCOL;
use crate::bindings::MAIN_PANEL;
use crate::http::{self, Files, HttpConnection};

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
/// WebSocket connections with a thread, those before `hello` included.
pub const MAX_CLIENTS: usize = 32;
/// Threads that refuse the connections past [`MAX_CLIENTS`].
pub(crate) const REFUSERS: usize = 2;
/// Connections waiting for a refusal thread.
pub(crate) const REFUSAL_QUEUE: usize = 16;
/// How long a refusal may take: the handshake, the reason, the close.
pub(crate) const REFUSAL_DEADLINE: Duration = Duration::from_secs(1);

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

/// The running servers; dropping them stops the HTTP workers and the accept
/// threads, each within a few polls whatever the clients do.
pub(crate) struct Server {
    http: SocketAddr,
    ws: SocketAddr,
    token: String,
    pub(crate) inbound: Receiver<Inbound>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// The project's `ui/` folder served under `/project/` (Signal.app opens
    /// another project while running: [`Server::set_project_ui`]).
    project: Arc<RwLock<PathBuf>>,
    /// How a client's connection is made (shared with the accept thread).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    accept: Arc<Accept>,
    /// The HTTP workers' queue (test hooks: in-process HTTP clients).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    http_queue: SyncSender<HttpConnection>,
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
        let http = TcpListener::bind(SocketAddr::new(ip, options.port))?;
        let http_addr = http.local_addr()?;
        let listener = TcpListener::bind(SocketAddr::new(ip, 0))?;
        listener.set_nonblocking(true)?;
        let ws_addr = listener.local_addr()?;
        let (inbound_tx, inbound) = sync_channel(INBOUND);

        let project = Arc::new(RwLock::new(options.project_ui.clone()));
        let files = Files {
            ui: options.ui_dir.clone(),
            project: Arc::clone(&project),
            port: http_addr.port(),
            remote: options.remote,
        };
        let (http_queue, mut threads) = http::workers(files, &stop)?;
        threads.push(http::accept(http, http_queue.clone(), Arc::clone(&stop))?);

        let accept = Arc::new(Accept {
            token: token.clone(),
            inbound: inbound_tx,
            queue: options.client_queue.max(1),
            http_port: http_addr.port(),
            remote: options.remote,
            stop: Arc::clone(&stop),
            next: Arc::new(AtomicU64::new(1)),
            clients: Arc::new(AtomicUsize::new(0)),
            refusals: refusers(http_addr.port(), options.remote, &stop)?,
        });
        let accepting = Arc::clone(&accept);
        threads.push(
            thread::Builder::new()
                .name("signal-ui-accept".to_owned())
                .spawn(move || accepting.run(&listener))?,
        );
        Ok(Server {
            http: http_addr,
            ws: ws_addr,
            token,
            inbound,
            stop,
            threads,
            project,
            accept,
            http_queue,
        })
    }

    /// Test hooks: the servers without a socket. Nothing is bound; clients
    /// connect through [`connect_local`](Server::connect_local), HTTP clients
    /// through [`LocalConnector::connect_http`], and are served by the same
    /// connection code and HTTP workers as over TCP.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn start_local(options: &ServerOptions) -> io::Result<Server> {
        let token = token()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (inbound_tx, inbound) = sync_channel(INBOUND);
        let none = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        let project = Arc::new(RwLock::new(options.project_ui.clone()));
        // Port 0: in-process HTTP clients send `Host: 127.0.0.1:0`.
        let files = Files {
            ui: options.ui_dir.clone(),
            project: Arc::clone(&project),
            port: 0,
            remote: false,
        };
        let (http_queue, threads) = http::workers(files, &stop)?;
        let accept = Arc::new(Accept {
            token: token.clone(),
            inbound: inbound_tx,
            queue: options.client_queue.max(1),
            http_port: 0,
            remote: false,
            stop: Arc::clone(&stop),
            next: Arc::new(AtomicU64::new(1)),
            clients: Arc::new(AtomicUsize::new(0)),
            refusals: refusers(0, false, &stop)?,
        });
        Ok(Server {
            http: none,
            ws: none,
            token,
            inbound,
            stop,
            threads,
            project,
            accept,
            http_queue,
        })
    }

    /// Test hooks: a client's end of an in-process connection, served by a
    /// client thread exactly as an accepted TCP connection is.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn connect_local(&self) -> crate::local::LocalStream {
        self.local_connector().connect()
    }

    /// Test hooks: what makes in-process connections, for another thread
    /// than the one that owns the servers.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn local_connector(&self) -> LocalConnector {
        LocalConnector {
            accept: Arc::clone(&self.accept),
            http: self.http_queue.clone(),
        }
    }

    /// Serves `dir` (a project's `ui/` folder) under `/project/` from now on.
    pub(crate) fn set_project_ui(&self, dir: PathBuf) {
        *self.project.write().unwrap_or_else(PoisonError::into_inner) = dir;
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
        self.url_of("/")
    }

    /// The project's main panel (`ui/main.html`, else the generated one),
    /// with the same fragment.
    pub(crate) fn panel_url(&self) -> String {
        self.url_of(&format!("/project/{MAIN_PANEL}"))
    }

    /// The page at `path` (from `/`), with the same fragment.
    pub(crate) fn url_of(&self, path: &str) -> String {
        format!(
            "http://127.0.0.1:{}{path}#ws={}&token={}",
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
pub(crate) fn host_port(text: &str) -> Option<(&str, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    Some((host, port.parse().ok()?))
}

pub(crate) fn loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// Test hooks: makes in-process connections to a server (from any thread),
/// served by a client thread exactly as an accepted TCP connection is.
#[cfg(feature = "test-hooks")]
#[derive(Clone)]
pub struct LocalConnector {
    accept: Arc<Accept>,
    http: SyncSender<HttpConnection>,
}

#[cfg(feature = "test-hooks")]
impl LocalConnector {
    /// A new client's end of an in-process connection.
    pub fn connect(&self) -> crate::local::LocalStream {
        let (client, server) = crate::local::pair();
        self.accept.spawn(server);
        client
    }

    /// A new HTTP client's end of an in-process connection, served by the
    /// HTTP workers exactly as an accepted TCP connection is (closed at once
    /// when their queue is full). It must send `Host: 127.0.0.1:0`.
    pub fn connect_http(&self) -> crate::local::LocalStream {
        let (client, server) = crate::local::pair();
        let _ = self.http.try_send(Box::new(server));
        client
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
    /// Connections with a thread ([`MAX_CLIENTS`]).
    clients: Arc<AtomicUsize>,
    /// Connections past the limit, for the refusal threads.
    refusals: SyncSender<Box<dyn Transport>>,
}

/// A connection's place among the [`MAX_CLIENTS`], given back when its
/// thread ends (or is never started).
struct Place(Arc<AtomicUsize>);

impl Drop for Place {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Starts the [`REFUSERS`] refusal threads (detached; each ends on stop or
/// when the servers are gone) and returns their queue.
fn refusers(
    http_port: u16,
    remote: bool,
    stop: &Arc<AtomicBool>,
) -> io::Result<SyncSender<Box<dyn Transport>>> {
    let (refusals, waiting) = sync_channel::<Box<dyn Transport>>(REFUSAL_QUEUE);
    let waiting = Arc::new(Mutex::new(waiting));
    for k in 0..REFUSERS {
        let waiting = Arc::clone(&waiting);
        let stop = Arc::clone(stop);
        thread::Builder::new()
            .name(format!("signal-ui-refuse-{k}"))
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let next = waiting
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .recv_timeout(Duration::from_millis(10));
                    match next {
                        Ok(stream) => refuse(stream, http_port, remote),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })?;
    }
    Ok(refusals)
}

/// A refused connection's stream: every read and write fails once
/// [`REFUSAL_DEADLINE`] has passed since its refusal began, so a client
/// sending its handshake a byte at a time holds a refusal thread no longer.
struct Deadline {
    stream: Box<dyn Transport>,
    until: Instant,
}

impl Deadline {
    fn within<T>(
        &mut self,
        mut io: impl FnMut(&mut dyn Transport) -> io::Result<T>,
    ) -> io::Result<T> {
        loop {
            if Instant::now() >= self.until {
                return Err(io::Error::from(ErrorKind::TimedOut));
            }
            match io(&mut *self.stream) {
                Err(e) if would_block(&e) || e.kind() == ErrorKind::Interrupted => {}
                other => return other,
            }
        }
    }
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.within(|s| s.read(buf))
    }
}

impl Write for Deadline {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.within(|s| s.write(buf))
    }
    fn flush(&mut self) -> io::Result<()> {
        self.within(|s| s.flush())
    }
}

/// Refuses a connection past [`MAX_CLIENTS`]: its handshake (the Origin
/// rule as for any client), the `too-many-clients` error, and a close with
/// code 1013 and that reason, within [`REFUSAL_DEADLINE`].
fn refuse(stream: Box<dyn Transport>, http_port: u16, remote: bool) {
    let poll = Some(Duration::from_millis(50));
    if stream.read_timeout(poll).is_err() || stream.write_timeout(poll).is_err() {
        return;
    }
    let stream = Deadline {
        stream,
        until: Instant::now() + REFUSAL_DEADLINE,
    };
    let Some(mut ws) = handshake(stream, http_port, remote) else {
        return;
    };
    let message = format!("at most {MAX_CLIENTS} clients are connected; try again later");
    let _ = ws.send(Message::text(error_text("too-many-clients", &message)));
    let _ = ws.close(Some(CloseFrame {
        code: CloseCode::Again,
        reason: "too-many-clients".into(),
    }));
    // Let the close go out: the stream waits for it up to the deadline.
    let _ = ws.flush();
    ws.get_mut().stream.finish();
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

    /// A client thread for an accepted connection, while fewer than
    /// [`MAX_CLIENTS`] have one; else it goes to the refusal threads,
    /// waiting (until stop) while their queue is full.
    fn spawn<S: Transport>(&self, stream: S) {
        let admitted = self
            .clients
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_CLIENTS).then_some(n + 1)
            })
            .is_ok();
        if !admitted {
            let mut stream: Box<dyn Transport> = Box::new(stream);
            loop {
                match self.refusals.try_send(stream) {
                    Ok(()) | Err(TrySendError::Disconnected(_)) => return,
                    Err(TrySendError::Full(back)) => {
                        if self.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        stream = back;
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }
        let place = Place(Arc::clone(&self.clients));
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
        // socket), and gives its place back (as does a thread not started).
        let _ = thread::Builder::new()
            .name(format!("signal-ui-client-{client}"))
            .spawn(move || {
                let _place = place;
                connection.run(stream);
            });
    }
}

/// A client connection's byte stream: TCP, or (test hooks) in-process.
pub(crate) trait Transport: Read + Write + Send + 'static {
    /// How long a read waits.
    fn read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// How long a write waits.
    fn write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// An HTTP response is complete: the client reads to its end.
    fn finish(&self) {}
}

impl Transport for TcpStream {
    fn read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(timeout)
    }
    fn write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_write_timeout(timeout)
    }
    fn finish(&self) {
        let _ = self.shutdown(Shutdown::Write);
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
        let Some(mut ws) = handshake(stream, self.http_port, self.remote) else {
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

/// The WebSocket handshake, refused (403) for a page of another origin.
fn handshake<S: Read + Write>(stream: S, http_port: u16, remote: bool) -> Option<WebSocket<S>> {
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
    tungstenite::accept_hdr_with_config(stream, check, Some(config())).ok()
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
