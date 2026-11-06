//! The source handoff (ADR 0038 §7, the mirror of [`crate::sink`]): the
//! engine knows sources only as instances whose descriptor declares a
//! [`SourceKind`], and lets a driver write their external rows, block by
//! block ([`SinkOutput::sources`](crate::SinkOutput::sources)), before the
//! block runs: frame phase 1, external input. What a kind means (an audio
//! device's channels, a recorded input, a render fixture) is the driver's
//! policy, outside the engine.

use signal_core::{InstanceAddress, SourceKind};

use crate::log::InstanceId;

/// A source of the running plan ([`Engine::sources`](crate::Engine::sources)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source<'a> {
    /// The instance, for [`Engine::source_rows_mut`](crate::Engine::source_rows_mut).
    pub id: InstanceId,
    /// The kind its descriptor declares.
    pub kind: SourceKind,
    /// Its address.
    pub address: &'a InstanceAddress,
    /// Its external rows: one per signal output row.
    pub rows: usize,
}
