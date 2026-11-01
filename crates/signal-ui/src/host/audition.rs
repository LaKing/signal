//! Auditioned notes (ADR 0039): the notes each client holds, and their
//! note-offs once it goes.

use signal_core::EventPayload;
use signal_engine::Stamp;

use super::Host;
use super::queue::{Job, LIVE_EDITS};

/// The event input a `note` goes to (ADR 0039): `std.note_seq`'s.
pub const AUDITION_INPUT: &str = "audition";

/// A `note` without a `velocity` (ADR 0039): the piano roll's default note's.
pub(super) const NOTE_VELOCITY: f64 = 0.8;

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
        self.waiting.retain(|w| match &w.job {
            Job::Message {
                client: c,
                lock: false,
                kind,
                ..
            } => *c != client || !LIVE_EDITS.contains(&kind.as_str()),
            _ => true,
        });
        if let Some(held) = self.auditions.remove(&client) {
            self.releases.extend(held);
        }
        self.send_releases();
    }

    /// Sends the waiting note-offs; keeps those the Controller refuses.
    pub(super) fn send_releases(&mut self) {
        if self.releases.is_empty() {
            return;
        }
        let Some(run) = self.run.as_mut() else {
            // No run: nothing sounds.
            self.releases.clear();
            return;
        };
        self.releases.retain(|(port, key)| {
            let off = EventPayload::Set(signal_std::audition_value(*key, 0.0, false));
            run.controller
                .send_event(port.clone(), off, Stamp::Default)
                .is_err()
        });
    }

    /// Test hooks: the note-offs still waiting for the Controller.
    #[cfg(feature = "test-hooks")]
    pub fn releases_waiting(&self) -> usize {
        self.releases.len()
    }

    pub(super) fn release(&mut self, client: u64) {
        let subs: Vec<u32> = self
            .subs
            .iter()
            .filter(|(_, s)| s.client == client)
            .map(|(&id, _)| id)
            .collect();
        for sub in subs {
            self.drop_sub(sub);
        }
    }
}
