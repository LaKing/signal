//! A UI-protocol client (`docs/ui-protocol.md`): the WebSocket handshake,
//! `hello` with the token, then one thread that owns the socket, writes
//! what the tools send and always reads what the server sends, so the
//! server never finds its queue for this client full (it would drop frames,
//! then evict the client on a reply: "Delivery: slow and gone clients").
//! Replies go to the request waiting for their `id` (the server's pushes are
//! read and let go: the tools ask for what they need); binary frames are
//! kept per subscription until taken.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use signal_ui::{LocalStream, PROTOCOL};
use tungstenite::{Message, WebSocket};

/// How long a request waits for its reply.
const REPLY: Duration = Duration::from_secs(30);
/// How long the socket thread's reads wait before it looks at its queue.
const POLL: Duration = Duration::from_millis(2);
/// Frames kept per subscription before the oldest are let go.
const KEPT_FRAMES: usize = 8192;
/// A client message is at most 64 KiB (`ui-protocol.md` "Limits"): a larger
/// one would close the connection, so it is refused before it is sent.
pub(crate) const MAX_MESSAGE: usize = 64 << 10;

/// A byte stream the socket thread can poll.
trait Duplex: Read + Write + Send + 'static {
    fn poll(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl Duplex for LocalStream {
    fn poll(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(timeout);
        Ok(())
    }
}

impl Duplex for TcpStream {
    fn poll(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(timeout)
    }
}

/// A binary frame (`ui-protocol.md` "Binary frames").
#[derive(Clone, Debug)]
pub(crate) struct Frame {
    pub(crate) first: u64,
    pub(crate) count: u32,
    pub(crate) extra: u32,
    pub(crate) payload: Vec<u8>,
}

impl Frame {
    fn parse(bytes: &[u8]) -> Option<(u32, Frame)> {
        let header = bytes.get(..24)?;
        let u32_at = |at: usize| {
            header
                .get(at..at + 4)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
        };
        let first = header
            .get(8..16)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)?;
        Some((
            u32_at(4)?,
            Frame {
                first,
                count: u32_at(16)?,
                extra: u32_at(20)?,
                payload: bytes.get(24..)?.to_vec(),
            },
        ))
    }

    pub(crate) fn f32s(&self) -> Vec<f32> {
        let (chunks, _) = self.payload.as_chunks::<4>();
        chunks.iter().map(|c| f32::from_le_bytes(*c)).collect()
    }

    pub(crate) fn f64s(&self) -> Vec<f64> {
        let (chunks, _) = self.payload.as_chunks::<8>();
        chunks.iter().map(|c| f64::from_le_bytes(*c)).collect()
    }
}

#[derive(Default)]
struct Shared {
    replies: Mutex<HashMap<u64, Sender<Value>>>,
    frames: Mutex<HashMap<u32, VecDeque<Frame>>>,
    closed: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A connected, authenticated client. Dropping it closes the connection.
pub(crate) struct Client {
    outgoing: Sender<Message>,
    shared: Arc<Shared>,
    next: AtomicU64,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
    }
}

impl Client {
    /// Connects in process to a host that binds nothing.
    pub(crate) fn connect_local(stream: LocalStream, token: &str) -> Result<Client, String> {
        Client::connect(stream, "ws://localhost/", token)
    }

    /// Attaches to a running server: `url` is the page URL `signal ui`
    /// prints (`http://127.0.0.1:<http>/…#ws=<port>&token=<token>`) or
    /// `ws://127.0.0.1:<port>/#token=<token>`; only loopback hosts.
    pub(crate) fn attach(url: &str) -> Result<Client, String> {
        let (address, token) = parse_url(url)?;
        let stream =
            TcpStream::connect(address).map_err(|e| format!("connecting to {address}: {e}"))?;
        Client::connect(stream, &format!("ws://{address}/"), &token)
    }

    fn connect<S: Duplex>(stream: S, url: &str, token: &str) -> Result<Client, String> {
        stream
            .poll(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let (mut ws, _) = tungstenite::client(url, stream)
            .map_err(|e| format!("the WebSocket handshake failed: {e}"))?;
        let hello = json!({"type": "hello", "protocol": PROTOCOL, "token": token});
        ws.send(Message::text(hello.to_string()))
            .map_err(|e| format!("hello: {e}"))?;
        // The server answers `hello` within its admission deadline (5 s).
        let until = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match ws.read() {
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) && std::time::Instant::now() < until => {}
                Ok(Message::Text(text)) => {
                    let value: Value =
                        serde_json::from_str(text.as_str()).map_err(|e| e.to_string())?;
                    match value.get("type").and_then(Value::as_str) {
                        Some("welcome") => break,
                        Some("error") => {
                            return Err(format!(
                                "the server refused the connection: {}",
                                value["message"].as_str().unwrap_or("")
                            ));
                        }
                        _ => {}
                    }
                }
                Ok(Message::Close(_)) => return Err("the server closed the connection".to_owned()),
                Ok(_) => {}
                Err(e) => return Err(format!("waiting for welcome: {e}")),
            }
        }
        ws.get_mut().poll(Some(POLL)).map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared::default());
        let (outgoing, queued) = channel();
        let socket = Arc::clone(&shared);
        thread::Builder::new()
            .name("signal-mcp-socket".to_owned())
            .spawn(move || serve(ws, &queued, &socket))
            .map_err(|e| e.to_string())?;
        Ok(Client {
            outgoing,
            shared,
            next: AtomicU64::new(1),
        })
    }

    /// Sends `message` and waits for its reply: `Ok` with the reply, or
    /// `Err` with the server's `error` message (or one made here).
    pub(crate) fn request(&self, mut message: Value) -> Result<Value, Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        message["id"] = json!(id);
        let text = message.to_string();
        if text.len() > MAX_MESSAGE {
            return Err(local_error(
                "too-large",
                format!(
                    "not sent: the message is {} bytes, over the UI protocol's {} KiB limit for a client message",
                    text.len(),
                    MAX_MESSAGE >> 10
                ),
            ));
        }
        let (tx, rx) = channel();
        lock(&self.shared.replies).insert(id, tx);
        if self.shared.closed.load(Ordering::Relaxed)
            || self.outgoing.send(Message::text(text)).is_err()
        {
            lock(&self.shared.replies).remove(&id);
            return Err(local_error(
                "closed",
                "the connection to the server is closed",
            ));
        }
        let reply = rx.recv_timeout(REPLY);
        lock(&self.shared.replies).remove(&id);
        match reply {
            Ok(reply) if reply.get("type").and_then(Value::as_str) == Some("error") => Err(reply),
            Ok(reply) => Ok(reply),
            Err(_) => Err(local_error(
                "timeout",
                "the server did not answer within 30 s",
            )),
        }
    }

    /// Sends `message` without waiting for a reply.
    pub(crate) fn send(&self, message: &Value) {
        let _ = self.outgoing.send(Message::text(message.to_string()));
    }

    /// Takes the frames that arrived for subscription `sub`.
    pub(crate) fn frames(&self, sub: u32) -> Vec<Frame> {
        lock(&self.shared.frames)
            .get_mut(&sub)
            .map(|q| q.drain(..).collect())
            .unwrap_or_default()
    }

    /// Forgets subscription `sub`'s frames.
    pub(crate) fn forget(&self, sub: u32) {
        lock(&self.shared.frames).remove(&sub);
    }
}

fn local_error(code: &str, message: impl Into<String>) -> Value {
    json!({"type": "error", "code": code, "message": message.into()})
}

/// The socket thread: writes what is queued, reads what arrives.
fn serve<S: Read + Write>(mut ws: WebSocket<S>, queued: &Receiver<Message>, shared: &Shared) {
    loop {
        if shared.closed.load(Ordering::Relaxed) {
            break;
        }
        loop {
            match queued.try_recv() {
                Ok(message) => {
                    if ws.send(message).is_err() {
                        shared.closed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return;
                }
            }
        }
        match ws.read() {
            Ok(Message::Text(text)) => {
                if let Ok(value) = serde_json::from_str::<Value>(text.as_str()) {
                    received(shared, value);
                }
            }
            Ok(Message::Binary(bytes)) => {
                if let Some((sub, frame)) = Frame::parse(&bytes) {
                    let mut frames = lock(&shared.frames);
                    let kept = frames.entry(sub).or_default();
                    if kept.len() >= KEPT_FRAMES {
                        kept.pop_front();
                    }
                    kept.push_back(frame);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    shared.closed.store(true, Ordering::Relaxed);
    let _ = ws.close(None);
    let _ = ws.flush();
}

fn received(shared: &Shared, value: Value) {
    if let Some(id) = value.get("id").and_then(Value::as_u64)
        && let Some(tx) = lock(&shared.replies).remove(&id)
    {
        let _ = tx.send(value);
    }
}

/// The WebSocket address and token of an attach URL; loopback only.
fn parse_url(url: &str) -> Result<(SocketAddr, String), String> {
    let usage = || {
        format!(
            "--attach takes the URL `signal ui` prints (http://127.0.0.1:<port>/#ws=<port>&token=<token>) or ws://127.0.0.1:<port>/#token=<token>, not `{url}`"
        )
    };
    let (base, fragment) = url.split_once('#').ok_or_else(usage)?;
    let field = |name: &str| {
        fragment
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_owned())
    };
    let token = field("token").ok_or_else(usage)?;
    let (scheme, rest) = base.split_once("://").ok_or_else(usage)?;
    let authority = rest.split('/').next().unwrap_or("");
    let (host, port) = authority.rsplit_once(':').ok_or_else(usage)?;
    let ip = match host {
        "127.0.0.1" | "localhost" => "127.0.0.1",
        "[::1]" => "::1",
        other => {
            return Err(format!(
                "--attach connects to this machine only (127.0.0.1, localhost or [::1]), not `{other}`"
            ));
        }
    };
    let port = match scheme {
        "ws" => port.parse::<u16>().map_err(|_| usage())?,
        "http" => field("ws")
            .and_then(|p| p.parse::<u16>().ok())
            .ok_or_else(usage)?,
        _ => return Err(usage()),
    };
    let address: SocketAddr = match ip {
        "::1" => SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
        _ => SocketAddr::from(([127, 0, 0, 1], port)),
    };
    Ok((address, token))
}

#[cfg(test)]
mod tests {
    use super::parse_url;

    #[test]
    fn attach_urls_are_loopback_only() {
        let (address, token) = parse_url("http://127.0.0.1:8800/#ws=8801&token=abc").unwrap();
        assert_eq!(address.to_string(), "127.0.0.1:8801");
        assert_eq!(token, "abc");
        let (address, _) =
            parse_url("http://127.0.0.1:8800/project/main.html#ws=9000&token=t").unwrap();
        assert_eq!(address.port(), 9000);
        let (address, token) = parse_url("ws://localhost:7000/#token=xyz").unwrap();
        assert_eq!(address.to_string(), "127.0.0.1:7000");
        assert_eq!(token, "xyz");
        let (address, _) = parse_url("ws://[::1]:7000/#token=x").unwrap();
        assert!(address.is_ipv6());
        for refused in [
            "http://192.168.1.2:8800/#ws=8801&token=abc",
            "ws://example.com:7000/#token=x",
            "http://127.0.0.1:8800/",
            "ftp://127.0.0.1:1/#token=x",
        ] {
            assert!(parse_url(refused).is_err(), "{refused}");
        }
    }
}
