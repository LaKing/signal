//! `std.note_seq`'s poly output (ADR 0044 §3): every sounding key of the
//! pattern and of the audition notes, as note events.

use signal_core::{Event, EventPayload, Outputs};

use crate::note_seq::audition_value;

/// MIDI's keys.
const KEYS: usize = 128;

/// The most events a frame may carry (`semantics.md` §4).
const PER_FRAME: usize = 16;

/// `dropped` saturates here, below the fault guard's 10⁶ (as
/// `std.count`'s `lost` does).
pub(crate) const DROPPED_MAX: u64 = (1 << 19) - 1;

/// Every key the sequencer sounds, for its `notes` output: the pattern's
/// (its ticks left, or its end beat when following a position) and the
/// audition's, with what changed within the current frame.
#[derive(Clone, Debug)]
pub(crate) struct Poly {
    /// Counting ticks: a pattern key's ticks left (0: not sounding).
    ticks: [u64; KEYS],
    /// Following a position: a pattern key's end beat.
    ends: [Option<f64>; KEYS],
    /// A pattern key's velocity, in 127ths.
    velocity: [u8; KEYS],
    /// An audition key's velocity in 127ths (0: not held).
    audition: [u8; KEYS],
    /// Whether each key sounded when the frame began.
    was: [bool; KEYS],
    /// The key stopped sounding at some point within the frame.
    dropped_out: [bool; KEYS],
    /// The pattern started the key again while it sounded (a retrigger).
    retriggered: [bool; KEYS],
    /// The velocity of the key's latest start, in 127ths.
    started_at: [u8; KEYS],
    /// Each key's latest start, as a count (for the reduction's order).
    start_order: [u64; KEYS],
    starts: u64,
    /// Notes left out by a reduction, since `prepare` (saturating).
    pub(crate) dropped: u64,
}

impl Default for Poly {
    fn default() -> Poly {
        Poly {
            ticks: [0; KEYS],
            ends: [None; KEYS],
            velocity: [0; KEYS],
            audition: [0; KEYS],
            was: [false; KEYS],
            dropped_out: [false; KEYS],
            retriggered: [false; KEYS],
            started_at: [0; KEYS],
            start_order: [0; KEYS],
            starts: 0,
            dropped: 0,
        }
    }
}

/// A velocity (0 to 1) in 127ths, as the note convention carries it.
fn v127(velocity: f64) -> u8 {
    (audition_value(0, velocity, true) as u8).clamp(1, 127)
}

impl Poly {
    fn pattern_sounding(&self, k: usize) -> bool {
        self.ticks.get(k).is_some_and(|&t| t > 0) || self.ends.get(k).is_some_and(Option::is_some)
    }

    fn sounding(&self, k: usize) -> bool {
        self.pattern_sounding(k) || self.audition.get(k).is_some_and(|&v| v > 0)
    }

    /// Before a frame's ticks or crossing.
    pub(crate) fn begin_frame(&mut self) {
        for k in 0..KEYS {
            let on = self.sounding(k);
            if let Some(was) = self.was.get_mut(k) {
                *was = on;
            }
        }
        self.dropped_out = [false; KEYS];
        self.retriggered = [false; KEYS];
    }

    /// Key `k` (a pattern note or an audition note) starts at `velocity`.
    fn started(&mut self, k: usize, velocity: f64) {
        self.starts += 1;
        if let Some(at) = self.started_at.get_mut(k) {
            *at = v127(velocity);
        }
        if let Some(order) = self.start_order.get_mut(k) {
            *order = self.starts;
        }
    }

    /// Key `k` may have stopped sounding.
    fn maybe_ended(&mut self, k: usize) {
        if !self.sounding(k)
            && let Some(out) = self.dropped_out.get_mut(k)
        {
            *out = true;
        }
    }

    /// A pattern note of `key` starts, for `ticks` ticks (counting) or
    /// until beat `end` (following).
    pub(crate) fn start(&mut self, key: f64, velocity: f64, ticks: Option<u64>, end: Option<f64>) {
        let Some(k) = key_index(key) else {
            return;
        };
        if self.pattern_sounding(k)
            && let Some(r) = self.retriggered.get_mut(k)
        {
            *r = true;
        }
        if let Some(slot) = self.ticks.get_mut(k) {
            *slot = ticks.unwrap_or(0);
        }
        if let Some(slot) = self.ends.get_mut(k) {
            *slot = end;
        }
        if let Some(slot) = self.velocity.get_mut(k) {
            *slot = v127(velocity);
        }
        self.started(k, velocity);
    }

    /// Ends pattern key `k`.
    fn end(&mut self, k: usize) {
        if let Some(t) = self.ticks.get_mut(k) {
            *t = 0;
        }
        if let Some(e) = self.ends.get_mut(k) {
            *e = None;
        }
        self.maybe_ended(k);
    }

    /// One tick: each pattern key's ticks go down, ending those at 0.
    pub(crate) fn tick(&mut self) {
        for k in 0..KEYS {
            if let Some(t) = self.ticks.get_mut(k)
                && *t > 0
            {
                *t -= 1;
                if *t == 0 {
                    self.maybe_ended(k);
                }
            }
        }
    }

    /// Following: the pattern keys whose end is at or before `beat` end.
    pub(crate) fn end_until(&mut self, beat: f64, inclusive: bool) {
        for k in 0..KEYS {
            let due = self
                .ends
                .get(k)
                .copied()
                .flatten()
                .is_some_and(|end| if inclusive { end <= beat } else { end < beat });
            if due {
                self.end(k);
            }
        }
    }

    /// A seek or a stop: every pattern key ends (the audition's stay).
    pub(crate) fn silence(&mut self) {
        for k in 0..KEYS {
            if self.pattern_sounding(k) {
                self.end(k);
            }
        }
    }

    /// An audition note on (`Some(key)`) or off of `key` (`None` for every
    /// audition key).
    pub(crate) fn audition_on(&mut self, key: f64, velocity: f64) {
        let Some(k) = key_index(key) else {
            return;
        };
        if self.audition.get(k).is_some_and(|&v| v > 0)
            && let Some(r) = self.retriggered.get_mut(k)
        {
            *r = true;
        }
        if let Some(slot) = self.audition.get_mut(k) {
            *slot = v127(velocity);
        }
        self.started(k, velocity);
    }

    pub(crate) fn audition_off(&mut self, key: Option<f64>) {
        match key {
            Some(key) => {
                if let Some(k) = key_index(key) {
                    if let Some(slot) = self.audition.get_mut(k) {
                        *slot = 0;
                    }
                    self.maybe_ended(k);
                }
            }
            None => {
                for k in 0..KEYS {
                    if self.audition.get(k).is_some_and(|&v| v > 0) {
                        if let Some(slot) = self.audition.get_mut(k) {
                            *slot = 0;
                        }
                        self.maybe_ended(k);
                    }
                }
            }
        }
    }

    /// The velocity a sounding key is announced with.
    fn velocity_of(&self, k: usize) -> u8 {
        self.started_at
            .get(k)
            .copied()
            .filter(|&v| v > 0)
            .unwrap_or(1)
    }

    /// After the frame: its note events at offset `n` on event output
    /// `port`, the offs first then the ons, each in key order; a frame of
    /// more than 16 reduced to its end state (a Trigger, then an on for
    /// each key still sounding, at most 15, latest start first then
    /// highest key), the keys left out counted in `dropped`.
    pub(crate) fn end_frame(&mut self, outputs: &mut Outputs<'_>, port: usize, n: usize) {
        let mut offs = [false; KEYS];
        let mut ons = [false; KEYS];
        let (mut count, mut sounding) = (0usize, 0usize);
        for k in 0..KEYS {
            let was = self.was.get(k).copied().unwrap_or(false);
            let now = self.sounding(k);
            let out = self.dropped_out.get(k).copied().unwrap_or(false);
            let again = self.retriggered.get(k).copied().unwrap_or(false);
            let (off, on) = match (was, now) {
                (true, false) => (true, false),
                (true, true) if out => (true, true),
                (true, true) => (false, again),
                (false, true) => (false, true),
                (false, false) => (false, false),
            };
            if let Some(slot) = offs.get_mut(k) {
                *slot = off;
            }
            if let Some(slot) = ons.get_mut(k) {
                *slot = on;
            }
            count += usize::from(off) + usize::from(on);
            sounding += usize::from(now);
        }
        let offset = u32::try_from(n).unwrap_or(u32::MAX);
        let Some(sink) = outputs.events(port) else {
            return;
        };
        let mut push = |payload: EventPayload| sink.push(Event { offset, payload });
        if count <= PER_FRAME {
            for k in (0..KEYS).filter(|&k| offs.get(k).copied().unwrap_or(false)) {
                push(EventPayload::Set(128.0 * k as f64));
            }
            for k in (0..KEYS).filter(|&k| ons.get(k).copied().unwrap_or(false)) {
                push(EventPayload::Set(
                    128.0 * k as f64 + f64::from(self.velocity_of(k)),
                ));
            }
            return;
        }
        push(EventPayload::Trigger);
        // The keys still sounding, latest start first then highest key.
        let mut kept = 0;
        let mut last: Option<(u64, usize)> = None;
        while kept < PER_FRAME - 1 {
            let next = (0..KEYS)
                .filter(|&k| self.sounding(k))
                .map(|k| (self.start_order.get(k).copied().unwrap_or(0), k))
                .filter(|&key| last.is_none_or(|l| key < l))
                .max();
            let Some((order, k)) = next else {
                break;
            };
            push(EventPayload::Set(
                128.0 * k as f64 + f64::from(self.velocity_of(k)),
            ));
            last = Some((order, k));
            kept += 1;
        }
        let left = sounding.saturating_sub(kept) as u64;
        self.dropped = (self.dropped + left).min(DROPPED_MAX);
    }
}

/// A key within MIDI's range, as an index.
fn key_index(key: f64) -> Option<usize> {
    (key.is_finite() && (0.0..KEYS as f64).contains(&key)).then_some(key as usize)
}
