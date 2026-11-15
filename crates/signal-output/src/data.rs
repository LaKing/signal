//! The data adapter (ADR 0057 §2): what a data sink (`SinkKind::Data`, a
//! `dmx` device slot's `std.data_out<N>`) hands to the world. At every frame
//! that is a multiple of the hop ([`data_hop`]: under Art-Net's 44 Hz DMX
//! refresh), each data sink's inputs become one DMX frame, each value in
//! `[0, 1]` (clamped; anything not finite 0) a byte `round(255·v)`.
//!
//! Offline the frames are collected ([`DataTrack`], written by `signal
//! render` as `devices/<slot>.dmx`); live, the same frames go through a ring
//! to the control thread, which sends each as one ArtDmx packet
//! ([`art_dmx`]). Frames are counted, never timed: the frames a run hands
//! out do not depend on its callbacks.

use std::io;
use std::net::{SocketAddr, UdpSocket};

use rtrb::{Consumer, Producer, RingBuffer};
use signal_core::{InstanceAddress, SinkKind};
use signal_engine::{Engine, InstanceId};

/// Art-Net's DMX refresh limit, frames per second.
pub const DMX_RATE: f64 = 44.0;

/// The most channels of one DMX universe.
pub const DMX_CHANNELS: usize = 512;

/// A data sink of the plan, as `signal-project` names it (ADR 0057 §2):
/// handed beside the plan, as play sides are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSide {
    /// The sink instance (`/lights/play`).
    pub address: InstanceAddress,
    /// The device slot it plays for (`lights`).
    pub slot: String,
    /// The slot's role.
    pub role: String,
    /// The slot's kind: `dmx` for a data sink; `osc` or `midi` for a
    /// control source (ADR 0057 §1).
    pub kind: String,
    /// Its channels' names, in DMX order from 1 (a control source's values,
    /// in output order).
    pub ports: Vec<String>,
}

/// The frames between two DMX frames at `sample_rate`: the smallest whole
/// number at or above `sample_rate / 44`, so a run never exceeds 44 Hz
/// (1091 at 48 kHz: 43.996 Hz).
pub fn data_hop(sample_rate: u32) -> u64 {
    ((f64::from(sample_rate) / DMX_RATE).ceil() as u64).max(1)
}

/// A level as a DMX byte: `[0, 1]` clamped, `round(255·v)`, 0 when not
/// finite.
pub fn dmx_byte(v: f64) -> u8 {
    if v.is_finite() {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

/// One data sink's DMX frames over a render: each frame number (a multiple
/// of the hop) with its channel bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataTrack {
    /// The sink.
    pub address: InstanceAddress,
    /// Its channels.
    pub width: usize,
    /// `(frame, bytes)` per DMX frame, in order.
    pub frames: Vec<(u64, Vec<u8>)>,
}

impl DataTrack {
    /// The track as `devices/<slot>.dmx` holds it: a header line, then one
    /// line per frame, its number and its bytes in hex.
    pub fn text(&self, hop: u64) -> String {
        let mut text = format!(
            "# signal dmx: {} channels, a frame every {hop} frames\n",
            self.width
        );
        for (frame, bytes) in &self.frames {
            text.push_str(&frame.to_string());
            text.push(' ');
            for byte in bytes {
                text.push_str(&format!("{byte:02x}"));
            }
            text.push('\n');
        }
        text
    }
}

/// The plan's data sinks, bound by address at each plan, sampled at the hop.
#[derive(Debug)]
pub(crate) struct DataSinks {
    hop: u64,
    /// Per sink: its address, width and id in the running plan (none when
    /// the plan lacks it).
    sinks: Vec<(InstanceAddress, usize, Option<InstanceId>)>,
    pub(crate) tracks: Vec<DataTrack>,
}

impl DataSinks {
    /// The data sinks of `engine`'s plan, in address order, sampled every
    /// `hop` frames.
    pub(crate) fn new(engine: &Engine, hop: u64) -> DataSinks {
        let mut sinks: Vec<_> = engine
            .sinks()
            .filter(|s| s.kind == SinkKind::Data)
            .map(|s| (s.address.clone(), s.inputs.min(DMX_CHANNELS), Some(s.id)))
            .collect();
        sinks.sort_by(|a, b| a.0.cmp(&b.0));
        let tracks = sinks
            .iter()
            .map(|(address, width, _)| DataTrack {
                address: address.clone(),
                width: *width,
                frames: Vec::new(),
            })
            .collect();
        DataSinks {
            hop: hop.max(1),
            sinks,
            tracks,
        }
    }

    /// Binds each sink by its address in `engine`'s running plan.
    pub(crate) fn bind(&mut self, engine: &Engine) {
        for (address, width, id) in &mut self.sinks {
            *id = engine
                .sinks()
                .find(|s| s.kind == SinkKind::Data && s.address == address && s.inputs >= *width)
                .map(|s| s.id);
        }
    }

    /// The last block's frames `at .. at + frames` that fall on the hop,
    /// each sink's bytes collected into its track.
    pub(crate) fn block(&mut self, engine: &Engine, at: u64, frames: usize) {
        let first = at.div_ceil(self.hop) * self.hop;
        let mut frame = first;
        while frame < at + frames as u64 {
            let f = (frame - at) as usize;
            for ((_, width, id), track) in self.sinks.iter().zip(&mut self.tracks) {
                let bytes = (0..*width)
                    .map(|c| {
                        id.and_then(|id| engine.sink_input_at(id, c, f))
                            .map_or(0, dmx_byte)
                    })
                    .collect();
                track.frames.push((frame, bytes));
            }
            frame += self.hop;
        }
    }
}

/// The most channels a live DMX frame carries (a data slot's widest).
pub const MAX_DATA_FRAME: usize = 64;

/// One DMX frame of one data sink, as the audio thread hands it to the
/// control thread (fixed size: nothing allocated).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataFrame {
    /// The sink's index among the plan's data sinks, in address order.
    pub sink: usize,
    /// The frame it was taken at (a multiple of the hop).
    pub frame: u64,
    /// Its channels.
    pub len: usize,
    /// Its bytes, `len` of them.
    pub bytes: [u8; MAX_DATA_FRAME],
}

/// The live half of the data adapter, on the audio thread: the data sinks
/// sampled at the hop into a preallocated ring.
pub(crate) struct LiveData {
    hop: u64,
    sinks: Vec<(InstanceAddress, usize, Option<InstanceId>)>,
    tx: Producer<DataFrame>,
    /// Frames the full ring dropped.
    pub(crate) dropped: u64,
}

impl std::fmt::Debug for LiveData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveData")
            .field("hop", &self.hop)
            .finish_non_exhaustive()
    }
}

impl LiveData {
    /// The data sinks of `engine`'s plan (address order) every `hop`
    /// frames, and the control thread's end of their ring (a second of
    /// frames per sink).
    pub(crate) fn new(engine: &Engine, hop: u64) -> (LiveData, Consumer<DataFrame>) {
        let mut sinks: Vec<_> = engine
            .sinks()
            .filter(|s| s.kind == SinkKind::Data)
            .map(|s| (s.address.clone(), s.inputs.min(MAX_DATA_FRAME), Some(s.id)))
            .collect();
        sinks.sort_by(|a, b| a.0.cmp(&b.0));
        let (tx, rx) = RingBuffer::new((sinks.len() * 64).max(1));
        (
            LiveData {
                hop: hop.max(1),
                sinks,
                tx,
                dropped: 0,
            },
            rx,
        )
    }

    /// Binds each sink by address in the running plan. Allocates nothing.
    pub(crate) fn bind(&mut self, engine: &Engine) {
        for (address, width, id) in &mut self.sinks {
            *id = engine
                .sinks()
                .find(|s| s.kind == SinkKind::Data && s.address == address && s.inputs >= *width)
                .map(|s| s.id);
        }
    }

    /// The block's frames `at .. at + frames` on the hop into the ring.
    /// Allocates nothing; a full ring drops the frame and counts it.
    pub(crate) fn block(&mut self, engine: &Engine, at: u64, frames: usize) {
        let mut frame = at.div_ceil(self.hop) * self.hop;
        while frame < at + frames as u64 {
            let f = (frame - at) as usize;
            for (sink, (_, width, id)) in self.sinks.iter().enumerate() {
                let mut bytes = [0u8; MAX_DATA_FRAME];
                for (c, byte) in bytes.iter_mut().enumerate().take(*width) {
                    *byte = id
                        .and_then(|id| engine.sink_input_at(id, c, f))
                        .map_or(0, dmx_byte);
                }
                let out = DataFrame {
                    sink,
                    frame,
                    len: *width,
                    bytes,
                };
                if self.tx.push(out).is_err() {
                    self.dropped = self.dropped.saturating_add(1);
                }
            }
            frame += self.hop;
        }
    }
}

/// The control thread's end of the data sinks' ring
/// ([`Runner::send_data`](crate::Runner::send_data)).
pub type DataFrames = Consumer<DataFrame>;

/// Where a data sink's frames go: an Art-Net node and its universe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtNetTarget {
    /// The node (port 6454 unless written).
    pub address: SocketAddr,
    /// Its 15-bit port-address: net, sub-net, universe.
    pub universe: u16,
}

/// The control thread's half: sends each frame from the ring as an ArtDmx
/// packet to its sink's target (none: not sent), sequence 1–255 per sink.
#[derive(Debug)]
pub struct ArtNetSender {
    socket: UdpSocket,
    targets: Vec<Option<ArtNetTarget>>,
    sequences: Vec<u8>,
    /// Packets sent.
    pub sent: u64,
    /// Packets the socket refused.
    pub failed: u64,
}

impl ArtNetSender {
    /// A sender for the data sinks in address order, `targets[i]` the
    /// i-th's (none: its frames are dropped), from an unbound local port.
    pub fn new(targets: Vec<Option<ArtNetTarget>>) -> io::Result<ArtNetSender> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_broadcast(true)?;
        let sequences = vec![0; targets.len()];
        Ok(ArtNetSender {
            socket,
            targets,
            sequences,
            sent: 0,
            failed: 0,
        })
    }

    /// Sends every frame waiting in `rx`; how many were sent.
    pub fn drain(&mut self, rx: &mut Consumer<DataFrame>) -> usize {
        let mut count = 0;
        while let Ok(frame) = rx.pop() {
            let Some(Some(target)) = self.targets.get(frame.sink) else {
                continue;
            };
            let Some(sequence) = self.sequences.get_mut(frame.sink) else {
                continue;
            };
            *sequence = if *sequence == 255 { 1 } else { *sequence + 1 };
            let data = frame.bytes.get(..frame.len).unwrap_or_default();
            let packet = art_dmx(*sequence, target.universe, data);
            match self.socket.send_to(&packet, target.address) {
                Ok(_) => {
                    self.sent += 1;
                    count += 1;
                }
                Err(_) => self.failed += 1,
            }
        }
        count
    }
}

/// `IP[:PORT][/UNIVERSE]` as an Art-Net target (port 6454, universe 0 by
/// default).
pub fn parse_artnet(text: &str) -> Result<ArtNetTarget, String> {
    let (host, universe) = match text.rsplit_once('/') {
        Some((host, u)) => (
            host,
            u.trim()
                .parse::<u16>()
                .ok()
                .filter(|u| *u <= 0x7fff)
                .ok_or_else(|| format!("`{u}` is not a universe (0 to 32767)"))?,
        ),
        None => (text, 0),
    };
    let with_port = if host.contains(':') {
        host.to_owned()
    } else {
        format!("{host}:6454")
    };
    let address = with_port
        .parse::<SocketAddr>()
        .map_err(|_| format!("`{host}` is not IP[:PORT]"))?;
    Ok(ArtNetTarget { address, universe })
}

/// An ArtDmx packet (Art-Net 4, OpCode 0x5000, protocol 14): `sequence`
/// (1–255; 0 disables reordering), physical port 0, `universe` (15 bits:
/// net, sub-net, universe), and `data` padded to an even length of at least
/// 2 and cut at 512.
pub fn art_dmx(sequence: u8, universe: u16, data: &[u8]) -> Vec<u8> {
    let mut length = data.len().clamp(2, DMX_CHANNELS);
    length += length % 2;
    let mut packet = Vec::with_capacity(18 + length);
    packet.extend_from_slice(b"Art-Net\0");
    packet.extend_from_slice(&0x5000u16.to_le_bytes());
    packet.extend_from_slice(&14u16.to_be_bytes());
    packet.push(sequence);
    packet.push(0);
    packet.extend_from_slice(&(universe & 0x7fff).to_le_bytes());
    packet.extend_from_slice(&(length as u16).to_be_bytes());
    packet.extend(data.iter().take(length).copied());
    packet.resize(18 + length, 0);
    packet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_reads_as_ip_port_and_universe() {
        let t = parse_artnet("2.0.0.1").unwrap();
        assert_eq!(
            (t.address.to_string(), t.universe),
            ("2.0.0.1:6454".to_owned(), 0)
        );
        let t = parse_artnet("127.0.0.1:7000/3").unwrap();
        assert_eq!(
            (t.address.to_string(), t.universe),
            ("127.0.0.1:7000".to_owned(), 3)
        );
        assert!(parse_artnet("lamp").is_err());
        assert!(parse_artnet("2.0.0.1/40000").is_err());
    }

    #[test]
    fn the_hop_stays_under_44_hz() {
        assert_eq!(data_hop(48_000), 1091);
        assert!(48_000.0 / data_hop(48_000) as f64 <= DMX_RATE);
        assert_eq!(data_hop(44_100), 1003);
        assert_eq!(data_hop(96_000), 2182);
    }

    #[test]
    fn levels_become_bytes() {
        assert_eq!(dmx_byte(0.0), 0);
        assert_eq!(dmx_byte(1.0), 255);
        assert_eq!(dmx_byte(0.5), 128);
        assert_eq!(dmx_byte(-3.0), 0);
        assert_eq!(dmx_byte(7.0), 255);
        assert_eq!(dmx_byte(f64::NAN), 0);
    }

    /// The ArtDmx layout (Art-Net 4 specification, ArtDmx).
    #[test]
    fn an_art_dmx_packet_is_laid_out_as_the_specification_says() {
        let packet = art_dmx(7, 0x0102, &[10, 20, 30]);
        assert_eq!(&packet[..8], b"Art-Net\0");
        assert_eq!(
            &packet[8..10],
            &[0x00, 0x50],
            "OpCode 0x5000, low byte first"
        );
        assert_eq!(&packet[10..12], &[0, 14], "protocol 14, high byte first");
        assert_eq!(packet[12], 7, "sequence");
        assert_eq!(packet[13], 0, "physical");
        assert_eq!(&packet[14..16], &[0x02, 0x01], "SubUni, Net");
        assert_eq!(&packet[16..18], &[0, 4], "length, even, high byte first");
        assert_eq!(&packet[18..], &[10, 20, 30, 0]);
        assert_eq!(art_dmx(1, 0, &[]).len(), 20, "at least 2 channels");
        assert_eq!(art_dmx(1, 0, &[1; 600]).len(), 18 + 512);
    }
}
