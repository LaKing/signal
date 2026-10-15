//! The static files over HTTP (`docs/ui-protocol.md` "Servers"): a small
//! `std::net` server that no client can block (M2.8-8).
//!
//! An accept thread hands each connection to a bounded queue (a full queue
//! closes it at once) and a fixed pool of [`HTTP_WORKERS`] workers serves
//! them, one request per connection (`Connection: close`). A worker waits for
//! a request head at most [`DEADLINES`]`.head`, streams the file in chunks,
//! and drops a client that takes nothing for [`DEADLINES`]`.write`; every
//! wait is a poll of [`IO_POLL`] that looks at the stop flag, so dropping the
//! server returns within a few polls whatever its clients do. A stalled
//! client holds one worker, never more.

use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::bindings::MAIN_PANEL;
use crate::server::Transport;

/// The workers serving HTTP requests.
pub(crate) const HTTP_WORKERS: usize = 4;
/// Connections waiting for a worker; more are closed at once.
const HTTP_QUEUE: usize = 32;
/// How long one read or write of a connection waits, and how often a waiting
/// worker or the accept thread looks at the stop flag.
const IO_POLL: Duration = Duration::from_millis(100);
/// The largest request head.
const MAX_HEAD: usize = 8 << 10;
/// The chunk a file is streamed in.
const CHUNK: usize = 64 << 10;
/// The generated panel in the UI library, served for a project's missing
/// `ui/main.html`.
const DEFAULT_PANEL: &str = "default-panel.html";

/// How long a client may take.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Deadlines {
    /// To send its request head, from the connection's start.
    pub(crate) head: Duration,
    /// To take any byte of the response, while it is written.
    pub(crate) write: Duration,
}

/// The server's deadlines.
pub(crate) const DEADLINES: Deadlines = Deadlines {
    head: Duration::from_secs(2),
    write: Duration::from_secs(3),
};

/// A connection waiting for a worker.
pub(crate) type HttpConnection = Box<dyn Transport>;

/// The static files.
pub(crate) struct Files {
    pub(crate) ui: PathBuf,
    pub(crate) project: Arc<RwLock<PathBuf>>,
    pub(crate) port: u16,
    pub(crate) remote: bool,
}

/// Starts the workers: the queue they serve, and their threads.
pub(crate) fn workers(
    files: Files,
    stop: &Arc<AtomicBool>,
) -> io::Result<(SyncSender<HttpConnection>, Vec<JoinHandle<()>>)> {
    let (queue, waiting) = sync_channel::<HttpConnection>(HTTP_QUEUE);
    let waiting = Arc::new(Mutex::new(waiting));
    let files = Arc::new(files);
    let mut threads = Vec::with_capacity(HTTP_WORKERS);
    for n in 0..HTTP_WORKERS {
        let (waiting, files, stop) = (Arc::clone(&waiting), Arc::clone(&files), Arc::clone(stop));
        threads.push(
            thread::Builder::new()
                .name(format!("signal-ui-http-{n}"))
                .spawn(move || work(&waiting, &files, &stop))?,
        );
    }
    Ok((queue, threads))
}

/// The accept thread: each connection, blocking again (its reads and writes
/// then wait at most their timeouts), into the queue.
pub(crate) fn accept(
    listener: TcpListener,
    queue: SyncSender<HttpConnection>,
    stop: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    listener.set_nonblocking(true)?;
    thread::Builder::new()
        .name("signal-ui-http-accept".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if stream.set_nonblocking(false).is_ok() {
                            // A full queue drops (closes) the connection.
                            let _ = queue.try_send(Box::new(stream));
                        }
                    }
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
        })
}

fn work(waiting: &Mutex<Receiver<HttpConnection>>, files: &Files, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let next = waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recv_timeout(IO_POLL);
        match next {
            Ok(connection) => serve(files, connection, stop, DEADLINES),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// One connection: its request, the response, then the connection closes.
fn serve(files: &Files, mut stream: HttpConnection, stop: &AtomicBool, deadlines: Deadlines) {
    if stream.read_timeout(Some(IO_POLL)).is_err() || stream.write_timeout(Some(IO_POLL)).is_err() {
        return;
    }
    let response = match read_head(&mut stream, stop, deadlines.head) {
        Head::Complete(head) => match parse(&head) {
            Some(request) => files.respond(&request),
            None => Response::text(400, "bad request"),
        },
        Head::TooLarge => Response::text(400, "bad request"),
        Head::Gone => return,
    };
    if write_response(&mut stream, response, stop, deadlines.write).is_ok() {
        stream.finish();
    }
}

/// What came of reading a request head.
#[derive(Debug, PartialEq)]
enum Head {
    /// The head, up to its blank line.
    Complete(Vec<u8>),
    /// More than [`MAX_HEAD`] without a blank line.
    TooLarge,
    /// Closed, too slow (`deadline`), failed or stopped: nothing to answer.
    Gone,
}

fn read_head<R: Read + ?Sized>(stream: &mut R, stop: &AtomicBool, deadline: Duration) -> Head {
    let until = Instant::now() + deadline;
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        if let Some(end) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            head.truncate(end);
            return Head::Complete(head);
        }
        if head.len() > MAX_HEAD {
            return Head::TooLarge;
        }
        if stop.load(Ordering::Relaxed) || Instant::now() >= until {
            return Head::Gone;
        }
        match stream.read(&mut buffer) {
            Ok(0) => return Head::Gone,
            Ok(n) => head.extend(buffer.iter().take(n)),
            Err(e) if waits(&e) => {}
            Err(_) => return Head::Gone,
        }
    }
}

/// A request: its method, its path (query and fragment dropped) and `Host`.
#[derive(Debug, PartialEq)]
struct Request {
    method: String,
    path: String,
    host: Option<String>,
}

fn parse(head: &[u8]) -> Option<Request> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target, version) = (first.next()?, first.next()?, first.next()?);
    if first.next().is_some() || method.is_empty() || !version.starts_with("HTTP/1.") {
        return None;
    }
    let mut host = None;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("host") {
            host = Some(value.trim().to_owned());
        }
    }
    Some(Request {
        method: method.to_owned(),
        path: target.split(['?', '#']).next().unwrap_or("").to_owned(),
        host,
    })
}

/// A response: its status, its type and its body.
struct Response {
    status: u16,
    content_type: &'static str,
    body: Body,
    /// HEAD: the head only.
    head_only: bool,
}

enum Body {
    Text(&'static str),
    File(fs::File, u64),
}

impl Response {
    fn text(status: u16, text: &'static str) -> Response {
        Response {
            status,
            content_type: "text/plain; charset=utf-8",
            body: Body::Text(text),
            head_only: false,
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "",
    }
}

impl Files {
    fn respond(&self, request: &Request) -> Response {
        let head_only = match request.method.as_str() {
            "GET" => false,
            "HEAD" => true,
            _ => return Response::text(405, "method not allowed"),
        };
        // Without remote clients only the loopback names are served, so a
        // page of another site cannot read these files through a name that
        // resolves to 127.0.0.1 (DNS rebinding).
        let allowed = match request.host.as_deref().and_then(crate::server::host_port) {
            Some((host, port)) => {
                self.remote || (crate::server::loopback(host) && port == self.port)
            }
            None => false,
        };
        if !allowed {
            return Response::text(403, "forbidden host");
        }
        let path = request.path.as_str();
        let found = match path {
            "/" => file(&self.ui, "index.html"),
            _ => match (path.strip_prefix("/ui/"), path.strip_prefix("/project/")) {
                (Some(rest), _) => file(&self.ui, rest),
                // A project without `ui/main.html` gets the generated panel.
                (_, Some(rest)) => {
                    let project = self
                        .project
                        .read()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone();
                    file(&project, rest).or_else(|| {
                        (rest == MAIN_PANEL)
                            .then(|| file(&self.ui, DEFAULT_PANEL))
                            .flatten()
                    })
                }
                _ => None,
            },
        };
        let opened = found.and_then(|path| {
            let file = fs::File::open(&path).ok()?;
            let length = file.metadata().ok()?.len();
            Some((path, file, length))
        });
        match opened {
            Some((path, file, length)) => Response {
                status: 200,
                content_type: mime(&path),
                body: Body::File(file, length),
                head_only,
            },
            None => Response::text(404, "not found"),
        }
    }
}

/// Writes the response: the head, then the body streamed in [`CHUNK`]s.
fn write_response<W: Write + ?Sized>(
    stream: &mut W,
    response: Response,
    stop: &AtomicBool,
    deadline: Duration,
) -> io::Result<()> {
    let length = match &response.body {
        Body::Text(text) => text.len() as u64,
        Body::File(_, length) => *length,
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {length}\r\n\
         X-Content-Type-Options: nosniff\r\nCache-Control: no-cache\r\n\
         Referrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        response.status,
        reason(response.status),
        response.content_type,
    );
    write_by(stream, head.as_bytes(), stop, deadline)?;
    if response.head_only {
        return Ok(());
    }
    match response.body {
        Body::Text(text) => write_by(stream, text.as_bytes(), stop, deadline),
        Body::File(mut file, length) => {
            let mut chunk = vec![0u8; CHUNK];
            let mut left = length;
            while left > 0 {
                let n = file.read(&mut chunk)?;
                if n == 0 {
                    // The file shrank: the client sees a short body.
                    return Err(io::Error::from(ErrorKind::UnexpectedEof));
                }
                let n = n.min(usize::try_from(left).unwrap_or(usize::MAX));
                write_by(stream, chunk.get(..n).unwrap_or(&[]), stop, deadline)?;
                left -= n as u64;
            }
            Ok(())
        }
    }
}

/// Writes all of `bytes`, each write waiting at most the stream's timeout;
/// fails once the stream has taken nothing for `deadline`, or on stop.
fn write_by<W: Write + ?Sized>(
    stream: &mut W,
    mut bytes: &[u8],
    stop: &AtomicBool,
    deadline: Duration,
) -> io::Result<()> {
    let mut progress = Instant::now();
    while !bytes.is_empty() {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::from(ErrorKind::Interrupted));
        }
        match stream.write(bytes) {
            Ok(0) => return Err(io::Error::from(ErrorKind::WriteZero)),
            Ok(n) => {
                bytes = bytes.get(n..).unwrap_or(&[]);
                progress = Instant::now();
            }
            Err(e) if waits(&e) => {
                if progress.elapsed() >= deadline {
                    return Err(io::Error::from(ErrorKind::TimedOut));
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// A read or write that waited out its timeout (or was interrupted).
fn waits(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
    )
}

/// `relative` inside `root`, an existing file: no `..`, no hidden or empty
/// segment, no escape through a link.
pub(crate) fn file(root: &Path, relative: &str) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_head_is_parsed_with_its_host_in_any_case() {
        let request = parse(b"GET /ui/a.js?x=1 HTTP/1.1\r\nhOsT: 127.0.0.1:80\r\nX: y").unwrap();
        assert_eq!(
            request,
            Request {
                method: "GET".to_owned(),
                path: "/ui/a.js".to_owned(),
                host: Some("127.0.0.1:80".to_owned()),
            }
        );
        assert_eq!(parse(b"GET / HTTP/1.0").unwrap().host, None);
        for bad in [
            &b"GET /"[..],
            b"GET / SPDY/3",
            b"GET / HTTP/1.1 extra",
            b"GET / HTTP/1.1\r\nno colon",
            b"\xff / HTTP/1.1",
        ] {
            assert_eq!(parse(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn a_head_ends_at_its_blank_line_and_is_bounded() {
        let stop = AtomicBool::new(false);
        let mut stream = Cursor::new(b"GET / HTTP/1.1\r\nHost: h\r\n\r\nbody".to_vec());
        assert_eq!(
            read_head(&mut stream, &stop, Duration::from_secs(1)),
            Head::Complete(b"GET / HTTP/1.1\r\nHost: h".to_vec())
        );
        let mut long = Cursor::new(vec![b'a'; MAX_HEAD + 2048]);
        assert_eq!(
            read_head(&mut long, &stop, Duration::from_secs(1)),
            Head::TooLarge
        );
        let mut short = Cursor::new(b"GET / HTTP/1.1\r\n".to_vec());
        assert_eq!(
            read_head(&mut short, &stop, Duration::from_secs(1)),
            Head::Gone,
            "closed before the blank line"
        );
    }

    /// A client that never sends (or never takes) anything: every read and
    /// write waits out a short timeout.
    struct Silent;

    impl Read for Silent {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            thread::sleep(Duration::from_millis(5));
            Err(io::Error::from(ErrorKind::WouldBlock))
        }
    }

    impl Write for Silent {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            thread::sleep(Duration::from_millis(5));
            Err(io::Error::from(ErrorKind::WouldBlock))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_client_that_takes_nothing_is_dropped_at_the_deadline() {
        let stop = AtomicBool::new(false);
        let began = Instant::now();
        let error = write_by(&mut Silent, b"data", &stop, Duration::from_millis(50)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        let took = began.elapsed();
        assert!(
            took >= Duration::from_millis(50) && took < Duration::from_secs(1),
            "{took:?}"
        );
        let began = Instant::now();
        assert_eq!(
            read_head(&mut Silent, &stop, Duration::from_millis(50)),
            Head::Gone
        );
        assert!(began.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_stop_ends_a_stalled_write_at_once() {
        let stop = AtomicBool::new(true);
        let began = Instant::now();
        let error = write_by(&mut Silent, b"data", &stop, Duration::from_secs(60)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Interrupted);
        assert_eq!(
            read_head(&mut Silent, &stop, Duration::from_secs(60)),
            Head::Gone
        );
        assert!(began.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn a_file_is_streamed_whole_after_its_head() {
        let base = std::env::temp_dir().join(format!("signal-ui-http-unit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("ui")).unwrap();
        let bytes: Vec<u8> = (0..(3 * CHUNK + 17)).map(|i| (i % 251) as u8).collect();
        fs::write(base.join("ui/big.bin"), &bytes).unwrap();
        let files = Files {
            ui: base.join("ui"),
            project: Arc::new(RwLock::new(base.join("project-ui"))),
            port: 8,
            remote: false,
        };
        let request = parse(b"GET /ui/big.bin HTTP/1.1\r\nHost: localhost:8").unwrap();
        let mut out = Vec::new();
        let stop = AtomicBool::new(false);
        write_response(&mut out, files.respond(&request), &stop, DEADLINES.write).unwrap();
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&out[..split]).to_string();
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
        assert!(head.contains(&format!("Content-Length: {}", bytes.len())));
        assert!(head.contains("Content-Type: application/octet-stream"));
        assert!(head.contains("Connection: close"));
        assert_eq!(&out[split + 4..], &bytes[..]);

        let head_only = parse(b"HEAD /ui/big.bin HTTP/1.1\r\nHost: localhost:8").unwrap();
        let mut out = Vec::new();
        write_response(&mut out, files.respond(&head_only), &stop, DEADLINES.write).unwrap();
        assert!(out.ends_with(b"\r\n\r\n"), "no body for HEAD");
        let post = parse(b"POST / HTTP/1.1\r\nHost: localhost:8").unwrap();
        assert_eq!(files.respond(&post).status, 405);
        let other = parse(b"GET / HTTP/1.1\r\nHost: example.com:8").unwrap();
        assert_eq!(files.respond(&other).status, 403);
        fs::remove_dir_all(&base).unwrap();
    }
}
