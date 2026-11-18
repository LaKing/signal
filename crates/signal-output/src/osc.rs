//! OSC control input (ADR 0057 §1): a control slot's values from Open Sound
//! Control messages over UDP.
//!
//! A listener thread receives datagrams on the bound address, reads each
//! message (or each message of a bundle, in order; the bundle's time tag is
//! ignored: a network message has no sample time), and for an address
//! `/<role>/<name>` whose role and name a control side has, with one
//! numeric argument (`f`, `i`, `d`, `h`, or `T`/`F` as 1/0), pushes the
//! value through a lock-free ring to that side's [`OscSource`]. The source,
//! an [`InputSource`] the audio thread reads, applies what arrived at the
//! start of each block and holds every value until the next: a capture side
//! like any other, so its words are recorded and a replay is exact.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::inputs::InputSource;

/// One OSC message's address and its first numeric argument.
#[derive(Clone, Debug, PartialEq)]
pub struct OscValue {
    /// The address pattern, as sent (`/surface/cutoff`).
    pub address: String,
    /// Its value.
    pub value: f64,
}

/// Reads the messages of datagram `packet` (a message or a bundle, bundles
/// nested), each with its first numeric argument; malformed parts and
/// messages without one are skipped.
pub fn parse_osc(packet: &[u8]) -> Vec<OscValue> {
    let mut out = Vec::new();
    read_packet(packet, &mut out, 0);
    out
}

/// The most nested bundles read.
const MAX_DEPTH: usize = 8;

fn read_packet(packet: &[u8], out: &mut Vec<OscValue>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    if packet.starts_with(b"#bundle\0") {
        // `#bundle`, an 8-byte time tag, then elements of a size and a body.
        let mut at = 16;
        while let Some(size) = packet
            .get(at..at + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
        {
            let size = u32::from_be_bytes(size) as usize;
            let Some(element) = packet.get(at + 4..at + 4 + size) else {
                return;
            };
            read_packet(element, out, depth + 1);
            at += 4 + size;
        }
        return;
    }
    if let Some(value) = read_message(packet) {
        out.push(value);
    }
}

/// A padded OSC string at `at`, and where the next field starts.
fn osc_string(packet: &[u8], at: usize) -> Option<(&str, usize)> {
    let rest = packet.get(at..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    let text = std::str::from_utf8(rest.get(..end)?).ok()?;
    Some((text, at + (end + 4) / 4 * 4))
}

fn read_message(packet: &[u8]) -> Option<OscValue> {
    let (address, at) = osc_string(packet, 0)?;
    if !address.starts_with('/') {
        return None;
    }
    let (tags, mut at) = osc_string(packet, at)?;
    let tags = tags.strip_prefix(',')?;
    for tag in tags.chars() {
        let value = match tag {
            'f' => Some(f64::from(f32::from_be_bytes(
                packet.get(at..at + 4)?.try_into().ok()?,
            ))),
            'i' => Some(f64::from(i32::from_be_bytes(
                packet.get(at..at + 4)?.try_into().ok()?,
            ))),
            'd' => Some(f64::from_be_bytes(packet.get(at..at + 8)?.try_into().ok()?)),
            'h' => Some(i64::from_be_bytes(packet.get(at..at + 8)?.try_into().ok()?) as f64),
            'T' => Some(1.0),
            'F' => Some(0.0),
            _ => None,
        };
        if let Some(value) = value.filter(|v| v.is_finite()) {
            return Some(OscValue {
                address: address.to_owned(),
                value,
            });
        }
        // Skip a non-numeric argument by its size.
        at += match tag {
            'f' | 'i' | 'c' | 'r' | 'm' => 4,
            'd' | 'h' | 't' => 8,
            's' | 'S' => osc_string(packet, at)?.1 - at,
            'b' => {
                let size = u32::from_be_bytes(packet.get(at..at + 4)?.try_into().ok()?) as usize;
                4 + size.div_ceil(4) * 4
            }
            _ => 0,
        };
    }
    None
}

/// An OSC message as a client sends it: `address`, one float argument.
pub fn osc_message(address: &str, value: f32) -> Vec<u8> {
    let mut packet = Vec::new();
    let pad = |packet: &mut Vec<u8>, text: &[u8]| {
        packet.extend_from_slice(text);
        packet.push(0);
        while !packet.len().is_multiple_of(4) {
            packet.push(0);
        }
    };
    pad(&mut packet, address.as_bytes());
    pad(&mut packet, b",f");
    packet.extend_from_slice(&value.to_be_bytes());
    packet
}

/// The audio thread's end of a control side: the values that arrived,
/// applied at the start of each read and held.
pub struct OscSource {
    rx: Consumer<(u16, f32)>,
    values: Vec<f32>,
}

impl InputSource for OscSource {
    fn read(&mut self, out: &mut [f32], _offset: usize) -> usize {
        while let Ok((index, value)) = self.rx.pop() {
            if let Some(slot) = self.values.get_mut(usize::from(index)) {
                *slot = value;
            }
        }
        let width = self.values.len().max(1);
        for frame in out.chunks_exact_mut(width) {
            frame.copy_from_slice(&self.values);
        }
        out.len() / width
    }
}

/// A control side as the listener routes it: its role and value names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OscRoute {
    /// The role, the address's first part.
    pub role: String,
    /// The value names, the second part, in output order.
    pub names: Vec<String>,
}

/// The listener thread: stops when dropped.
pub struct OscListener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Messages received that reached a side.
    pub received: Arc<AtomicU64>,
    /// The address it listens on.
    pub local: SocketAddr,
}

impl Drop for OscListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What [`listen_each`] opened for one binding: its source and the address
/// it listens on, or why its address could not be opened.
pub type Opened = Result<(OscSource, SocketAddr), String>;

/// Listens for each of `bindings` (an address and its route) with one
/// socket per address, so the roles bound to the same address share it
/// (each a route of its listener): the listeners, and what each binding
/// got, in order.
pub fn listen_each(bindings: &[(String, OscRoute)]) -> (Vec<OscListener>, Vec<Opened>) {
    let mut opened: Vec<Option<Opened>> = bindings.iter().map(|_| None).collect();
    let mut listeners = Vec::new();
    let mut addresses: Vec<&str> = bindings.iter().map(|(a, _)| a.as_str()).collect();
    addresses.sort_unstable();
    addresses.dedup();
    for address in addresses {
        let members: Vec<usize> = bindings
            .iter()
            .enumerate()
            .filter(|(_, (a, _))| a == address)
            .map(|(i, _)| i)
            .collect();
        let routes: Vec<OscRoute> = bindings
            .iter()
            .filter(|(a, _)| a == address)
            .map(|(_, r)| r.clone())
            .collect();
        match listen(address, &routes) {
            Ok((listener, sources)) => {
                for (i, source) in members.into_iter().zip(sources) {
                    if let Some(slot) = opened.get_mut(i) {
                        *slot = Some(Ok((source, listener.local)));
                    }
                }
                listeners.push(listener);
            }
            Err(error) => {
                for i in members {
                    if let Some(slot) = opened.get_mut(i) {
                        *slot = Some(Err(error.to_string()));
                    }
                }
            }
        }
    }
    let opened = opened
        .into_iter()
        .map(|o| o.unwrap_or_else(|| Err("not opened".to_owned())))
        .collect();
    (listeners, opened)
}

/// Listens on `address` for `routes`' messages: the listener, and one
/// [`OscSource`] per route, in order.
pub fn listen(address: &str, routes: &[OscRoute]) -> io::Result<(OscListener, Vec<OscSource>)> {
    let socket = UdpSocket::bind(address)?;
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    let local = socket.local_addr()?;
    let mut producers: Vec<Producer<(u16, f32)>> = Vec::new();
    let mut sources = Vec::new();
    for route in routes {
        let (tx, rx) = RingBuffer::new(1024);
        producers.push(tx);
        sources.push(OscSource {
            rx,
            values: vec![0.0; route.names.len()],
        });
    }
    let stop = Arc::new(AtomicBool::new(false));
    let received = Arc::new(AtomicU64::new(0));
    let routes = routes.to_vec();
    let thread = {
        let (stop, received) = (Arc::clone(&stop), Arc::clone(&received));
        thread::Builder::new()
            .name("signal-osc".to_owned())
            .spawn(move || {
                let mut buffer = [0u8; 65_536];
                while !stop.load(Ordering::Relaxed) {
                    let Ok(n) = socket.recv(&mut buffer) else {
                        continue;
                    };
                    for message in parse_osc(buffer.get(..n).unwrap_or_default()) {
                        let mut parts = message.address.trim_start_matches('/').splitn(2, '/');
                        let (Some(role), Some(name)) = (parts.next(), parts.next()) else {
                            continue;
                        };
                        for (route, tx) in routes.iter().zip(producers.iter_mut()) {
                            if route.role != role {
                                continue;
                            }
                            if let Some(index) = route.names.iter().position(|n| n == name)
                                && tx.push((index as u16, message.value as f32)).is_ok()
                            {
                                received.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            })?
    };
    Ok((
        OscListener {
            stop,
            thread: Some(thread),
            received,
            local,
        },
        sources,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_reads_its_address_and_value() {
        let packet = osc_message("/surface/cutoff", 0.25);
        assert_eq!(packet.len() % 4, 0);
        assert_eq!(
            parse_osc(&packet),
            [OscValue {
                address: "/surface/cutoff".to_owned(),
                value: 0.25
            }]
        );
    }

    #[test]
    fn ints_doubles_booleans_and_bundles_read_too() {
        let mut int = b"/a/x\0\0\0\0,i\0\0".to_vec();
        int.extend_from_slice(&7i32.to_be_bytes());
        assert_eq!(parse_osc(&int)[0].value, 7.0);
        let mut double = b"/a/y\0\0\0\0,sd\0".to_vec();
        double.extend_from_slice(b"hi\0\0");
        double.extend_from_slice(&0.5f64.to_be_bytes());
        assert_eq!(parse_osc(&double)[0].value, 0.5, "a string skipped");
        assert_eq!(parse_osc(b"/a/z\0\0\0\0,T\0\0")[0].value, 1.0);
        let mut bundle = b"#bundle\0\0\0\0\0\0\0\0\x01".to_vec();
        for message in [osc_message("/a/x", 1.0), osc_message("/a/y", 2.0)] {
            bundle.extend_from_slice(&(message.len() as u32).to_be_bytes());
            bundle.extend_from_slice(&message);
        }
        let values: Vec<f64> = parse_osc(&bundle).iter().map(|v| v.value).collect();
        assert_eq!(values, [1.0, 2.0]);
        assert!(parse_osc(b"garbage").is_empty());
        assert!(
            parse_osc(b"/a/s\0\0\0\0,s\0\0hi\0\0").is_empty(),
            "no number"
        );
    }

    #[test]
    fn a_source_holds_what_arrived_until_the_next() {
        let (mut tx, rx) = RingBuffer::new(8);
        let mut source = OscSource {
            rx,
            values: vec![0.0; 2],
        };
        let mut out = [9.0f32; 6];
        assert_eq!(source.read(&mut out, 0), 3);
        assert_eq!(out, [0.0; 6]);
        tx.push((1, 0.5)).unwrap();
        source.read(&mut out, 0);
        assert_eq!(out, [0.0, 0.5, 0.0, 0.5, 0.0, 0.5]);
        source.read(&mut out, 0);
        assert_eq!(out, [0.0, 0.5, 0.0, 0.5, 0.0, 0.5], "held");
    }

    #[test]
    fn a_listener_routes_role_and_name_to_its_source() {
        let routes = [OscRoute {
            role: "surface".to_owned(),
            names: vec!["cutoff".to_owned(), "x".to_owned()],
        }];
        let (listener, mut sources) = listen("127.0.0.1:0", &routes).unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        for (address, value) in [
            ("/surface/x", 0.75f32),
            ("/other/x", 0.1),
            ("/surface/y", 0.2),
        ] {
            client
                .send_to(&osc_message(address, value), listener.local)
                .unwrap();
        }
        let mut out = [0.0f32; 2];
        for _ in 0..100 {
            if listener.received.load(Ordering::Relaxed) >= 1 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        sources[0].read(&mut out, 0);
        assert_eq!(out, [0.0, 0.75]);
        assert_eq!(listener.received.load(Ordering::Relaxed), 1);
    }

    /// Two roles bound to one address share its socket (a second bind of
    /// it would fail), each reading its own messages.
    #[test]
    fn roles_bound_to_one_address_share_its_listener() {
        let route = |role: &str| OscRoute {
            role: role.to_owned(),
            names: vec!["x".to_owned()],
        };
        let bindings = [
            ("127.0.0.1:0".to_owned(), route("a")),
            ("127.0.0.1:0".to_owned(), route("b")),
        ];
        let (listeners, opened) = listen_each(&bindings);
        assert_eq!(listeners.len(), 1);
        let mut sources: Vec<OscSource> = opened.into_iter().map(|o| o.unwrap().0).collect();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        for (address, value) in [("/a/x", 0.25f32), ("/b/x", 0.5)] {
            client
                .send_to(&osc_message(address, value), listeners[0].local)
                .unwrap();
        }
        for _ in 0..100 {
            if listeners[0].received.load(Ordering::Relaxed) >= 2 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut out = [0.0f32; 1];
        sources[0].read(&mut out, 0);
        assert_eq!(out, [0.25]);
        sources[1].read(&mut out, 0);
        assert_eq!(out, [0.5]);
    }
}
