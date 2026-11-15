//! MIDI input from CoreMIDI (ADR 0057 §3): the fifth module allowed
//! `unsafe` code (AGENTS.md rule 9, ADR 0017's rules), the only place
//! Signal calls CoreMIDI.
//!
//! It creates a client and one input port, connects the system's MIDI
//! sources whose names contain the binding's text, and copies every
//! received packet's bytes, in order, into a lock-free ring
//! ([`rtrb`]); reading them (running status, notes, controllers, clock) is
//! safe Rust on the control thread (`signal_output::midi`). For tests it
//! can also make a virtual source in this process and send through it.
//!
//! Its calls, the only ones it makes outside safe Rust: `MIDIClientCreate`,
//! `MIDIInputPortCreate`, `MIDIGetNumberOfSources`, `MIDIGetSource`,
//! `MIDIObjectGetStringProperty`, `MIDIPortConnectSource`,
//! `MIDIPortDispose`, `MIDIClientDispose`, `MIDISourceCreate`,
//! `MIDIEndpointDispose`, `MIDIReceived` (CoreMIDI) and
//! `CFStringCreateWithCString`, `CFStringGetCString`, `CFRelease`
//! (CoreFoundation), declared here against the macOS SDK's headers.
//!
//! The audit:
//! - **Every CoreMIDI object** is a `u32` reference CoreMIDI owns; this
//!   module disposes of what it created (the port, then the client, then
//!   a virtual source) in `Drop`, after which CoreMIDI calls no callback.
//! - **The read callback** runs on CoreMIDI's thread, one call at a time
//!   for this port. Its `refcon` is a pointer to the port's [`Feed`], a box
//!   this module allocated and frees only after `MIDIPortDispose`; the
//!   callback takes `&mut` of it, which nothing else touches while the
//!   port lives. It walks the packet list by CoreMIDI's layout (packed to 4
//!   bytes: a `u32` count, then each packet's `u64` time stamp, `u16`
//!   length and bytes; on arm64 the next packet starts at the next multiple
//!   of 4), reading every field unaligned, and never past `length` (at most
//!   256 bytes a packet, as the SDK declares). It allocates nothing and
//!   never blocks: a full ring drops bytes and counts them.
//! - **CoreFoundation strings** made here are released here; a string
//!   CoreMIDI returns from `MIDIObjectGetStringProperty` is owned by the
//!   caller (the "Get" of a CF property that copies, per the header's
//!   doc) and released after it is read into a Rust `String`.

#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_void};
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

type OsStatus = i32;
type MidiRef = u32;
type CfStringRef = *const c_void;

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;

/// The bytes a ring holds: a second of a dense MIDI stream, several times.
const RING_BYTES: usize = 16 * 1024;

// SAFETY: declared as the macOS SDK's CoreMIDI headers declare them (the
// references `u32`, `ItemCount` a `usize`, the read proc a C function).
#[link(name = "CoreMIDI", kind = "framework")]
unsafe extern "C" {
    fn MIDIClientCreate(
        name: CfStringRef,
        notify: *const c_void,
        notify_refcon: *mut c_void,
        out: *mut MidiRef,
    ) -> OsStatus;
    fn MIDIInputPortCreate(
        client: MidiRef,
        name: CfStringRef,
        read: extern "C" fn(*const u8, *mut c_void, *mut c_void),
        refcon: *mut c_void,
        out: *mut MidiRef,
    ) -> OsStatus;
    fn MIDIGetNumberOfSources() -> usize;
    fn MIDIGetSource(index: usize) -> MidiRef;
    fn MIDIObjectGetStringProperty(
        object: MidiRef,
        property: CfStringRef,
        out: *mut CfStringRef,
    ) -> OsStatus;
    fn MIDIPortConnectSource(port: MidiRef, source: MidiRef, refcon: *mut c_void) -> OsStatus;
    fn MIDIPortDispose(port: MidiRef) -> OsStatus;
    fn MIDIClientDispose(client: MidiRef) -> OsStatus;
    fn MIDISourceCreate(client: MidiRef, name: CfStringRef, out: *mut MidiRef) -> OsStatus;
    fn MIDIEndpointDispose(endpoint: MidiRef) -> OsStatus;
    fn MIDIReceived(source: MidiRef, packets: *const u8) -> OsStatus;
    static kMIDIPropertyName: CfStringRef;
}

// SAFETY: declared as the SDK's CoreFoundation headers declare them.
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        text: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFStringGetCString(
        string: CfStringRef,
        buffer: *mut c_char,
        size: isize,
        encoding: u32,
    ) -> u8;
    fn CFRelease(object: *const c_void);
}

/// A CoreFoundation string made from `text`, released when dropped.
struct CfString(CfStringRef);

impl CfString {
    fn new(text: &str) -> Option<CfString> {
        let text = CString::new(text).ok()?;
        // SAFETY: `text` is a NUL-terminated buffer alive for the call; the
        // default allocator (null); the result is owned here or null.
        let string = unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
        (!string.is_null()).then_some(CfString(string))
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        // SAFETY: created by `CFStringCreateWithCString` above, released once.
        unsafe { CFRelease(self.0) };
    }
}

/// A source's display name, if CoreMIDI gives one.
fn name_of(object: MidiRef) -> Option<String> {
    let mut string: CfStringRef = std::ptr::null();
    // SAFETY: `string` is a live local CoreMIDI writes one reference into;
    // the property key is CoreMIDI's own constant.
    let status = unsafe { MIDIObjectGetStringProperty(object, kMIDIPropertyName, &mut string) };
    if status != 0 || string.is_null() {
        return None;
    }
    let mut buffer = [0 as c_char; 256];
    // SAFETY: `buffer` holds 256 bytes, the size passed; `string` is the
    // live reference just returned.
    let ok = unsafe { CFStringGetCString(string, buffer.as_mut_ptr(), 256, UTF8) };
    // SAFETY: the caller owns a string a CoreMIDI property get returns.
    unsafe { CFRelease(string) };
    if ok == 0 {
        return None;
    }
    let bytes: Vec<u8> = buffer
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8(bytes).ok()
}

/// What the read callback writes into, behind the port's `refcon`.
struct Feed {
    tx: Producer<u8>,
    dropped: Arc<AtomicU64>,
}

/// CoreMIDI's read callback: the packets' bytes into the ring.
extern "C" fn read(packets: *const u8, refcon: *mut c_void, _source: *mut c_void) {
    if packets.is_null() || refcon.is_null() {
        return;
    }
    // SAFETY: `refcon` is the `Feed` box this port was created with, alive
    // until after `MIDIPortDispose`, touched by this one callback at a time.
    let feed = unsafe { &mut *refcon.cast::<Feed>() };
    // SAFETY: CoreMIDI hands a valid packet list for the call: a `u32`
    // count at its start, read unaligned.
    let count = unsafe { packets.cast::<u32>().read_unaligned() };
    let mut at = 4usize;
    for _ in 0..count {
        // SAFETY: each packet lies within the list CoreMIDI passed: its
        // time stamp (8 bytes), its length (2), then `length` bytes (at
        // most 256), all read unaligned.
        let length = unsafe { packets.add(at + 8).cast::<u16>().read_unaligned() };
        let length = usize::from(length).min(256);
        for i in 0..length {
            // SAFETY: as above, `i < length`.
            let byte = unsafe { packets.add(at + 10 + i).read() };
            if feed.tx.push(byte).is_err() {
                feed.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        // arm64: the next packet at the next multiple of 4.
        at = (at + 10 + length + 3) & !3;
    }
}

/// A connected MIDI input: its bytes arrive in [`MidiInput::take_bytes`]'s
/// ring until it is dropped.
pub struct MidiInput {
    client: MidiRef,
    port: MidiRef,
    virtual_source: Option<MidiRef>,
    feed: *mut Feed,
    rx: Option<Consumer<u8>>,
    /// The sources it connected, by name.
    pub sources: Vec<String>,
    /// Bytes the full ring dropped.
    pub dropped: Arc<AtomicU64>,
}

// SAFETY: the raw `feed` pointer is only freed in `Drop`, after CoreMIDI
// stops calling back; the struct is moved between threads whole.
unsafe impl Send for MidiInput {}

impl MidiInput {
    /// Opens a client and an input port and connects every MIDI source
    /// whose name contains `filter` (all of them for `*`); the reason when
    /// CoreMIDI refuses.
    pub fn open(filter: &str) -> Result<MidiInput, String> {
        let name = CfString::new("Signal").ok_or("no client name")?;
        let mut client: MidiRef = 0;
        // SAFETY: `name` is a live CF string; no notify proc (null); the
        // client reference is written into a live local.
        let status = unsafe { MIDIClientCreate(name.0, std::ptr::null(), null_mut(), &mut client) };
        if status != 0 {
            return Err(format!("CoreMIDI refused a client (OSStatus {status})"));
        }
        let (tx, rx) = RingBuffer::new(RING_BYTES);
        let dropped = Arc::new(AtomicU64::new(0));
        let feed = Box::into_raw(Box::new(Feed {
            tx,
            dropped: Arc::clone(&dropped),
        }));
        let port_name = CfString::new("Signal input").ok_or("no port name")?;
        let mut port: MidiRef = 0;
        let refcon = feed.cast();
        // SAFETY: `feed` is the box above, freed only in `Drop` after the
        // port is disposed; `read` matches CoreMIDI's MIDIReadProc.
        let status = unsafe { MIDIInputPortCreate(client, port_name.0, read, refcon, &mut port) };
        let mut input = MidiInput {
            client,
            port,
            virtual_source: None,
            feed,
            rx: Some(rx),
            sources: Vec::new(),
            dropped,
        };
        if status != 0 {
            return Err(format!(
                "CoreMIDI refused an input port (OSStatus {status})"
            ));
        }
        input.connect(filter);
        Ok(input)
    }

    /// Connects the sources whose names contain `filter` (`*`: all).
    fn connect(&mut self, filter: &str) {
        // SAFETY: a plain query.
        let count = unsafe { MIDIGetNumberOfSources() };
        for index in 0..count {
            // SAFETY: `index` is below the count just read.
            let source = unsafe { MIDIGetSource(index) };
            if source == 0 {
                continue;
            }
            let name = name_of(source).unwrap_or_default();
            if filter != "*" && !name.contains(filter) {
                continue;
            }
            // SAFETY: `port` and `source` are live references.
            if unsafe { MIDIPortConnectSource(self.port, source, null_mut()) } == 0 {
                self.sources.push(name);
            }
        }
    }

    /// The ring's reading end (once).
    pub fn take_bytes(&mut self) -> Option<Consumer<u8>> {
        self.rx.take()
    }

    /// Makes a virtual source named `name` in this client and connects the
    /// port to it: a test's way in, without a device.
    pub fn connect_virtual(&mut self, name: &str) -> Result<(), String> {
        let cf = CfString::new(name).ok_or("no source name")?;
        let mut source: MidiRef = 0;
        // SAFETY: live client and string; the reference into a live local.
        let status = unsafe { MIDISourceCreate(self.client, cf.0, &mut source) };
        if status != 0 {
            return Err(format!(
                "CoreMIDI refused a virtual source (OSStatus {status})"
            ));
        }
        self.virtual_source = Some(source);
        // SAFETY: live port and source.
        let status = unsafe { MIDIPortConnectSource(self.port, source, null_mut()) };
        if status != 0 {
            return Err(format!(
                "CoreMIDI refused the connection (OSStatus {status})"
            ));
        }
        self.sources.push(name.to_owned());
        Ok(())
    }

    /// Sends `bytes` as one packet from the virtual source (tests).
    pub fn send_virtual(&self, bytes: &[u8]) -> Result<(), String> {
        let source = self.virtual_source.ok_or("no virtual source")?;
        let length = bytes.len().min(256);
        // A packet list of one packet, laid out as CoreMIDI reads it.
        let mut list = Vec::with_capacity(4 + 8 + 2 + 256);
        list.extend_from_slice(&1u32.to_ne_bytes());
        list.extend_from_slice(&0u64.to_ne_bytes());
        list.extend_from_slice(&(length as u16).to_ne_bytes());
        list.extend(bytes.iter().take(length));
        list.resize(4 + 8 + 2 + 256, 0);
        // SAFETY: `list` is a packet list of one packet in CoreMIDI's
        // layout, alive for the call.
        let status = unsafe { MIDIReceived(source, list.as_ptr()) };
        if status != 0 {
            return Err(format!("CoreMIDI refused the packet (OSStatus {status})"));
        }
        Ok(())
    }
}

impl Drop for MidiInput {
    fn drop(&mut self) {
        // SAFETY: references created here, disposed once: after the port is
        // gone CoreMIDI calls `read` no more, so the feed can be freed.
        unsafe {
            MIDIPortDispose(self.port);
            if let Some(source) = self.virtual_source.take() {
                MIDIEndpointDispose(source);
            }
            MIDIClientDispose(self.client);
            drop(Box::from_raw(self.feed));
        }
    }
}

/// A `midi` slot fed from CoreMIDI (ADR 0057 §1, §3): a thread reads the
/// input's bytes into a [`MidiState`](signal_output::midi::MidiState) and
/// hands each value that changed to the slot's
/// [`MidiSource`](signal_output::midi::MidiSource). Stops when dropped.
pub struct MidiFeed {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<MidiInput>>,
    /// The sources it connected, by name.
    pub sources: Vec<String>,
}

impl MidiFeed {
    /// Connects the MIDI sources whose names contain `filter` (`*`: all) for
    /// a slot capturing `names`: the feed and the slot's source.
    pub fn start(
        filter: &str,
        names: &[String],
    ) -> Result<(MidiFeed, signal_output::midi::MidiSource), String> {
        let mut input = MidiInput::open(filter)?;
        let mut bytes = input.take_bytes().ok_or("no ring")?;
        let (mut tx, rx) = RingBuffer::new(4096);
        let source = signal_output::midi::MidiSource::new(rx, names.len());
        let mut state = signal_output::midi::MidiState::new(names);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sources = input.sources.clone();
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("signal-midi".to_owned())
                .spawn(move || {
                    let mut last = state.values.clone();
                    while !stop.load(Ordering::Relaxed) {
                        let now = std::time::Instant::now();
                        let mut any = false;
                        while let Ok(byte) = bytes.pop() {
                            state.byte(byte, now);
                            any = true;
                        }
                        if any {
                            for (i, (value, before)) in
                                state.values.iter().zip(last.iter_mut()).enumerate()
                            {
                                if value.to_bits() != before.to_bits() {
                                    *before = *value;
                                    let _ = tx.push((i as u16, *value));
                                }
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    input
                })
                .map_err(|e| e.to_string())?
        };
        Ok((
            MidiFeed {
                stop,
                thread: Some(thread),
                sources,
            },
            source,
        ))
    }
}

impl Drop for MidiFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // The input is dropped here, on this thread: CoreMIDI disposed.
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// ADR 0057 phase 3, without a device: bytes sent through a virtual
    /// source in this process arrive in the ring, in order.
    #[test]
    fn bytes_from_a_source_arrive_in_order() {
        let mut input = match MidiInput::open("no such source") {
            Ok(input) => input,
            // No MIDI server (a build machine without one): nothing to test.
            Err(why) => return eprintln!("skipped: {why}"),
        };
        input.connect_virtual("Signal test source").unwrap();
        let mut rx = input.take_bytes().unwrap();
        input.send_virtual(&[0x90, 60, 100, 0xB0, 74, 64]).unwrap();
        input.send_virtual(&[0xF8]).unwrap();
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while got.len() < 7 && Instant::now() < deadline {
            while let Ok(b) = rx.pop() {
                got.push(b);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, [0x90, 60, 100, 0xB0, 74, 64, 0xF8]);
    }
}
