//! The protocol's binary data frames (`docs/ui-protocol.md`, "Binary
//! frames"): a 24-byte little-endian header, then the payload.
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | u8 | kind: 1 meters, 2 scope, 3 parameters, 4 vector |
//! | 1 | u8 | protocol version |
//! | 2 | u16 | reserved, 0 |
//! | 4 | u32 | subscription |
//! | 8 | u64 | first frame |
//! | 16 | u32 | count |
//! | 20 | u32 | meters: frames covered; scope and vector: decimation; parameters: 0 |

use crate::PROTOCOL;

/// The header's size.
pub const HEADER: usize = 24;
/// Per channel: `f32` peak, `f32` RMS (linear, full scale 1).
pub const METERS: u8 = 1;
/// `f32` points.
pub const SCOPE: u8 = 2;
/// Per address: `f64` base, `f64` effective (NaN when not known yet).
pub const PARAMETERS: u8 = 3;
/// `f32` rows of a vector output at one frame (M2.8-7).
pub const VECTOR: u8 = 4;

/// A frame with its header, ready for `payload` bytes.
pub(crate) fn frame(
    kind: u8,
    sub: u32,
    first: u64,
    count: u32,
    extra: u32,
    payload: usize,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + payload);
    bytes.push(kind);
    bytes.push(PROTOCOL as u8);
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&sub.to_le_bytes());
    bytes.extend_from_slice(&first.to_le_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    bytes.extend_from_slice(&extra.to_le_bytes());
    bytes
}
