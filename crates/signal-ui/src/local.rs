//! In process (the `in-process` feature: `signal mcp` and the tests): an
//! in-process duplex byte stream, so that the protocol's
//! tests run the server's WebSocket connection code (handshake, Origin,
//! `hello`, the bounded writer, the reads) where no port can be bound. Each
//! direction holds at most [`CAPACITY`] bytes, as a socket's buffer does: a
//! write waits for room up to its timeout, a read for data up to its
//! timeout; dropping one end closes both directions for the other.

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::server::Transport;

/// Bytes one direction holds.
const CAPACITY: usize = 256 << 10;

#[derive(Default)]
struct State {
    bytes: VecDeque<u8>,
    closed: bool,
}

#[derive(Default)]
struct Half {
    state: Mutex<State>,
    changed: Condvar,
}

impl Half {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }
}

/// One end of an in-process connection.
pub struct LocalStream {
    incoming: Arc<Half>,
    outgoing: Arc<Half>,
    /// Timeouts in microseconds; 0 waits without end.
    read_timeout: AtomicU64,
    write_timeout: AtomicU64,
}

/// Both ends of a new connection.
pub(crate) fn pair() -> (LocalStream, LocalStream) {
    let (a, b) = (Arc::new(Half::default()), Arc::new(Half::default()));
    let end = |incoming: &Arc<Half>, outgoing: &Arc<Half>| LocalStream {
        incoming: Arc::clone(incoming),
        outgoing: Arc::clone(outgoing),
        read_timeout: AtomicU64::new(0),
        write_timeout: AtomicU64::new(0),
    };
    (end(&a, &b), end(&b, &a))
}

fn micros(timeout: Option<Duration>) -> u64 {
    timeout.map_or(0, |t| {
        u64::try_from(t.as_micros()).unwrap_or(u64::MAX).max(1)
    })
}

/// Waits on `half` until `ready`, up to `timeout` microseconds (0: forever).
fn wait<'a>(
    half: &'a Half,
    mut state: MutexGuard<'a, State>,
    timeout: u64,
    ready: impl Fn(&State) -> bool,
) -> io::Result<MutexGuard<'a, State>> {
    let until = (timeout > 0).then(|| Instant::now() + Duration::from_micros(timeout));
    while !ready(&state) {
        state = match until {
            None => half.changed.wait(state).unwrap_or_else(|e| e.into_inner()),
            Some(until) => {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::Error::from(ErrorKind::WouldBlock));
                }
                half.changed
                    .wait_timeout(state, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
        };
    }
    Ok(state)
}

impl LocalStream {
    /// How long a read waits (`None`: without end).
    pub fn set_read_timeout(&self, timeout: Option<Duration>) {
        self.read_timeout.store(micros(timeout), Ordering::Relaxed);
    }

    /// How long a write waits for room (`None`: without end).
    pub fn set_write_timeout(&self, timeout: Option<Duration>) {
        self.write_timeout.store(micros(timeout), Ordering::Relaxed);
    }
}

impl Read for LocalStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = self.read_timeout.load(Ordering::Relaxed);
        let state = self.incoming.lock();
        let mut state = wait(&self.incoming, state, timeout, |s| {
            !s.bytes.is_empty() || s.closed
        })?;
        let n = buf.len().min(state.bytes.len());
        for (to, from) in buf.iter_mut().zip(state.bytes.drain(..n)) {
            *to = from;
        }
        drop(state);
        self.incoming.changed.notify_all();
        // Empty and closed: the end of the stream.
        Ok(n)
    }
}

impl Write for LocalStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = self.write_timeout.load(Ordering::Relaxed);
        let state = self.outgoing.lock();
        let mut state = wait(&self.outgoing, state, timeout, |s| {
            s.closed || s.bytes.len() < CAPACITY
        })?;
        if state.closed {
            return Err(io::Error::from(ErrorKind::BrokenPipe));
        }
        let n = buf.len().min(CAPACITY - state.bytes.len());
        state.bytes.extend(buf.iter().take(n));
        drop(state);
        self.outgoing.changed.notify_all();
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LocalStream {
    fn drop(&mut self) {
        self.incoming.close();
        self.outgoing.close();
    }
}

impl Transport for LocalStream {
    fn read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(timeout);
        Ok(())
    }
    fn write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_write_timeout(timeout);
        Ok(())
    }
}
