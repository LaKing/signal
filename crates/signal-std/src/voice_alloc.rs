//! `std.voice_alloc` (ADR 0044 §4): a polyphonic slot's allocator.

use signal_core::{
    Combine, DirectPrimitive, Direction, EventPayload, Feedthrough, Inputs, ModuleDescriptor,
    Outputs, PortKind, PortShape, PrepareContext,
};

use crate::{OFF_ON, choice, event_port, parameter, port_at};

/// The most voices a slot may have (ADR 0044 §1), and the width of every
/// output.
pub const MAX_VOICES: usize = 64;

/// MIDI's key range, the note convention's (ADR 0039, ADR 0044 §3).
const KEYS: usize = 128;

/// No voice plays the key.
const NONE: u8 = u8::MAX;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.voice_alloc",
    doc: "A polyphonic slot's allocator (ADR 0044): note events (Set(128·key + v), v 1–127 an on at v/127, 0 an off; a Trigger ends every note) shared among `voices` voices, a free voice released longest ago first, else the oldest note stolen; a steal or retrigger takes the gate low for one frame (not with `legato`). Each output a vector of 64, element e voice e: hz (held after the note), gate, velocity (held).",
    ports: &[
        event_port("notes", Direction::In),
        port_at(
            "hz",
            Direction::Out,
            PortShape::Vector(MAX_VOICES as u32),
            1,
            PortKind::Frequency,
            Some("Hz"),
            None,
        ),
        port_at(
            "gate",
            Direction::Out,
            PortShape::Vector(MAX_VOICES as u32),
            1,
            PortKind::Gate,
            None,
            None,
        ),
        port_at(
            "velocity",
            Direction::Out,
            PortShape::Vector(MAX_VOICES as u32),
            1,
            PortKind::Control,
            None,
            None,
        ),
    ],
    parameters: &[
        parameter(
            "voices",
            PortKind::Control,
            None,
            1.0,
            MAX_VOICES as f64,
            1.0,
            Combine::Replace,
        ),
        choice(
            parameter(
                "legato",
                PortKind::Gate,
                None,
                0.0,
                1.0,
                0.0,
                Combine::Replace,
            ),
            OFF_ON,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    source: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// A voice's state (ADR 0044 §4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Gate 0, no note.
    #[default]
    Free,
    /// Gate 1, playing its key.
    Sounding,
    /// Given a new note this frame while it sounded: gate 0 this frame,
    /// sounding with the new note from the next.
    Claimed,
}

/// One voice.
#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    state: State,
    /// The key it plays (or will, claimed); none when free.
    key: Option<u8>,
    /// What it outputs now (held after a note ends).
    hz: f64,
    velocity: f64,
    /// A claimed voice's new note: its hz and velocity from the next frame.
    pending: (f64, f64),
    /// The event counter at its last on or off; 0 for a voice never used.
    order: u64,
}

/// A note event, read (ADR 0044 §3's convention).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Note {
    On(u8, f64),
    Off(u8),
    AllOff,
}

fn read(payload: EventPayload) -> Option<Note> {
    match payload {
        EventPayload::Trigger => Some(Note::AllOff),
        EventPayload::Set(x) if x.is_finite() && x == x.trunc() && x >= 0.0 => {
            let key = (x / 128.0).trunc();
            let v = x - 128.0 * key;
            if key >= KEYS as f64 {
                return None;
            }
            let key = key as u8;
            Some(if v >= 1.0 {
                Note::On(key, v.min(127.0) / 127.0)
            } else {
                Note::Off(key)
            })
        }
        _ => None,
    }
}

/// `std.voice_alloc` (ADR 0044 §4): shares a polyphonic slot's note events
/// among its voices.
///
/// **Ports**
/// - `notes` (event in): ADR 0044 §3's note events, `Set(128·key + v)`
///   (`v` 1–127 an on at velocity `v/127`, 0 an off of that key); a
///   `Trigger` ends every note; anything else is ignored.
/// - `hz` (vector of 64 out, frequency, Hz): element *e* voice *e*'s
///   `440·2^((key − 69)/12)`, through `std.note_seq`'s function (so the
///   values are bit-identical); 0 before the voice's first note, held after
///   a note ends.
/// - `gate` (vector of 64 out, gate): 1 while voice *e* sounds, else 0.
/// - `velocity` (vector of 64 out, control): `v/127`, held after a note.
///
/// **Parameters**
/// - `voices` (1 to 64, read rounded, default 1): how many voices are used.
///   Falling, the voices from the new count up take gate 0 and keep their
///   `hz` and `velocity`; rising, the voices added are reset (gate, `hz`,
///   `velocity` and order 0), on the frame the new value is read.
/// - `legato` (off/on, default off): a sounding voice given a new note
///   takes it on that frame, its gate high (no one-frame gap, no claim).
///
/// **Each frame** the claimed voices become sounding with their new note;
/// then the frame's events apply in canonical order, one after another,
/// each raising an event counter that a voice records at its on or off:
/// 1. an on of a key a voice plays (sounding or claimed): a retrigger of
///    that voice with the new velocity;
/// 2. an on, a free voice among the first `voices`: the free voice with
///    the smallest order (released longest ago; ties to the lower index)
///    sounds from this frame;
/// 3. an on, no free voice: the sounding or claimed voice with the
///    smallest order (the oldest note) is stolen;
/// 4. an off of a key a voice plays: that voice is free (gate 0), a
///    claimed one's pending note cancelled; an off of a key no voice plays
///    (a stolen note) is ignored. A `Trigger` frees every voice.
///
/// A retrigger or steal of a sounding voice claims it: its gate is 0 on
/// that frame and 1 with the new note from the next (a new edge for an
/// envelope); with `legato` on it takes the note at once, its gate high.
///
/// **Determinism:** the state moves frame by frame from the events in
/// canonical order and the parameters only, so any partition gives the
/// same frames. **Real time:** fixed arrays (64 voices, a 128-key table);
/// each event scans at most 64 voices; no allocation. **Feedthrough:**
/// direct.
#[derive(Clone, Debug)]
pub struct VoiceAlloc {
    voices: [Voice; MAX_VOICES],
    /// The voice playing each key (sounding or claimed), or [`NONE`].
    by_key: [u8; KEYS],
    counter: u64,
    /// The count in use, once read.
    count: Option<usize>,
}

impl Default for VoiceAlloc {
    fn default() -> VoiceAlloc {
        VoiceAlloc {
            voices: [Voice::default(); MAX_VOICES],
            by_key: [NONE; KEYS],
            counter: 0,
            count: None,
        }
    }
}

impl VoiceAlloc {
    /// Voice `e` lets its key go (it no longer plays it).
    fn unmap(&mut self, e: usize) {
        if let Some(key) = self.voices.get(e).and_then(|v| v.key)
            && let Some(slot) = self.by_key.get_mut(usize::from(key))
            && usize::from(*slot) == e
        {
            *slot = NONE;
        }
        if let Some(voice) = self.voices.get_mut(e) {
            voice.key = None;
        }
    }

    /// Voice `e` plays `key` from now (sounding or claimed).
    fn map(&mut self, e: usize, key: u8) {
        if let Some(voice) = self.voices.get_mut(e) {
            voice.key = Some(key);
        }
        if let Some(slot) = self.by_key.get_mut(usize::from(key)) {
            *slot = e as u8;
        }
    }

    /// The count read at a frame: falling, the voices above it go silent
    /// (holding `hz` and `velocity`); rising, those added are reset.
    fn set_count(&mut self, count: usize) {
        let before = self.count.replace(count);
        match before {
            Some(was) if count < was => {
                for e in count..was {
                    self.unmap(e);
                    if let Some(voice) = self.voices.get_mut(e) {
                        voice.state = State::Free;
                    }
                }
            }
            Some(was) if count > was => {
                for e in was..count {
                    self.unmap(e);
                    if let Some(voice) = self.voices.get_mut(e) {
                        *voice = Voice::default();
                    }
                }
            }
            _ => {}
        }
    }

    /// One note event applied, among the first `count` voices.
    fn apply(&mut self, note: Note, count: usize, legato: bool) {
        self.counter += 1;
        let order = self.counter;
        match note {
            Note::On(key, velocity) => {
                let hz = crate::pattern::hz(f64::from(key));
                let playing = self
                    .by_key
                    .get(usize::from(key))
                    .copied()
                    .filter(|&e| e != NONE)
                    .map(usize::from)
                    .filter(|&e| e < count);
                let target = playing.or_else(|| {
                    // A free voice, released longest ago (ties to the
                    // lower index), else the oldest note.
                    let pick = |free: bool| {
                        self.voices
                            .iter()
                            .take(count)
                            .enumerate()
                            .filter(|(_, v)| (v.state == State::Free) == free)
                            .min_by_key(|(e, v)| (v.order, *e))
                            .map(|(e, _)| e)
                    };
                    pick(true).or_else(|| pick(false))
                });
                let Some(e) = target else {
                    return;
                };
                let state = self.voices.get(e).map_or(State::Free, |v| v.state);
                self.unmap(e);
                self.map(e, key);
                let Some(voice) = self.voices.get_mut(e) else {
                    return;
                };
                voice.order = order;
                match state {
                    State::Free => {
                        voice.state = State::Sounding;
                        voice.hz = hz;
                        voice.velocity = velocity;
                    }
                    State::Sounding if legato => {
                        voice.hz = hz;
                        voice.velocity = velocity;
                    }
                    State::Sounding | State::Claimed => {
                        voice.state = State::Claimed;
                        voice.pending = (hz, velocity);
                    }
                }
            }
            Note::Off(key) => {
                let Some(e) = self
                    .by_key
                    .get(usize::from(key))
                    .copied()
                    .filter(|&e| e != NONE)
                    .map(usize::from)
                else {
                    return;
                };
                self.unmap(e);
                if let Some(voice) = self.voices.get_mut(e) {
                    voice.state = State::Free;
                    voice.order = order;
                }
            }
            Note::AllOff => {
                for e in 0..MAX_VOICES {
                    if self.voices.get(e).is_some_and(|v| v.state != State::Free) {
                        self.unmap(e);
                        if let Some(voice) = self.voices.get_mut(e) {
                            voice.state = State::Free;
                            voice.order = order;
                        }
                    }
                }
            }
        }
    }
}

impl DirectPrimitive for VoiceAlloc {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        *self = VoiceAlloc::default();
    }

    fn reset(&mut self) {
        *self = VoiceAlloc::default();
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let frames = inputs.frames();
        let events = inputs.events(0);
        let counts = inputs.parameter(0);
        let legatos = inputs.parameter(1);
        let mut next = 0;
        for f in 0..frames {
            let count =
                (counts.get(f).copied().unwrap_or(1.0).round() as usize).clamp(1, MAX_VOICES);
            let legato = legatos.get(f).copied().unwrap_or(0.0) >= 0.5;
            self.set_count(count);
            // Claimed voices sound with their new note.
            for voice in &mut self.voices {
                if voice.state == State::Claimed {
                    voice.state = State::Sounding;
                    (voice.hz, voice.velocity) = voice.pending;
                }
            }
            while let Some(event) = events.get(next).filter(|e| e.offset as usize <= f) {
                next += 1;
                if let Some(note) = read(event.payload) {
                    self.apply(note, count, legato);
                }
            }
            for (e, voice) in self.voices.iter().enumerate() {
                let gate = if e < count && voice.state == State::Sounding {
                    1.0
                } else {
                    0.0
                };
                for (port, value) in [(0, voice.hz), (1, gate), (2, voice.velocity)] {
                    if let Some(row) = outputs.rows(port * MAX_VOICES + e, 1).get_mut(f) {
                        *row = value;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signal_core::Event;

    const ON: f64 = 127.0;

    fn on(key: u8) -> EventPayload {
        EventPayload::Set(128.0 * f64::from(key) + ON)
    }

    fn off(key: u8) -> EventPayload {
        EventPayload::Set(128.0 * f64::from(key))
    }

    /// Runs `frames` frames with `voices` voices and `events` (frame,
    /// payload); every frame's (gate, hz) of the first `show` voices.
    fn run(
        voices: f64,
        legato: bool,
        frames: usize,
        events: &[(usize, EventPayload)],
        show: usize,
    ) -> Vec<Vec<(f64, f64)>> {
        let mut alloc = VoiceAlloc::default();
        let mut out = Vec::new();
        for f in 0..frames {
            let these: Vec<Event> = events
                .iter()
                .filter(|(at, _)| *at == f)
                .map(|(_, p)| Event {
                    offset: 0,
                    payload: *p,
                })
                .collect();
            let ends = [these.len()];
            let params = [voices, if legato { 1.0 } else { 0.0 }];
            let inputs = Inputs::new(1, &[], &params, &these, &ends);
            let mut rows = vec![0.0; 3 * MAX_VOICES];
            let mut outputs = Outputs::new(1, &mut rows, &mut []);
            alloc.process(&inputs, &mut outputs);
            out.push((0..show).map(|e| (rows[MAX_VOICES + e], rows[e])).collect());
        }
        out
    }

    fn hz(key: u8) -> f64 {
        crate::pattern::hz(f64::from(key))
    }

    #[test]
    fn a_chord_takes_free_voices_and_a_release_rings_on_its_own() {
        let frames = run(
            3.0,
            false,
            4,
            &[(0, on(60)), (0, on(64)), (0, on(67)), (2, off(64))],
            3,
        );
        assert_eq!(frames[0], [(1.0, hz(60)), (1.0, hz(64)), (1.0, hz(67))]);
        // The released voice holds its hz.
        assert_eq!(frames[2][1], (0.0, hz(64)));
        assert_eq!(frames[3][0], (1.0, hz(60)));
    }

    #[test]
    fn rotate_takes_the_voice_released_longest_ago() {
        // v0 then v1 released; the next note goes to v0.
        let frames = run(
            2.0,
            false,
            4,
            &[
                (0, on(60)),
                (0, on(62)),
                (1, off(60)),
                (2, off(62)),
                (3, on(65)),
            ],
            2,
        );
        assert_eq!(frames[3], [(1.0, hz(65)), (0.0, hz(62))]);
    }

    #[test]
    fn a_steal_takes_the_oldest_note_with_one_frame_low() {
        let frames = run(2.0, false, 4, &[(0, on(60)), (1, on(62)), (2, on(65))], 2);
        // Frame 2: v0 (oldest) claimed: gate 0, still its old hz.
        assert_eq!(frames[2][0], (0.0, hz(60)));
        assert_eq!(frames[3][0], (1.0, hz(65)));
        assert_eq!(frames[3][1], (1.0, hz(62)));
        // The stolen note's off is ignored.
        let frames = run(
            2.0,
            false,
            5,
            &[(0, on(60)), (1, on(62)), (2, on(65)), (4, off(60))],
            2,
        );
        assert_eq!(frames[4][0], (1.0, hz(65)));
    }

    #[test]
    fn legato_takes_a_note_without_a_gap() {
        let frames = run(1.0, true, 3, &[(0, on(60)), (1, on(62))], 1);
        assert_eq!(frames[1][0], (1.0, hz(62)));
        let frames = run(1.0, false, 3, &[(0, on(60)), (1, on(62))], 1);
        assert_eq!(frames[1][0], (0.0, hz(60)));
        assert_eq!(frames[2][0], (1.0, hz(62)));
    }

    #[test]
    fn an_off_on_the_claim_frame_cancels_the_new_note() {
        let frames = run(1.0, false, 3, &[(0, on(60)), (1, on(62)), (1, off(62))], 1);
        assert_eq!(frames[1][0].0, 0.0);
        assert_eq!(frames[2][0].0, 0.0, "62 never sounds");
    }

    #[test]
    fn a_trigger_frees_every_voice() {
        let frames = run(
            2.0,
            false,
            2,
            &[(0, on(60)), (0, on(62)), (1, EventPayload::Trigger)],
            2,
        );
        assert_eq!(frames[1], [(0.0, hz(60)), (0.0, hz(62))]);
    }

    #[test]
    fn fewer_voices_silence_the_rest_and_more_reset_them() {
        let mut alloc = VoiceAlloc::default();
        let step = |alloc: &mut VoiceAlloc, voices: f64, events: &[EventPayload]| {
            let these: Vec<Event> = events
                .iter()
                .map(|p| Event {
                    offset: 0,
                    payload: *p,
                })
                .collect();
            let ends = [these.len()];
            let params = [voices, 0.0];
            let inputs = Inputs::new(1, &[], &params, &these, &ends);
            let mut rows = vec![0.0; 3 * MAX_VOICES];
            alloc.process(&inputs, &mut Outputs::new(1, &mut rows, &mut []));
            rows
        };
        step(&mut alloc, 2.0, &[on(60), on(62)]);
        let rows = step(&mut alloc, 1.0, &[]);
        assert_eq!(
            (rows[MAX_VOICES + 1], rows[1]),
            (0.0, hz(62)),
            "silent, hz held"
        );
        let rows = step(&mut alloc, 2.0, &[]);
        assert_eq!((rows[MAX_VOICES + 1], rows[1]), (0.0, 0.0), "reset");
        // The reset voice is taken first (order 0).
        let rows = step(&mut alloc, 2.0, &[on(64)]);
        assert_eq!(rows[1], hz(64));
    }

    #[test]
    fn hz_is_note_seqs() {
        assert_eq!(hz(69), 440.0);
        assert_eq!(
            read(EventPayload::Set(128.0 * 69.0 + 102.0)),
            Some(Note::On(69, 102.0 / 127.0))
        );
        assert_eq!(read(EventPayload::Set(128.0 * 128.0 + 1.0)), None);
        assert_eq!(read(EventPayload::Set(1.5)), None);
    }
}
