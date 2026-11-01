//! Auditioned notes (ADR 0039): the notes each client holds, and their
//! note-offs once it goes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use signal_core::{EventPayload, PortAddress};
use signal_engine::Stamp;
use signal_output::Controller;

use super::Host;
use super::queue::{Job, Order, class_of};

/// The event input a `note` goes to (ADR 0039): `std.note_seq`'s.
pub const AUDITION_INPUT: &str = "audition";

/// A `note` without a `velocity` (ADR 0039): the piano roll's default note's.
pub(super) const NOTE_VELOCITY: f64 = 0.8;

/// The live notes each client holds (`note`, ADR 0039), and the note-offs
/// not yet taken of notes whose client went.
#[derive(Default)]
pub(super) struct Auditions {
    /// Each client's held notes: the port and key, ended when the client
    /// goes so that no note is left sounding.
    held: BTreeMap<u64, BTreeSet<(PortAddress, u8)>>,
    /// The note-offs of notes whose client went (or was evicted) that the
    /// Controller has not taken yet (a full command queue): sent again
    /// each step until it takes them. At most one per port and key, so
    /// bounded by the plan's audition inputs times 128 keys.
    releases: BTreeSet<(PortAddress, u8)>,
}

impl Auditions {
    /// A new run, or none: nothing is held or waits.
    pub(super) fn clear(&mut self) {
        self.held.clear();
        self.releases.clear();
    }

    /// Key `key` on `port` taken on (`on`) or off for `client`.
    pub(super) fn hold(&mut self, client: u64, port: PortAddress, key: u8, on: bool) {
        // A note of this key taken now supersedes a release still
        // waiting for it (that note was ended; this one is held).
        self.releases.remove(&(port.clone(), key));
        let held = self.held.entry(client).or_default();
        if on {
            // One held note per port and client: a new one takes over,
            // as the sequencer's voice does.
            held.retain(|(p, _)| *p != port);
            held.insert((port, key));
        } else {
            // Only the key held ends: an off of a key slid away from (or
            // of another roll's key on the port) leaves the held one,
            // which the sequencer still sounds.
            held.remove(&(port, key));
        }
        if held.is_empty() {
            self.held.remove(&client);
        }
    }

    /// A note to end at once (its client is gone).
    pub(super) fn end_now(&mut self, port: PortAddress, key: u8) {
        self.releases.insert((port, key));
    }

    /// `client` went: the notes it holds are to be ended.
    fn end(&mut self, client: u64) {
        if let Some(held) = self.held.remove(&client) {
            self.releases.extend(held);
        }
    }

    /// Sends the waiting note-offs; keeps those the Controller refuses.
    fn send_releases(&mut self, controller: Option<&mut Controller>) {
        if self.releases.is_empty() {
            return;
        }
        let Some(controller) = controller else {
            // No run: nothing sounds.
            self.releases.clear();
            return;
        };
        self.releases.retain(|(port, key)| {
            let off = EventPayload::Set(signal_std::audition_value(*key, 0.0, false));
            controller
                .send_event(port.clone(), off, Stamp::Default)
                .is_err()
        });
    }
}

impl Host {
    /// A client gone or evicted with a key held (a closed page, a lost
    /// connection, a stalled socket): its notes end, unacknowledged, so
    /// none is left sounding. A note-off the Controller refuses (its
    /// command queue full) waits in `releases` and is sent again by every
    /// step until it is taken ([`Host::send_releases`]).
    pub(super) fn end_notes(&mut self, client: u64) {
        // Its live edits still waiting behind the writer lock are dropped:
        // a note it can no longer end, or a value it can no longer see,
        // is never played for it (its file writes still run, as asked).
        self.queue.waiting.retain(|w| match &w.job {
            Job::Message {
                client: c,
                lock: false,
                kind,
                ..
            } => *c != client || !matches!(class_of(kind, &Value::Null).order, Order::Live(_)),
            _ => true,
        });
        self.auditions.end(client);
        self.send_releases();
    }

    /// Sends the waiting note-offs; keeps those the Controller refuses.
    pub(super) fn send_releases(&mut self) {
        let controller = self.run.as_mut().map(|run| &mut run.controller);
        self.auditions.send_releases(controller);
    }

    /// Test hooks: the note-offs still waiting for the Controller.
    #[cfg(feature = "test-hooks")]
    pub fn releases_waiting(&self) -> usize {
        self.auditions.releases.len()
    }
}
