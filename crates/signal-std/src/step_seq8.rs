//! `std.step_seq8` (ADR 0028 §6): a drum-machine step sequencer playing a
//! `steps` pattern of up to 8 lanes, counting `std.transport`'s ticks or
//! following a position in beats (`std.playhead`, ADR 0033).

use signal_core::{
    Combine, DirectPrimitive, Direction, Event, EventPayload, Feedthrough, Inputs,
    ModuleDescriptor, Outputs, PortKind, PrepareContext, Range, Table,
};

use crate::follow::{self, Motion};
use crate::pattern::{LANES, STEP_WIDTH};
use crate::{
    PPQ, event_port, parameter, ppq_at, round_half_up, signal_port, tick_count, triggered,
};

const TICK: usize = 0;
const RESET: usize = 1;
/// `follow`'s place among the descriptor's ports (for
/// [`Inputs::is_connected`]) and its signal input row.
const FOLLOW_PORT: usize = 2;
const FOLLOW: usize = 0;

/// Event outputs: `t1` … `t8` are 0 … 7.
const ACCENT_OUT: usize = LANES;
const STEP: usize = 0;

const PPQ_PARAMETER: usize = 0;
const ACCENT_LEVEL: usize = 1;
const SWING: usize = 2;

const DEFAULT_ACCENT: f64 = 1.5;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.step_seq8",
    doc: "Drum-machine step sequencer: plays a `steps` pattern (pattern = \"<name>\") of up to 8 lanes, counting transport ticks or following a position in beats (follow, from std.playhead); per-lane triggers t1..t8 carrying the lane's level (x accent on accented steps), an accent trigger, swing and a step playhead.",
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
        event_port("t1", Direction::Out),
        event_port("t2", Direction::Out),
        event_port("t3", Direction::Out),
        event_port("t4", Direction::Out),
        event_port("t5", Direction::Out),
        event_port("t6", Direction::Out),
        event_port("t7", Direction::Out),
        event_port("t8", Direction::Out),
        event_port("accent", Direction::Out),
        signal_port(
            "step",
            Direction::Out,
            PortKind::Generic,
            None,
            Some(Range {
                min: 0.0,
                max: 255.0,
            }),
        ),
    ],
    parameters: &[
        PPQ,
        parameter(
            "accent_level",
            PortKind::Gain,
            None,
            0.0,
            4.0,
            DEFAULT_ACCENT,
            Combine::Add,
        ),
        parameter(
            "swing",
            PortKind::Control,
            Some("%"),
            0.0,
            75.0,
            0.0,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// `std.step_seq8`: plays the `steps` pattern its instance names
/// (`pattern = "<name>"`, `docs/project-format.md` §12), lane *i* on `ti`.
///
/// **Ports**
/// - `tick` (event in): `std.transport`'s `tick`, counted by the `tick`
///   contract ([`Transport`](crate::Transport)): 1 per `Trigger`, *v* per
///   `Set(v)` (at most 6400, the transport's largest), so every tick counts,
///   also several on one frame.
/// - `reset` (event in, trigger): the next tick is position 0 (step 1);
///   applied before the frame's ticks. Following a position, it is a seek
///   (below): wire `std.playhead.jump` into it, so that a forward seek of
///   less than a beat does not fire the steps it skips.
/// - `follow` (signal in, beats; ADR 0033): a position to follow, such as
///   `std.playhead.position`. **Connected, it drives the sequencer instead
///   of `tick`** ("Following a position", below); a plan that connects both
///   `tick` and `follow` is refused. Unconnected, it changes nothing.
/// - `t1` … `t8` (event out): on a frame where lane *i*'s step fires, a
///   `Set(level)` (the lane's `level`, times `accent_level` on an accented step,
///   `X`), then a `Trigger`, so envelopes and exciters fire on it (they take
///   `Trigger`s and skip `Set`s) and value readers get the level. A lane
///   wired into a parameter sets its base to the level and the `Trigger` is a
///   logged discard (one per hit, counted, never a fault); `std.count` counts
///   both events (its value sum is level + 1). Outputs past the pattern's
///   lanes stay silent.
/// - `accent` (event out, trigger): on a frame where a step with an accented
///   lane fires.
/// - `step` (signal out, 0 to 255): the playhead, which shows the **loop
///   position**: on each tick, the index from 0 of the latest step whose
///   grid tick `g(i)` is at or before the current tick (0 before the first
///   tick). It follows the grid, not the hits: swing does not move it, and
///   a step wrapped from the loop's end (below) fires on the next loop's
///   tick without moving the playhead back, so on a tick where wrapped hits
///   share the tick with the next loop's steps it shows the loop position,
///   the next loop's step.
///
/// **Parameters**
/// - `ppq` (generic, ticks per beat, integer, range `[1, 960]`, default 96):
///   must equal the transport's.
/// - `accent_level` (gain, range `[0, 4]`, default 1.5): the factor of an
///   accented hit's level (ADR 0028 §6's "accent parameter"; named apart from
///   the `accent` output because a type's ports and parameters share one
///   namespace).
/// - `swing` (control, %, range `[0, 75]`, default 0): delays the even steps
///   (2, 4, …; odd indices from 0) by `min(round_half_up(swing/100 · d),
///   d − 1)` ticks, `d = round_half_up(step·ppq)`: a whole number of ticks.
///
/// All three are read on each frame with ticks.
///
/// **Feedthrough:** direct.
///
/// **Formula.** With `steps` *S*, `step` *s* beats and *q* = `ppq`, step *i*
/// is on tick `g(i) = round_half_up(i·s·q)`, fires on `g(i)` (even *i*) or
/// `g(i) + swing delay` (odd *i*), and the loop has `L = max(1,
/// round_half_up(S·s·q))` ticks. A firing tick at or past `L` (possible only
/// when `s·q` is not a whole number of ticks: steps that round up to the
/// loop end, or the last step delayed by swing) **wraps**: the step fires on
/// tick `firing − L` of the next loop, where it falls in time, so no step is
/// lost (from the second loop on: before the first there is no loop end). A
/// whole number of ticks per step never wraps. The position counts ticks as
/// `std.note_seq`'s does (0 on the first tick after `prepare` or a reset,
/// back to 0 at the loop's end). Without a table nothing fires and the loop
/// is 1 beat.
///
/// **Following a position** (ADR 0033). With `follow` connected, each frame
/// compares the position *x(n)* with the frame before, as `std.note_seq`
/// does (`crate::follow`, `docs/module-contract.md` "Following a
/// position"): forward by at most one beat, every step whose beat
/// `m·L + i·s + (swing delay if i is odd)` (loop *m*, `L = S·s`) lies in
/// `[x(n − 1), x(n))` fires on this frame, exact in beats, with the swing
/// delay `swing/100 · s` beats (not rounded to ticks; below `s`, so no step
/// leaves its loop and nothing wraps); not moving fires nothing; a seek
/// (a `reset`, where `std.playhead.jump` marks every seek; or, inferred,
/// backwards, more than one beat, more than 64 loops in a frame, the first
/// frame) fires nothing and resumes from the new place: a step
/// exactly there fires on the next frame. `step` shows `⌊place/s⌋` of the
/// place `x − ⌊x/L⌋·L` in the loop (at most *S* − 1). `ppq` is not read.
///
/// **Cost.** Ticks are processed one by one: each counted tick does a
/// tick's work (binary searches over the steps), so a frame costs in
/// proportion to the ticks it holds (the transport puts at most ⌊bpm·ppq /
/// (60·sr)⌋ + 1 on a frame: 1 at 48 kHz up to 400 bpm and 960 ppq, 6400 at
/// the lowest rate, 1 Hz). The count of one `Set` is capped at 6400, so the
/// work per call stays bounded; driving `tick` from another stream of large
/// `Set` values (not a transport) is a misuse that costs up to 6400 ticks'
/// work per event (`module-contract.md`, "The tick clock and the
/// sequencers").
///
/// **Frame resolution.** At most one hit per lane per frame (the highest
/// level, when ticks are denser than frames, below 640 Hz at 96 ppq) and
/// one `accent`: at most 2 events per output per frame.
///
/// **Live edits.** A changed pattern arrives as a new table at a swap frame
/// (ADR 0028 §4); the position carries on, a shorter loop wraps on the next
/// tick.
#[derive(Clone, Debug, Default)]
pub struct StepSeq8 {
    /// The position in ticks, once counting.
    position: Option<u64>,
    /// The step index the `step` output shows.
    step: u64,
    /// Whether the position has wrapped since `prepare` or a reset: only
    /// then do steps wrapped from the loop's end fire (there was no loop
    /// before the first).
    looped: bool,
    /// Following a position: the previous frame's, if any.
    last: Option<f64>,
}

/// What one frame's ticks fired: per lane the highest level, and accent.
#[derive(Default)]
struct Hits {
    level: [Option<f64>; LANES],
    accent: bool,
}

impl StepSeq8 {
    /// One tick: advance and fire the steps whose (swung) tick it is.
    fn tick(
        &mut self,
        table: Option<&Table>,
        (ppq, accent, swing): (f64, f64, f64),
        hits: &mut Hits,
    ) {
        let header = table.map(|t| t.row(0));
        let steps = header.and_then(|h| h.first()).copied().unwrap_or(0.0);
        let step = header.and_then(|h| h.get(1)).copied().unwrap_or(1.0);
        let span = step * ppq;
        let loop_ticks = (round_half_up(steps * span) as u64).max(1);
        let p = match self.position {
            Some(p) if p + 1 < loop_ticks => p + 1,
            Some(_) => {
                self.looped = true;
                0
            }
            None => 0,
        };
        self.position = Some(p);
        let Some(table) = table else {
            return;
        };
        let grid = |i: u64| round_half_up(i as f64 * span) as u64;
        let d = (round_half_up(span) as u64).max(1);
        let delay = (round_half_up(swing / 100.0 * d as f64) as u64).min(d - 1);
        let count = steps as u64;
        // `grid` never decreases: the first step whose grid tick is at least
        // `tick`, by binary search (several steps may share a tick).
        let first_from = |tick: u64| {
            let (mut lo, mut hi) = (0, count);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if grid(mid) < tick {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        // Every step that can fire on p: grid tick in [p − delay, p], and,
        // wrapped from the loop's end, in [p + L − delay, p + L] (a firing
        // tick at or past the loop length L fires on the next loop's tick
        // `fires − L`; L ≥ the step's span > delay, so the ranges are
        // disjoint and one wrap suffices). The last step with grid tick at
        // most p is the playhead's.
        let (from, to) = (first_from(p.saturating_sub(delay)), first_from(p + 1));
        let wrapped = if self.looped {
            first_from((p + loop_ticks).saturating_sub(delay))..first_from(p + loop_ticks + 1)
        } else {
            0..0
        };
        let current = to.checked_sub(1);
        for i in (from..to).chain(wrapped) {
            let fires = grid(i) + if i % 2 == 1 { delay } else { 0 };
            let fires = if fires >= loop_ticks {
                fires - loop_ticks
            } else {
                fires
            };
            if fires != p {
                continue;
            }
            let row = table.row(1 + i as usize);
            for (lane, slot) in hits.level.iter_mut().enumerate() {
                let level = header.and_then(|h| h.get(2 + lane)).copied().unwrap_or(0.0);
                let cell = row.get(lane).copied().unwrap_or(0.0);
                let value = if cell == 1.0 {
                    level
                } else if cell == 2.0 {
                    level * accent
                } else {
                    continue;
                };
                *slot = Some(slot.map_or(value, |v: f64| v.max(value)));
            }
            hits.accent |= row.get(LANES).is_some_and(|&a| a == 1.0);
        }
        if let Some(i) = current {
            self.step = i;
        }
    }
}

/// A step pattern's header as read when following: steps *S*, step *s*
/// beats, and the loop `L = S·s` (1 beat, and no steps, without a usable
/// table).
fn shape(table: Option<&Table>) -> (u64, f64, f64) {
    let header = table.map(|t| t.row(0));
    let steps = header.and_then(|h| h.first()).copied().unwrap_or(0.0);
    let step = header.and_then(|h| h.get(1)).copied().unwrap_or(1.0);
    if steps >= 1.0 && step.is_finite() && step > 0.0 {
        let count = steps as u64;
        (count, step, follow::period(count as f64 * step))
    } else {
        (0, 1.0, 1.0)
    }
}

impl StepSeq8 {
    /// Following a position: fires the steps whose beat lies in `[from,
    /// to)`.
    fn cross(
        table: &Table,
        (count, step, period): (u64, f64, f64),
        (accent, swing): (f64, f64),
        (from, to): (f64, f64),
        hits: &mut Hits,
    ) {
        let header = table.row(0);
        let delay = swing.clamp(0.0, 75.0) / 100.0 * step;
        // Strictly increasing in i: the delay is below a step.
        let fires = |i: u64| i as f64 * step + if i % 2 == 1 { delay } else { 0.0 };
        for base in follow::loops(from, to, period) {
            let (mut lo, mut hi) = (0, count);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if base + fires(mid) < from {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let mut i = lo;
            while i < count && base + fires(i) < to {
                let row = table.row(1 + i as usize);
                for (lane, slot) in hits.level.iter_mut().enumerate() {
                    let level = header.get(2 + lane).copied().unwrap_or(0.0);
                    let cell = row.get(lane).copied().unwrap_or(0.0);
                    let value = if cell == 1.0 {
                        level
                    } else if cell == 2.0 {
                        level * accent
                    } else {
                        continue;
                    };
                    *slot = Some(slot.map_or(value, |v: f64| v.max(value)));
                }
                hits.accent |= row.get(LANES).is_some_and(|&a| a == 1.0);
                i += 1;
            }
        }
    }

    /// `process` with `follow` connected.
    fn follow(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>, table: Option<&Table>) {
        let positions = inputs.signal(FOLLOW);
        let shape = shape(table);
        let (count, step, period) = shape;
        let mut next_reset = 0;
        for n in 0..inputs.frames() {
            let now = positions.get(n).copied().unwrap_or(0.0);
            // ADR 0037 §6: a swap that gave `follow` other sources (another
            // transport) is a seek on its first frame, as a reset is.
            let rewired = n == 0 && inputs.sources_changed(FOLLOW_PORT);
            let motion = if triggered(inputs.events(RESET), &mut next_reset, n) || rewired {
                Motion::Seek
            } else {
                follow::motion(self.last, now, period)
            };
            let mut hits = Hits::default();
            if let (Motion::Forward { from, to }, Some(table)) = (motion, table) {
                let settings = (
                    inputs
                        .parameter(ACCENT_LEVEL)
                        .get(n)
                        .copied()
                        .unwrap_or(DEFAULT_ACCENT),
                    inputs.parameter(SWING).get(n).copied().unwrap_or(0.0),
                );
                Self::cross(table, shape, settings, (from, to), &mut hits);
            }
            self.last = now.is_finite().then_some(now);
            let index = libm::floor(follow::place(now, period) / step);
            self.step = if count == 0 {
                0
            } else {
                (index.max(0.0) as u64).min(count - 1)
            };
            write(outputs, n, &hits, self.step);
        }
    }
}

/// Frame `n`'s hits and playhead.
fn write(outputs: &mut Outputs<'_>, n: usize, hits: &Hits, step: u64) {
    for (lane, level) in hits.level.iter().enumerate() {
        if let Some(level) = *level {
            push(outputs, lane, n, EventPayload::Set(level.clamp(-1e6, 1e6)));
            push(outputs, lane, n, EventPayload::Trigger);
        }
    }
    if hits.accent {
        push(outputs, ACCENT_OUT, n, EventPayload::Trigger);
    }
    if let Some(out) = outputs.signal(STEP).get_mut(n) {
        *out = step as f64;
    }
}

fn push(outputs: &mut Outputs<'_>, port: usize, n: usize, payload: EventPayload) {
    if let Some(sink) = outputs.events(port) {
        sink.push(Event {
            offset: u32::try_from(n).unwrap_or(u32::MAX),
            payload,
        });
    }
}

impl DirectPrimitive for StepSeq8 {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        *self = StepSeq8::default();
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let table = inputs.table().filter(|t| t.width() == STEP_WIDTH);
        if inputs.is_connected(FOLLOW_PORT) {
            return self.follow(inputs, outputs, table);
        }
        self.last = None;
        let ticks = inputs.events(TICK);
        let (mut next_tick, mut next_reset) = (0, 0);
        for n in 0..inputs.frames() {
            if triggered(inputs.events(RESET), &mut next_reset, n) {
                self.position = None;
                self.step = 0;
                self.looped = false;
            }
            let settings = (
                ppq_at(inputs.parameter(PPQ_PARAMETER), n) as f64,
                inputs
                    .parameter(ACCENT_LEVEL)
                    .get(n)
                    .copied()
                    .unwrap_or(DEFAULT_ACCENT),
                inputs.parameter(SWING).get(n).copied().unwrap_or(0.0),
            );
            let mut hits = Hits::default();
            while let Some(event) = ticks.get(next_tick) {
                if event.offset as usize > n {
                    break;
                }
                if event.offset as usize == n {
                    for _ in 0..tick_count(event.payload) {
                        self.tick(table, settings, &mut hits);
                    }
                }
                next_tick += 1;
            }
            write(outputs, n, &hits, self.step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::compile_steps;
    use crate::testing::context;
    use signal_core::EventSink;

    /// `lanes` as (level, cells) with `.`, `x`, `X`.
    fn steps(step: f64, lanes: &[(f64, &str)]) -> Table {
        let count = lanes[0].1.len();
        let mut values = Vec::new();
        for (level, cells) in lanes {
            values.push(*level);
            values.extend(cells.chars().map(|c| match c {
                'x' => 1.0,
                'X' => 2.0,
                _ => 0.0,
            }));
        }
        let source = Table::new(count + 1, values).unwrap();
        compile_steps(&[count as f64, step], &source).unwrap()
    }

    /// Per lane its (frame, payload) events, the accent frames, `step` per frame.
    type Run = (Vec<Vec<(u32, EventPayload)>>, Vec<u32>, Vec<f64>);

    /// One tick per frame over `frames`.
    fn run(table: &Table, (ppq, accent, swing): (f64, f64, f64), frames: usize) -> Run {
        let mut seq = StepSeq8::default();
        seq.prepare(&context(48_000.0, frames, 1));
        let events: Vec<Event> = (0..frames as u32)
            .map(|offset| Event {
                offset,
                payload: EventPayload::Trigger,
            })
            .collect();
        let ends = [events.len(), events.len()];
        let parameters = [vec![ppq; frames], vec![accent; frames], vec![swing; frames]].concat();
        let mut signals = vec![0.0; frames];
        let mut sinks = vec![EventSink::per_frame(16, frames); LANES + 1];
        seq.process(
            &Inputs::new(frames, &[], &parameters, &events, &ends).with_table(Some(table)),
            &mut Outputs::new(frames, &mut signals, &mut sinks),
        );
        let lanes = sinks[..LANES]
            .iter()
            .map(|s| s.events().iter().map(|e| (e.offset, e.payload)).collect())
            .collect();
        let accents = sinks[LANES].events().iter().map(|e| e.offset).collect();
        (lanes, accents, signals)
    }

    #[test]
    fn lanes_fire_level_then_trigger_with_accents_and_loop() {
        // 4 steps of 1 beat at 1 ppq: one step per tick.
        let table = steps(1.0, &[(0.5, "x.X."), (1.0, "..x.")]);
        let (lanes, accents, step) = run(&table, (1.0, 2.0, 0.0), 8);
        assert_eq!(
            lanes[0],
            [
                (0, EventPayload::Set(0.5)),
                (0, EventPayload::Trigger),
                (2, EventPayload::Set(1.0)),
                (2, EventPayload::Trigger),
                (4, EventPayload::Set(0.5)),
                (4, EventPayload::Trigger),
                (6, EventPayload::Set(1.0)),
                (6, EventPayload::Trigger),
            ]
        );
        assert_eq!(
            lanes[1]
                .iter()
                .filter(|e| e.1 == EventPayload::Trigger)
                .count(),
            2
        );
        assert!(lanes[2..].iter().all(Vec::is_empty));
        assert_eq!(accents, [2, 6]);
        assert_eq!(step, [0.0, 1.0, 2.0, 3.0, 0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn swing_delays_even_steps_by_whole_ticks() {
        // 16ths at 8 ppq: 2 ticks a step; 50 % swing delays odd indices 1 tick.
        let table = steps(0.25, &[(1.0, "xxxx")]);
        let (lanes, _, step) = run(&table, (8.0, 1.5, 50.0), 8);
        let frames: Vec<u32> = lanes[0]
            .iter()
            .filter(|e| e.1 == EventPayload::Trigger)
            .map(|e| e.0)
            .collect();
        assert_eq!(frames, [0, 3, 4, 7]);
        assert_eq!(step, [0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0]);
        // 75 % of 2 ticks rounds to 2, capped at 1: the delayed step never
        // reaches the next one.
        let (lanes, _, _) = run(&table, (8.0, 1.5, 75.0), 8);
        assert_eq!(lanes[0].len(), 8);
    }
}
