//! `std.note_seq` (ADR 0028 §6): a monophonic note sequencer playing a
//! `notes` pattern, counting `std.transport`'s ticks or following a
//! position in beats (`std.playhead`, ADR 0033).

use signal_core::{
    DirectPrimitive, Direction, Event, EventPayload, Feedthrough, Inputs, ModuleDescriptor,
    Outputs, PortKind, PrepareContext, Range, Table,
};

use crate::follow::{self, Motion};
use crate::pattern::NOTE_WIDTH;
use crate::{
    PPQ, SIGNAL_BOUND, event_port, ppq_at, round_half_up, signal_port, tick_count, triggered,
};

const TICK: usize = 0;
const RESET: usize = 1;
/// `follow`'s place among the descriptor's ports (for
/// [`Inputs::is_connected`]) and its signal input row.
const FOLLOW_PORT: usize = 2;
const FOLLOW: usize = 0;

const GATE: usize = 0;
const KEY: usize = 1;
const HZ: usize = 2;
const VELOCITY: usize = 3;
const POSITION: usize = 4;

const ON: usize = 0;
const OFF: usize = 1;

const PPQ_PARAMETER: usize = 0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.note_seq",
    doc: "Monophonic note sequencer: plays a `notes` pattern (pattern = \"<name>\"), counting transport ticks or following a position in beats (follow, from std.playhead); gate, key, hz, velocity, on/off events and a position playhead; loops over the pattern's length, last-note priority.",
    ports: &[
        event_port("tick", Direction::In),
        event_port("reset", Direction::In),
        signal_port(
            "follow",
            Direction::In,
            PortKind::Control,
            Some("beats"),
            None,
        ),
        signal_port(
            "gate",
            Direction::Out,
            PortKind::Gate,
            None,
            Some(Range { min: 0.0, max: 1.0 }),
        ),
        signal_port("key", Direction::Out, PortKind::Control, None, None),
        signal_port("hz", Direction::Out, PortKind::Frequency, Some("Hz"), None),
        signal_port(
            "velocity",
            Direction::Out,
            PortKind::Control,
            None,
            Some(Range { min: 0.0, max: 1.0 }),
        ),
        signal_port(
            "position",
            Direction::Out,
            PortKind::Control,
            Some("beats"),
            None,
        ),
        event_port("on", Direction::Out),
        event_port("off", Direction::Out),
    ],
    parameters: &[PPQ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// `std.note_seq`: plays the `notes` pattern its instance names
/// (`pattern = "<name>"`, `docs/project-format.md` §12) as one voice.
///
/// **Ports**
/// - `tick` (event in): `std.transport`'s `tick`. Counted by the `tick`
///   contract ([`Transport`](crate::Transport)): 1 per `Trigger`, *v* per
///   `Set(v)` (rounded half up, from 0 to 6400, the transport's largest);
///   `Ramp` counts nothing. So every
///   tick is counted, also when several share a frame.
/// - `reset` (event in, trigger): a frame with a `Trigger` ends the sounding
///   note (`off`) and makes the next tick position 0. It is applied before
///   the frame's ticks. Following a position, it is a seek (below): wire
///   `std.playhead.jump` into it so that every seek is one, a forward one
///   of less than a beat included.
/// - `follow` (signal in, beats; ADR 0033): a position to follow, such as
///   `std.playhead.position`. **Connected, it drives the sequencer instead
///   of `tick`** ("Following a position", below); a plan that connects both
///   `tick` and `follow` is refused. Unconnected, it changes nothing: the
///   ticks drive it exactly as without it.
/// - `gate` (signal out, 0 or 1): 1 while a note sounds.
/// - `key` (signal out): the key of the last note started (held after it
///   ends; 0 before the first).
/// - `hz` (signal out, Hz): `440·2^((key − 69)/12)` of that key (0 before
///   the first note).
/// - `velocity` (signal out, 0 to 1): that note's velocity (held; 0 before).
/// - `position` (signal out, beats): the current tick's place in the loop,
///   `min(p, T − 1) / ppq` with *T* and `ppq` of the frame (so a `ppq`
///   changed on a long loop never shows a place past the loop's end),
///   within ±1e6, for a playhead; 0 before the first tick and after a
///   reset.
/// - `on` (event out, trigger): on a frame where a note starts.
/// - `off` (event out, trigger): on a frame where the sounding note ends.
///
/// **Parameters**
/// - `ppq` (generic, ticks per beat, integer, range `[1, 960]`, default 96):
///   must equal the transport's `ppq`; read on every frame with ticks.
///   Following a position it is not used.
///
/// **Feedthrough:** direct (a tick on frame *n* changes the outputs at *n*).
///
/// **Formula.** The pattern is the instance's table (`length` *L* beats;
/// notes by start, then key). With `ppq` = *q*, the loop has `T =
/// max(1, round_half_up(L·q))` ticks; position *p* counts ticks: the first
/// tick after `prepare` or a reset is *p* = 0, each later tick *p* + 1, back
/// to 0 at *T*. A note fires on the tick `s = round_half_up(start·q)` and
/// lasts `max(1, round_half_up((start + length)·q) − s)` ticks, counted
/// tick by tick, so a note crossing the loop end ends in the next loop. A
/// note starting just before the loop's end can round to `s = T`, the end
/// itself: it **wraps** to tick 0 of the next loop (as `std.step_seq8`'s
/// steps wrap, from the second loop on, before the notes starting at 0, so
/// those take priority over it), lasting the same ticks, so no note that
/// starts before `length` is lost. On
/// each tick, the sounding note ends first (its last tick passed), then the
/// notes starting at *p* start: **last-note priority**, the last of them (the
/// highest key) takes over the voice (`key`, `hz`, `velocity`, `on`) and the
/// gate stays high; a note it takes over from does not come back. Without a
/// table the loop is 1 beat and nothing plays.
///
/// **Frame resolution.** Outputs are per frame: `on` and `off` fire at most
/// once each per frame. When ticks are denser than frames (below 640 Hz at
/// 96 ppq), several notes starting within one frame merge into one `on`, the
/// last one's; the signals show the state at the end of the frame. When a
/// note both starts and ends within a frame, `on` comes before `off`;
/// otherwise `off` (of the note that ended) comes first.
///
/// **Live edits.** A changed pattern arrives as a new table at a swap frame
/// (ADR 0028 §4); the position, the sounding note and its remaining ticks
/// carry on. A shorter loop wraps on the next tick.
///
/// **Following a position** (ADR 0033). With `follow` connected, each frame
/// reads the position *x(n)* and compares it with the frame before
/// (`crate::follow`, `docs/module-contract.md` "Following a position"):
/// - **Forward** by at most one beat, the beats `[x(n − 1), x(n))` are
///   crossed: in order of their beat, the sounding note ends when its end
///   `S + length` (*S* its start beat) is at or before a note starting in
///   them, and each note whose start `S = m·L + start` (loop *m*, *L* the
///   pattern's `length`) lies in them starts, last-note priority as above;
///   then the sounding note ends if its end lies before *x(n)*. So a note
///   fires on the first frame whose position is past its start, exact in
///   beats (no ticks, no `ppq` rounding), and ends on the first frame past
///   its end; the loop wraps where the position crosses a multiple of *L*,
///   and a note crossing the loop's end ends in the next loop. A note
///   starts at most once: consecutive frames share their bound.
/// - **Not moving** (a stopped or held transport) fires nothing and ends
///   the sounding note: a stopped transport releases its note.
/// - **A seek** (a `reset`, which `std.playhead.jump` marks on every seek;
///   and, inferred from the motion, backwards, more than one beat forward,
///   more than 64 loops in one frame, the first frame after `prepare`, or a
///   swap that connected `follow`): nothing fires retroactively; the sounding
///   note ends (`off` on this frame), and the next frame resumes from the
///   new place: a note that starts exactly there fires, **a note that spans
///   it is not restarted** (it began before the place, so playing it would
///   mean starting a note late, cut at its start; the next note plays on
///   time). A transport playing backwards therefore plays nothing.
///
/// `position` then shows the place of *x(n)* in the loop, `x − ⌊x/L⌋·L`,
/// exact. The `ppq` parameter is not read. A swap that disconnects
/// `follow` ends the sounding note and goes on counting ticks from the tick
/// position it had.
///
/// **Cost.** Ticks are processed one by one: a binary search over the notes
/// per tick, so a frame costs in proportion to the ticks it holds (the
/// transport puts at most ⌊bpm·ppq / (60·sr)⌋ + 1 on a frame: 1 at 48 kHz
/// up to 400 bpm and 960 ppq, 6400 at the lowest rate, 1 Hz). The count of one `Set` is capped at 6400, the
/// transport's largest, so the work per call stays bounded; driving `tick`
/// from another stream of large `Set` values (not a transport) is a misuse
/// that costs up to 6400 ticks' work per event (`module-contract.md`, "The
/// tick clock and the sequencers").
#[derive(Clone, Debug, Default)]
pub struct NoteSeq {
    /// The position in ticks, once counting.
    position: Option<u64>,
    /// The sounding note's ticks left.
    remaining: u64,
    /// Whether the position has wrapped since `prepare` or a reset: only
    /// then do notes wrapped from the loop's end start (there was no loop
    /// before the first).
    looped: bool,
    key: f64,
    hz: f64,
    velocity: f64,
    /// Following a position: the previous frame's, if any.
    last: Option<f64>,
    /// Following a position: the sounding note's end, in beats of the
    /// followed position.
    end: Option<f64>,
}

/// The first note row (from 1) of the loop starting at beat `base` whose
/// start beat `base + start` is at least `from`.
fn first_from(table: &Table, base: f64, from: f64) -> usize {
    let at = |row: usize| base + table.row(row).first().copied().unwrap_or(0.0);
    let (mut lo, mut hi) = (1, table.rows());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if at(mid) < from {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The first note row (from 1) whose start tick is at least `p`.
fn first_at(table: &Table, p: u64, ppq: f64) -> usize {
    let tick = |row: usize| round_half_up(table.row(row).first().copied().unwrap_or(0.0) * ppq);
    let (mut lo, mut hi) = (1, table.rows());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if tick(mid) < p as f64 {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The loop's length in ticks with `ppq`: `max(1, round_half_up(L·ppq))`
/// (1 beat without a table).
fn loop_ticks(table: Option<&Table>, ppq: f64) -> u64 {
    let length = table.map_or(1.0, |t| t.row(0).first().copied().unwrap_or(1.0));
    (round_half_up(length * ppq) as u64).max(1)
}

/// What one frame's ticks did.
#[derive(Default)]
struct Frame {
    started: bool,
    ended: bool,
}

impl NoteSeq {
    /// One tick: advance, end the sounding note if its time is up, start the
    /// notes at the new position.
    fn tick(&mut self, table: Option<&Table>, ppq: f64, frame: &mut Frame) {
        let loop_ticks = loop_ticks(table, ppq);
        let p = match self.position {
            Some(p) if p + 1 < loop_ticks => p + 1,
            Some(_) => {
                self.looped = true;
                0
            }
            None => 0,
        };
        self.position = Some(p);
        if self.remaining > 0 {
            self.remaining -= 1;
            if self.remaining == 0 {
                frame.ended = true;
            }
        }
        let Some(table) = table else {
            return;
        };
        // The notes starting on tick `at`, the last of them kept.
        let mut last = None;
        let mut starting = |at: u64| {
            let mut row = first_at(table, at, ppq);
            while let [start, len, key, velocity, hz] = *table.row(row) {
                if round_half_up(start * ppq) != at as f64 {
                    break;
                }
                last = Some((start, len, key, velocity, hz));
                row += 1;
            }
        };
        // Notes rounding to the loop's end wrap to its first tick (from the
        // second loop on), before the notes starting there.
        if p == 0 && self.looped {
            starting(loop_ticks);
        }
        starting(p);
        if let Some((start, len, key, velocity, hz)) = last {
            let from = round_half_up(start * ppq);
            let to = round_half_up((start + len) * ppq);
            self.remaining = ((to - from) as u64).max(1);
            (self.key, self.hz, self.velocity) = (key, hz, velocity);
            frame.started = true;
        }
    }
}

impl NoteSeq {
    /// Whether a note sounds, counted in ticks or followed in beats.
    fn sounding(&self) -> bool {
        self.remaining > 0 || self.end.is_some()
    }

    /// Ends the sounding note, if any.
    fn silence(&mut self, frame: &mut Frame) {
        frame.ended |= self.sounding();
        self.remaining = 0;
        self.end = None;
    }

    /// Following a position: the beats `[from, to)` of a forward motion.
    fn cross(
        &mut self,
        table: Option<&Table>,
        period: f64,
        (from, to): (f64, f64),
        frame: &mut Frame,
    ) {
        if let Some(table) = table {
            for base in follow::loops(from, to, period) {
                let mut row = first_from(table, base, from);
                while let [start, len, key, velocity, hz] = *table.row(row) {
                    let at = base + start;
                    if at >= to {
                        break;
                    }
                    if self.end.is_some_and(|end| end <= at) {
                        // An `off` only for a note that sounded when the
                        // frame began: one that started and ended within
                        // this frame is merged into its `on`.
                        frame.ended |= !frame.started;
                        self.end = None;
                    }
                    self.end = Some(at + len);
                    (self.key, self.hz, self.velocity) = (key, hz, velocity);
                    frame.started = true;
                    row += 1;
                }
            }
        }
        if self.end.is_some_and(|end| end < to) {
            frame.ended = true;
            self.end = None;
        }
    }

    /// `process` with `follow` connected.
    fn follow(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>, table: Option<&Table>) {
        let positions = inputs.signal(FOLLOW);
        let period =
            follow::period(table.map_or(1.0, |t| t.row(0).first().copied().unwrap_or(1.0)));
        let mut next_reset = 0;
        for n in 0..inputs.frames() {
            let now = positions.get(n).copied().unwrap_or(0.0);
            let mut frame = Frame::default();
            let motion = if triggered(inputs.events(RESET), &mut next_reset, n) {
                Motion::Seek
            } else {
                follow::motion(self.last, now, period)
            };
            match motion {
                Motion::Seek | Motion::Still => self.silence(&mut frame),
                Motion::Forward { from, to } => self.cross(table, period, (from, to), &mut frame),
            }
            self.last = now.is_finite().then_some(now);
            let position = follow::place(now, period).clamp(-SIGNAL_BOUND, SIGNAL_BOUND);
            self.write(outputs, n, &frame, position);
        }
    }

    /// Frame `n`'s events and signals.
    fn write(&self, outputs: &mut Outputs<'_>, n: usize, frame: &Frame, position: f64) {
        let sounding = self.sounding();
        match (frame.ended, frame.started) {
            (true, true) if sounding => {
                push(outputs, OFF, n);
                push(outputs, ON, n);
            }
            (true, true) => {
                push(outputs, ON, n);
                push(outputs, OFF, n);
            }
            (true, false) => push(outputs, OFF, n),
            (false, true) => push(outputs, ON, n),
            (false, false) => {}
        }
        let values = [
            f64::from(u8::from(sounding)),
            self.key,
            self.hz,
            self.velocity,
            position,
        ];
        for (port, value) in [GATE, KEY, HZ, VELOCITY, POSITION].into_iter().zip(values) {
            if let Some(out) = outputs.signal(port).get_mut(n) {
                *out = value;
            }
        }
    }
}

fn push(outputs: &mut Outputs<'_>, port: usize, n: usize) {
    if let Some(sink) = outputs.events(port) {
        sink.push(Event {
            offset: u32::try_from(n).unwrap_or(u32::MAX),
            payload: EventPayload::Trigger,
        });
    }
}

impl DirectPrimitive for NoteSeq {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        *self = NoteSeq::default();
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let table = inputs.table().filter(|t| t.width() == NOTE_WIDTH);
        if inputs.is_connected(FOLLOW_PORT) {
            return self.follow(inputs, outputs, table);
        }
        // Counting ticks. A note followed in beats (before a swap that
        // disconnected `follow`) ends on the first frame.
        self.last = None;
        let followed = self.end.take().is_some();
        let ticks = inputs.events(TICK);
        let (mut next_tick, mut next_reset) = (0, 0);
        for n in 0..inputs.frames() {
            let ppq = ppq_at(inputs.parameter(PPQ_PARAMETER), n) as f64;
            let mut frame = Frame {
                ended: followed && n == 0,
                ..Frame::default()
            };
            if triggered(inputs.events(RESET), &mut next_reset, n) {
                frame.ended |= self.remaining > 0;
                self.remaining = 0;
                self.position = None;
                self.looped = false;
            }
            while let Some(event) = ticks.get(next_tick) {
                if event.offset as usize > n {
                    break;
                }
                if event.offset as usize == n {
                    for _ in 0..tick_count(event.payload) {
                        self.tick(table, ppq, &mut frame);
                    }
                }
                next_tick += 1;
            }
            let position = self.position.map_or(0.0, |p| {
                let last = loop_ticks(table, ppq) - 1;
                (p.min(last) as f64 / ppq).clamp(-SIGNAL_BOUND, SIGNAL_BOUND)
            });
            self.write(outputs, n, &frame, position);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::compile_notes;
    use crate::testing::context;
    use signal_core::EventSink;

    fn notes(length: f64, notes: &[[f64; 4]]) -> Table {
        let source = Table::new(4, notes.concat()).unwrap();
        compile_notes(&[length], &source).unwrap()
    }

    /// Runs `frames` frames with a tick on every frame in `ticks` (one per
    /// frame, or the count in a `Set`), returning per frame (gate, key,
    /// position) and the on/off frames.
    fn run(
        table: Option<&Table>,
        ppq: f64,
        ticks: &[(u32, EventPayload)],
        frames: usize,
    ) -> (Vec<[f64; 3]>, Vec<u64>, Vec<u64>) {
        let mut seq = NoteSeq::default();
        seq.prepare(&context(48_000.0, frames, 1));
        let events: Vec<Event> = ticks
            .iter()
            .map(|&(offset, payload)| Event { offset, payload })
            .collect();
        let ends = [events.len(), events.len()];
        let parameters = vec![ppq; frames];
        let mut signals = vec![0.0; 5 * frames];
        let mut sinks = vec![EventSink::per_frame(16, frames); 2];
        for sink in &mut sinks {
            sink.start_call(frames);
        }
        seq.process(
            &Inputs::new(frames, &[], &parameters, &events, &ends).with_table(table),
            &mut Outputs::new(frames, &mut signals, &mut sinks),
        );
        let state = (0..frames)
            .map(|n| [signals[n], signals[frames + n], signals[4 * frames + n]])
            .collect();
        let at = |sink: &EventSink| sink.events().iter().map(|e| u64::from(e.offset)).collect();
        (state, at(&sinks[0]), at(&sinks[1]))
    }

    fn every_frame(frames: u32) -> Vec<(u32, EventPayload)> {
        (0..frames).map(|n| (n, EventPayload::Trigger)).collect()
    }

    /// At 2 ticks per beat: a note at beat 0 for 1 beat, a later one at 1.5
    /// crossing the 2-beat loop end, and the loop wrap.
    #[test]
    fn notes_loop_and_cross_the_loop_end() {
        let table = notes(2.0, &[[60.0, 0.0, 1.0, 0.5], [64.0, 1.5, 1.0, 1.0]]);
        let (state, on, off) = run(Some(&table), 2.0, &every_frame(10), 10);
        // Ticks 0..9: positions 0,1,2,3,0,1,2,3,0,1.
        let positions: Vec<f64> = state.iter().map(|s| s[2]).collect();
        assert_eq!(
            positions,
            [0.0, 0.5, 1.0, 1.5, 0.0, 0.5, 1.0, 1.5, 0.0, 0.5]
        );
        // 60 on at 0, off at 2; 64 on at 3, crossing the end, off at 5; but 60
        // starts again at 4 and takes over (last-note priority), ends at 6.
        assert_eq!(on, [0, 3, 4, 7, 8]);
        assert_eq!(off, [2, 6]);
        let keys: Vec<f64> = state.iter().map(|s| s[1]).collect();
        assert_eq!(
            keys,
            [60.0, 60.0, 60.0, 64.0, 60.0, 60.0, 60.0, 64.0, 60.0, 60.0]
        );
        let gates: Vec<f64> = state.iter().map(|s| s[0]).collect();
        assert_eq!(gates, [1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0]);
    }

    /// Ticks counted from a `Trigger` and a `Set(n − 1)` on one frame are all
    /// counted; notes starting within the frame merge into one `on`.
    #[test]
    fn dense_ticks_are_counted_and_merge_per_frame() {
        let table = notes(4.0, &[[60.0, 0.0, 0.25, 1.0], [62.0, 0.5, 0.25, 1.0]]);
        let ticks = [
            (0, EventPayload::Trigger),
            (0, EventPayload::Set(7.0)),
            (1, EventPayload::Trigger),
        ];
        let (state, on, off) = run(Some(&table), 4.0, &ticks, 2);
        // 8 ticks on frame 0: positions 0..7 (1.75 beats); note 60 at 0 for
        // 1 tick, note 62 at tick 2 for 1 tick: both ended within the frame.
        assert_eq!(state[0], [0.0, 62.0, 1.75]);
        assert_eq!((on, off), (vec![0], vec![0]));
        assert_eq!(state[1][2], 2.0);
    }

    /// A reset on a tick's frame applies first: that tick is position 0.
    #[test]
    fn reset_restarts_the_loop() {
        let table = notes(4.0, &[[60.0, 0.0, 4.0, 1.0]]);
        let mut seq = NoteSeq::default();
        seq.prepare(&context(48_000.0, 4, 1));
        let events = [
            Event {
                offset: 0,
                payload: EventPayload::Trigger,
            },
            Event {
                offset: 1,
                payload: EventPayload::Trigger,
            },
            Event {
                offset: 2,
                payload: EventPayload::Trigger,
            },
            Event {
                offset: 2,
                payload: EventPayload::Trigger,
            },
        ];
        let ends = [3, 4];
        let mut signals = vec![0.0; 5 * 4];
        let mut sinks = vec![EventSink::per_frame(16, 4); 2];
        seq.process(
            &Inputs::new(4, &[], &[1.0; 4], &events, &ends).with_table(Some(&table)),
            &mut Outputs::new(4, &mut signals, &mut sinks),
        );
        assert_eq!(signals[16..20], [0.0, 1.0, 0.0, 0.0]);
        let on: Vec<u32> = sinks[0].events().iter().map(|e| e.offset).collect();
        let off: Vec<u32> = sinks[1].events().iter().map(|e| e.offset).collect();
        assert_eq!((on, off), (vec![0, 2], vec![2]));
    }

    /// L2: a note whose start rounds to the loop's tick count (3.995 of 4
    /// beats at 96 ppq: tick 384 = T) wraps to tick 0 of the next loop, from
    /// the second loop on, lasting its ticks; notes starting at 0 take over
    /// from it on that tick.
    #[test]
    fn a_note_rounding_to_the_loop_end_wraps_to_the_next_loop() {
        let table = notes(4.0, &[[64.0, 3.995, 0.25, 1.0]]);
        let (state, on, off) = run(Some(&table), 96.0, &every_frame(800), 800);
        // 384 ticks a loop; the note from tick 384 − 384 = 0 of each later
        // loop, for round(4.245·96) − 384 = 24 ticks.
        assert_eq!(on, [384, 768]);
        assert_eq!(off, [408, 792]);
        assert_eq!(state[384], [1.0, 64.0, 0.0]);
        assert_eq!(state[383][0], 0.0, "not in the first loop");

        let both = notes(4.0, &[[60.0, 0.0, 0.5, 1.0], [64.0, 3.995, 0.25, 1.0]]);
        let (state, on, _) = run(Some(&both), 96.0, &every_frame(400), 400);
        assert_eq!(on, [0, 384]);
        assert_eq!(state[384][1], 60.0, "the note at 0 takes over");
        assert_eq!(state[0][1], 60.0);
    }

    /// L1: `position` stays within the loop when `ppq` changes on a long
    /// loop: `min(p, T − 1) / ppq` with the frame's `ppq`.
    #[test]
    fn the_position_stays_in_the_loop_when_ppq_changes() {
        let table = notes(8.0, &[[60.0, 0.0, 1.0, 1.0]]);
        let mut seq = NoteSeq::default();
        seq.prepare(&context(48_000.0, 3, 1));
        // 701 ticks on frame 0 (p = 700 of 768 at 96 ppq), none on frame 1
        // (ppq 1: T = 8), one on frame 2 (it wraps to 0).
        let events = [
            Event {
                offset: 0,
                payload: EventPayload::Trigger,
            },
            Event {
                offset: 0,
                payload: EventPayload::Set(700.0),
            },
            Event {
                offset: 2,
                payload: EventPayload::Trigger,
            },
        ];
        let ends = [3, 3];
        let mut signals = vec![0.0; 5 * 3];
        let mut sinks = vec![EventSink::per_frame(16, 3); 2];
        seq.process(
            &Inputs::new(3, &[], &[96.0, 1.0, 1.0], &events, &ends).with_table(Some(&table)),
            &mut Outputs::new(3, &mut signals, &mut sinks),
        );
        assert_eq!(signals[12..15], [700.0 / 96.0, 7.0, 0.0]);
    }
}
