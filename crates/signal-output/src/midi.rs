//! MIDI control input (ADR 0057 §1, §3, §4): a `midi` slot's values from a
//! MIDI byte stream, in safe Rust. The bytes come from CoreMIDI's thread
//! (`signal-audio/src/midi_in.rs`, the audited FFI module) through a ring;
//! [`MidiState`] reads them (running status included) into the slot's
//! named values, and [`MidiSource`], an [`InputSource`], hands those to the
//! engine at the start of each block and holds them, so a session records
//! what was read and a replay is exact.
//!
//! The names a `midi` slot may capture ([`MIDI_NAMES`]): `note_gate`,
//! `note_key`, `note_velocity` (the last note held, last-note priority:
//! releasing it returns to the one held before), `cc0` … `cc127` (0–1),
//! `bend` (−1–1), and from a MIDI clock (24 ticks a quarter note)
//! `clock_position` (beats since its start), `clock_running` (0/1) and
//! `clock_tempo` (bpm, from the ticks of the last beat).

use std::time::Instant;

use rtrb::Consumer;

use crate::inputs::InputSource;

/// The fixed names a `midi` slot captures, besides `cc0` … `cc127`.
pub const MIDI_NAMES: [&str; 7] = [
    "note_gate",
    "note_key",
    "note_velocity",
    "bend",
    "clock_position",
    "clock_running",
    "clock_tempo",
];

/// Whether `name` is a value a `midi` slot can capture.
pub fn is_midi_name(name: &str) -> bool {
    MIDI_NAMES.contains(&name)
        || name
            .strip_prefix("cc")
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| n < 128 && name == format!("cc{n}"))
}

/// A value's place in [`MidiState::values`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Value {
    Gate,
    Key,
    Velocity,
    Bend,
    Position,
    Running,
    Tempo,
    Cc(u8),
}

fn value_of(name: &str) -> Option<Value> {
    Some(match name {
        "note_gate" => Value::Gate,
        "note_key" => Value::Key,
        "note_velocity" => Value::Velocity,
        "bend" => Value::Bend,
        "clock_position" => Value::Position,
        "clock_running" => Value::Running,
        "clock_tempo" => Value::Tempo,
        cc => Value::Cc(
            cc.strip_prefix("cc")?
                .parse()
                .ok()
                .filter(|n: &u8| *n < 128)?,
        ),
    })
}

/// The most notes held at once that the state remembers (older ones drop).
const HELD: usize = 16;

/// What a MIDI stream has said so far, and the slot's values from it.
#[derive(Clone, Debug)]
pub struct MidiState {
    names: Vec<Value>,
    /// The slot's values, in its order.
    pub values: Vec<f32>,
    status: u8,
    data: [u8; 2],
    have: usize,
    held: Vec<(u8, u8)>,
    ticks: u64,
    running: bool,
    /// When the last beat's first tick came (control thread only: a tempo).
    beat_started: Option<Instant>,
}

impl MidiState {
    /// The state of a slot capturing `names` (each a [`MIDI_NAMES`] or
    /// `cc<n>` name; any other reads 0).
    pub fn new(names: &[String]) -> MidiState {
        let names = names
            .iter()
            .map(|n| value_of(n).unwrap_or(Value::Cc(255)))
            .collect::<Vec<_>>();
        let values = vec![0.0; names.len()];
        MidiState {
            names,
            values,
            status: 0,
            data: [0; 2],
            have: 0,
            held: Vec::with_capacity(HELD),
            ticks: 0,
            running: false,
            beat_started: None,
        }
    }

    fn set(&mut self, which: Value, value: f32) {
        for (name, slot) in self.names.iter().zip(&mut self.values) {
            if *name == which {
                *slot = value;
            }
        }
    }

    fn note_values(&mut self) {
        match self.held.last().copied() {
            Some((key, velocity)) => {
                self.set(Value::Gate, 1.0);
                self.set(Value::Key, f32::from(key));
                self.set(Value::Velocity, f32::from(velocity) / 127.0);
            }
            None => self.set(Value::Gate, 0.0),
        }
    }

    /// Reads one byte of the stream at `now` (for the clock's tempo).
    pub fn byte(&mut self, byte: u8, now: Instant) {
        if byte >= 0xF8 {
            self.realtime(byte, now);
            return;
        }
        if byte & 0x80 != 0 {
            // A status byte; system common messages cancel running status.
            self.status = if byte < 0xF0 { byte } else { 0 };
            self.have = 0;
            return;
        }
        if self.status == 0 {
            return;
        }
        if let Some(slot) = self.data.get_mut(self.have) {
            *slot = byte;
        }
        self.have += 1;
        let needed = match self.status & 0xF0 {
            0xC0 | 0xD0 => 1,
            _ => 2,
        };
        if self.have < needed {
            return;
        }
        self.have = 0;
        let [a, b] = self.data;
        match self.status & 0xF0 {
            0x90 if b > 0 => {
                self.held.retain(|(key, _)| *key != a);
                if self.held.len() == HELD {
                    self.held.remove(0);
                }
                self.held.push((a, b));
                self.note_values();
            }
            0x80 | 0x90 => {
                self.held.retain(|(key, _)| *key != a);
                self.note_values();
            }
            0xB0 => self.set(Value::Cc(a), f32::from(b) / 127.0),
            0xE0 => {
                let raw = i32::from(a) | (i32::from(b) << 7);
                self.set(Value::Bend, ((raw - 8192) as f32 / 8192.0).clamp(-1.0, 1.0));
            }
            _ => {}
        }
    }

    fn realtime(&mut self, byte: u8, now: Instant) {
        match byte {
            // Clock: 24 ticks a quarter note, counted while running.
            0xF8 if self.running => {
                if self.ticks.is_multiple_of(24) {
                    if let Some(start) = self.beat_started {
                        let seconds = now.duration_since(start).as_secs_f64();
                        if seconds > 0.0 {
                            self.set(Value::Tempo, (60.0 / seconds) as f32);
                        }
                    }
                    self.beat_started = Some(now);
                }
                self.ticks += 1;
                self.set(Value::Position, self.ticks as f32 / 24.0);
            }
            // Start: from the beginning.
            0xFA => {
                self.ticks = 0;
                self.running = true;
                self.beat_started = None;
                self.set(Value::Position, 0.0);
                self.set(Value::Running, 1.0);
            }
            // Continue: from where it stopped.
            0xFB => {
                self.running = true;
                self.beat_started = None;
                self.set(Value::Running, 1.0);
            }
            0xFC => {
                self.running = false;
                self.set(Value::Running, 0.0);
            }
            _ => {}
        }
    }
}

/// The audio thread's end of a `midi` slot: the values the control thread
/// computed, applied at the start of each read and held.
pub struct MidiSource {
    rx: Consumer<(u16, f32)>,
    values: Vec<f32>,
}

impl MidiSource {
    /// A source of `width` values fed by `rx` (index, value).
    pub fn new(rx: Consumer<(u16, f32)>, width: usize) -> MidiSource {
        MidiSource {
            rx,
            values: vec![0.0; width],
        }
    }
}

impl InputSource for MidiSource {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| (*n).to_owned()).collect()
    }

    fn feed(state: &mut MidiState, bytes: &[u8]) {
        let now = Instant::now();
        for &b in bytes {
            state.byte(b, now);
        }
    }

    #[test]
    fn the_names_are_the_vocabulary_and_the_controllers() {
        assert!(is_midi_name("note_gate") && is_midi_name("cc0") && is_midi_name("cc127"));
        assert!(!is_midi_name("cc128") && !is_midi_name("cc07") && !is_midi_name("volume"));
    }

    #[test]
    fn notes_hold_the_last_and_return_to_the_one_before() {
        let mut s = MidiState::new(&names(&["note_gate", "note_key", "note_velocity"]));
        feed(&mut s, &[0x90, 60, 127]);
        assert_eq!(s.values, [1.0, 60.0, 1.0]);
        // Running status: a second note without its status byte.
        feed(&mut s, &[64, 64]);
        assert_eq!(s.values[1], 64.0);
        feed(&mut s, &[0x80, 64, 0]);
        assert_eq!(s.values[..2], [1.0, 60.0], "back to the note held before");
        feed(&mut s, &[0x90, 60, 0]);
        assert_eq!(s.values[0], 0.0, "a note-on of velocity 0 is a release");
    }

    #[test]
    fn controllers_and_bend_are_scaled() {
        let mut s = MidiState::new(&names(&["cc1", "cc74", "bend"]));
        feed(&mut s, &[0xB0, 1, 127, 74, 0, 0xE0, 0, 64]);
        assert_eq!(s.values, [1.0, 0.0, 0.0]);
        feed(&mut s, &[0xE0, 0x7F, 0x7F]);
        assert!((s.values[2] - 1.0).abs() < 1e-3);
        // A clock byte inside a message does not break it.
        feed(&mut s, &[0xB0, 74, 0xF8, 64]);
        assert!((s.values[1] - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn a_clock_is_a_position_a_running_gate_and_a_tempo() {
        let mut s = MidiState::new(&names(&["clock_position", "clock_running", "clock_tempo"]));
        let t0 = Instant::now();
        s.byte(0xFA, t0);
        assert_eq!(s.values[..2], [0.0, 1.0]);
        // 48 ticks, half a second a beat: 120 bpm.
        for i in 0..49u64 {
            s.byte(0xF8, t0 + Duration::from_micros(i * 500_000 / 24));
        }
        assert!((s.values[0] - 49.0 / 24.0).abs() < 1e-6);
        assert!((s.values[2] - 120.0).abs() < 0.5, "tempo {}", s.values[2]);
        s.byte(0xFC, t0);
        assert_eq!(s.values[1], 0.0);
        s.byte(0xF8, t0);
        assert!(
            (s.values[0] - 49.0 / 24.0).abs() < 1e-6,
            "stopped: no ticks counted"
        );
    }
}
