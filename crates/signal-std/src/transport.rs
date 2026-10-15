//! `std.transport`: musical time, which belongs in a library and never in the
//! core (ADR 0003).

use signal_core::{
    Combine, DirectPrimitive, Direction, Event, EventPayload, Feedthrough, Inputs,
    ModuleDescriptor, Outputs, PortKind, PrepareContext, RngStream,
};

use crate::{at, event_port, parameter, round_half_up};

const BPM: usize = 0;
const GHOST_PROBABILITY: usize = 1;
const GHOST_OFFSET_MS: usize = 2;
const ACCENT_EVERY: usize = 3;

const BEAT: usize = 0;
const GHOST: usize = 1;
const ACCENT: usize = 2;

const DEFAULT_BPM: f64 = 120.0;
const DEFAULT_ACCENT_EVERY: f64 = 3.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.transport",
    doc: "Tempo clock: beat and accent triggers on an exact integer frame grid, and seeded ghost notes on off-beat 16ths.",
    ports: &[
        event_port("beat", Direction::Out),
        event_port("ghost", Direction::Out),
        event_port("accent", Direction::Out),
    ],
    parameters: &[
        parameter(
            "bpm",
            PortKind::Control,
            Some("bpm"),
            20.0,
            400.0,
            DEFAULT_BPM,
            Combine::Add,
        ),
        parameter(
            "ghost_probability",
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.0,
            Combine::Add,
        ),
        parameter(
            "ghost_offset_ms",
            PortKind::Control,
            Some("ms"),
            -50.0,
            50.0,
            0.0,
            Combine::Add,
        ),
        parameter(
            "accent_every",
            PortKind::Control,
            Some("beats"),
            1.0,
            64.0,
            DEFAULT_ACCENT_EVERY,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.transport`: a tempo clock that emits beats, accents and seeded ghost
/// notes at exact frames.
///
/// **Ports**
/// - `beat` (event out, trigger): one `Trigger` per beat.
/// - `ghost` (event out, trigger): one `Trigger` per ghost note.
/// - `accent` (event out, trigger): one `Trigger` per accented beat, on the
///   beat's frame.
///
/// **Parameters**
/// - `bpm` (control, beats per minute, range `[20, 400]`, default 120,
///   combine `add`): read on the first frame after `prepare` or `reset` and
///   again on each beat's frame, quantised to `bpm_milli =
///   round_half_up(bpm·1000)`. A changed tempo takes effect from that beat:
///   the grid is anchored again at the beat (its frame and index), so the
///   tempo changes without a jump or drift, at most one beat after the
///   parameter. At a constant tempo the anchor stays at frame 0.
/// - `ghost_probability` (control, no unit, range `[0, 1]`, default 0, combine
///   `add`): read on each candidate's frame.
/// - `ghost_offset_ms` (control, ms, range `[−50, 50]`, default 0, combine
///   `add`): read once, on the first frame, so that the offset is fixed for
///   the run.
/// - `accent_every` (control, beats, range `[1, 64]`, default 3, combine
///   `add`): read on each beat's frame, rounded half up to a whole number of beats.
///
/// **Feedthrough:** direct (it has no inputs).
///
/// **Formula** (all frames counted from the first frame; sr the sample rate,
/// a whole number of frames per second; exact integer arithmetic in `u128`,
/// never accumulated floats; `round_half_up(p/q) = ⌊(2p + q) / 2q⌋`):
///
/// ```text
/// anchor (a, b)                 frame a and index b of the beat where the
///                               current tempo began ((0, 0) at first)
/// beat k (k = b, b+1, …)        at a + round_half_up((k−b)·60000·sr / bpm_milli)
/// accent                        on beat k when k mod round_half_up(accent_every) = 0
/// ghost candidate j             for j = 1, 2, 3, 5, 6, 7, 9, … (16ths, j mod 4 ≠ 0)
///                               at max(0, a + round_half_up((j−4b)·15000·sr / bpm_milli)
///                                         + round_half_up(ghost_offset_ms·sr/1000)),
///                               and never before the event before it
/// ghost                         at candidate j when its draw < ghost_probability
/// ```
///
/// Each candidate consumes **exactly one** `next_f64()` from the instance's
/// `RngStream`, in candidate order, whatever the probability, so the seed
/// changes only the ghosts: beats and accents do not depend on it. The accent
/// against the 4-beat bar is the polymeter (3 against 4 by default).
///
/// Work per call is proportional to the events it emits, not to its frames:
/// at most one beat and, beside the clamped first candidates, one ghost
/// candidate per frame at any rate of at least 27 frames per second.
#[derive(Clone, Debug, Default)]
pub struct Transport {
    sample_rate: f64,
    /// The sample rate as whole frames per second.
    rate: u128,
    /// The stream as `prepare` received it, and the stream being drawn from.
    streams: Option<(RngStream, RngStream)>,
    /// The first frame of the next call.
    frame: u64,
    /// The frame of the last event, which the next one never precedes.
    last: u64,
    /// Fixed on the first frame.
    grid: Option<Grid>,
    /// The index of the next beat.
    beat: u64,
    /// The index of the next ghost candidate (never a multiple of 4).
    candidate: u64,
}

/// The grid: the quantised tempo and where it began (a beat), and the
/// ghost offset the first frame fixed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Grid {
    bpm_milli: u128,
    /// The ghost offset in frames.
    offset: i128,
    /// The frame of the beat where this tempo began.
    origin: u64,
    /// That beat's index.
    base: u64,
}

/// `round_half_up(numerator / denominator)` for a signed numerator and
/// `denominator` ≥ 1: `⌊(2p + q) / 2q⌋`.
fn signed_ratio(numerator: i128, denominator: u128) -> i128 {
    let q = i128::try_from(denominator).unwrap_or(i128::MAX).max(1);
    numerator
        .saturating_mul(2)
        .saturating_add(q)
        .div_euclid(q.saturating_mul(2))
}

/// `round_half_up(numerator / denominator)` for `denominator` ≥ 1.
fn ratio(numerator: u128, denominator: u128) -> u128 {
    numerator
        .saturating_mul(2)
        .saturating_add(denominator)
        .checked_div(denominator.saturating_mul(2))
        .unwrap_or(u128::MAX)
}

fn frame(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

impl Grid {
    /// The frame of beat `k` (k ≥ the anchor's beat).
    fn beat(&self, k: u64, rate: u128) -> u64 {
        let numerator = u128::from(k.saturating_sub(self.base))
            .saturating_mul(60_000)
            .saturating_mul(rate);
        self.origin
            .saturating_add(frame(ratio(numerator, self.bpm_milli)))
    }

    /// The frame of the 16th `j`, offset and clamped at frame 0. A 16th
    /// before the anchor's beat (a late candidate of the tempo before) is
    /// placed back from the anchor.
    fn candidate(&self, j: u64, rate: u128) -> u64 {
        let sixteenths = i128::from(j) - 4 * i128::from(self.base);
        let numerator = sixteenths
            .saturating_mul(15_000)
            .saturating_mul(i128::try_from(rate).unwrap_or(i128::MAX));
        let at = i128::from(self.origin)
            .saturating_add(signed_ratio(numerator, self.bpm_milli))
            .saturating_add(self.offset);
        frame(u128::try_from(at).unwrap_or(0))
    }
}

/// The candidate after `j`, skipping multiples of 4 (the beats).
fn next_candidate(j: u64) -> u64 {
    let next = j.saturating_add(1);
    if next.is_multiple_of(4) {
        next.saturating_add(1)
    } else {
        next
    }
}

fn trigger(outputs: &mut Outputs<'_>, port: usize, offset: u64) {
    if let Some(sink) = outputs.events(port) {
        sink.push(Event {
            offset: u32::try_from(offset).unwrap_or(u32::MAX),
            payload: EventPayload::Trigger,
        });
    }
}

impl DirectPrimitive for Transport {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.rate = round_half_up(ctx.sample_rate).max(1.0) as u128;
        self.streams = Some((ctx.rng.clone(), ctx.rng.clone()));
        self.reset();
    }

    fn reset(&mut self) {
        if let Some((initial, stream)) = &mut self.streams {
            stream.clone_from(initial);
        }
        self.frame = 0;
        self.last = 0;
        self.grid = None;
        self.beat = 0;
        self.candidate = 1;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let start = self.frame;
        let end = start.saturating_add(inputs.frames() as u64);
        let rate = self.rate;
        let sample_rate = self.sample_rate;
        let tempo = |index: usize| {
            round_half_up(at(inputs.parameter(BPM), index, DEFAULT_BPM) * 1000.0).max(1.0) as u128
        };
        let mut grid = *self.grid.get_or_insert_with(|| Grid {
            bpm_milli: tempo(0),
            offset: round_half_up(
                at(inputs.parameter(GHOST_OFFSET_MS), 0, 0.0) * sample_rate / 1000.0,
            ) as i128,
            origin: 0,
            base: 0,
        });
        loop {
            let beat = grid.beat(self.beat, rate);
            // Never before the event before it (a tempo change can move a
            // pending candidate back).
            let candidate = grid.candidate(self.candidate, rate).max(self.last);
            let next = beat.min(candidate);
            if next >= end {
                break;
            }
            self.last = next;
            let offset = next.saturating_sub(start);
            let index = usize::try_from(offset).unwrap_or(usize::MAX);
            if beat == next {
                // The tempo on the beat's frame: a change takes effect from
                // this beat, anchored here.
                let bpm_milli = tempo(index);
                if bpm_milli != grid.bpm_milli {
                    grid = Grid {
                        bpm_milli,
                        origin: beat,
                        base: self.beat,
                        ..grid
                    };
                    self.grid = Some(grid);
                }
                trigger(outputs, BEAT, offset);
                let every = at(inputs.parameter(ACCENT_EVERY), index, DEFAULT_ACCENT_EVERY);
                // NaN converts to 0, which `max(1)` lifts back into range.
                let every = (round_half_up(every).clamp(1.0, 64.0) as u64).max(1);
                if self.beat.is_multiple_of(every) {
                    trigger(outputs, ACCENT, offset);
                }
                self.beat = self.beat.saturating_add(1);
            } else {
                let draw = match &mut self.streams {
                    Some((_, stream)) => stream.next_f64(),
                    None => 1.0,
                };
                if draw < at(inputs.parameter(GHOST_PROBABILITY), index, 0.0) {
                    trigger(outputs, GHOST, offset);
                }
                self.candidate = next_candidate(self.candidate);
            }
        }
        self.frame = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{context, stream};
    use signal_core::EventSink;

    /// The frames of the triggers on `beat`, `ghost` and `accent`.
    #[derive(Debug, Default, PartialEq)]
    struct Events {
        beat: Vec<u64>,
        ghost: Vec<u64>,
        accent: Vec<u64>,
    }

    /// Runs a transport for `frames` frames in calls of `block` frames. The
    /// parameters are piecewise constant: `segments` lists `(first frame,
    /// [bpm, ghost_probability, ghost_offset_ms, accent_every])` in order.
    fn run(
        sample_rate: f64,
        seed: u64,
        frames: u64,
        block: usize,
        segments: &[(u64, [f64; 4])],
    ) -> Events {
        let mut transport = Transport::default();
        transport.prepare(&context(sample_rate, block, seed));
        let mut sinks = vec![EventSink::with_capacity(64); 3];
        let mut parameters = vec![0.0; 4 * block];
        let mut filled: Option<usize> = None;
        let mut events = Events::default();
        let mut start = 0u64;
        while start < frames {
            let k = (block as u64).min(frames - start) as usize;
            let segment = |frame: u64| segments.iter().rposition(|&(from, _)| from <= frame);
            let (first, last) = (segment(start), segment(start + k as u64 - 1));
            if first != last || filled != first || k != block {
                for i in 0..k {
                    let values = segments[segment(start + i as u64).unwrap()].1;
                    for (p, value) in values.into_iter().enumerate() {
                        parameters[p * k + i] = value;
                    }
                }
                filled = if first == last && k == block {
                    first
                } else {
                    None
                };
            }
            transport.process(
                &Inputs::new(k, &[], &parameters[..4 * k], &[], &[]),
                &mut Outputs::new(k, &mut [], &mut sinks),
            );
            let lists = [&mut events.beat, &mut events.ghost, &mut events.accent];
            for (list, sink) in lists.into_iter().zip(&mut sinks) {
                assert_eq!(sink.overflows(), 0);
                list.extend(sink.events().iter().map(|e| start + u64::from(e.offset)));
                sink.clear();
            }
            start += k as u64;
        }
        events
    }

    /// `round_half_up(numerator / denominator)`, written out independently.
    fn exact(numerator: u128, denominator: u128) -> u64 {
        let (q, r) = (numerator / denominator, numerator % denominator);
        (if 2 * r >= denominator { q + 1 } else { q }) as u64
    }

    fn candidates(bpm_milli: u128, sample_rate: u128, offset_frames: i128, below: u64) -> Vec<u64> {
        (1u64..)
            .filter(|j| !j.is_multiple_of(4))
            .map(|j| {
                let base = exact(u128::from(j) * 15_000 * sample_rate, bpm_milli) as i128;
                (base + offset_frames).max(0) as u64
            })
            .take_while(|&f| f < below)
            .collect()
    }

    /// AC11: at 132 bpm and 96 kHz the first 10 000 beats fall exactly on
    /// `round_half_up(k·60000·sr/bpm_milli)`: no drift. Accents every 3rd beat.
    #[test]
    fn beats_at_132_bpm_and_96_khz_have_no_drift_over_10000_beats() {
        let beat = |k: u64| exact(u128::from(k) * 60_000 * 96_000, 132_000);
        let frames = beat(9_999) + 1;
        let events = run(96_000.0, 1, frames, 1 << 16, &[(0, [132.0, 0.0, 0.0, 3.0])]);
        assert_eq!(events.beat.len(), 10_000);
        for (k, &frame) in events.beat.iter().enumerate() {
            assert_eq!(frame, beat(k as u64), "beat {k}");
        }
        // 60000·96000/132000 = 43636.36…: every interval is 43636 or 43637.
        assert!(
            events
                .beat
                .windows(2)
                .all(|w| w[1] - w[0] == 43_636 || w[1] - w[0] == 43_637)
        );
        // 9999·60000·96000/132000 = 9999·480000/11, and 11 divides 9999.
        assert_eq!(events.beat[9_999], 436_320_000);
        let accents: Vec<u64> = events.beat.iter().copied().step_by(3).collect();
        assert_eq!(events.accent, accents);
        assert!(events.ghost.is_empty());
    }

    /// AC11: the same seed gives the same ghosts; another seed gives other
    /// ghosts and the same beats and accents.
    #[test]
    fn ghosts_are_reproducible_per_seed_and_beats_are_seed_independent() {
        let segments = [(0, [132.0, 0.5, -10.0, 3.0])];
        let frames = 2_000_000;
        let a = run(96_000.0, 7, frames, 4096, &segments);
        let again = run(96_000.0, 7, frames, 999, &segments);
        let b = run(96_000.0, 8, frames, 4096, &segments);
        assert_eq!(a, again);
        assert!(!a.ghost.is_empty());
        assert_ne!(a.ghost, b.ghost);
        assert_eq!(a.beat, b.beat);
        assert_eq!(a.accent, b.accent);
        assert_eq!(a.beat.len(), 46);
    }

    /// AC11: every candidate makes exactly one draw, in candidate order,
    /// whatever the probability: with p = 0, then 1, then 0.5, candidate *c*
    /// fires exactly when the *c*-th draw of the stream is below p.
    #[test]
    fn exactly_one_draw_per_candidate() {
        let (sample_rate, seed) = (96_000.0, 3);
        let frames = 3_000_000;
        let segments = [
            (0, [132.0, 0.0, 12.5, 3.0]),
            (1_000_000, [132.0, 1.0, 12.5, 3.0]),
            (2_000_000, [132.0, 0.5, 12.5, 3.0]),
        ];
        let probability = |frame: u64| match frame {
            0..1_000_000 => 0.0,
            1_000_000..2_000_000 => 1.0,
            _ => 0.5,
        };
        let offset = round_half_up(12.5 * 96_000.0 / 1000.0) as i128; // 1200 frames
        let mut draws = stream(seed);
        let expected: Vec<u64> = candidates(132_000, 96_000, offset, frames)
            .into_iter()
            .filter(|&frame| draws.next_f64() < probability(frame))
            .collect();
        let events = run(sample_rate, seed, frames, 1000, &segments);
        assert_eq!(events.ghost, expected);
        // Every candidate fires while p = 1, none while p = 0, some at p = 0.5.
        let all = candidates(132_000, 96_000, offset, frames);
        let between = |list: &[u64], from: u64, to: u64| {
            list.iter().filter(|&&f| (from..to).contains(&f)).count()
        };
        assert_eq!(between(&events.ghost, 0, 1_000_000), 0);
        assert_eq!(
            between(&events.ghost, 1_000_000, 2_000_000),
            between(&all, 1_000_000, 2_000_000)
        );
        let half = between(&events.ghost, 2_000_000, frames);
        assert!(half > 0 && half < between(&all, 2_000_000, frames));
    }

    /// bpm is quantised to thousandths, rounding half up; the ghost offset
    /// is read on the first frame only.
    #[test]
    fn tempo_is_quantised_and_the_ghost_offset_read_once() {
        let fixed = run(
            48_000.0,
            1,
            500_000,
            512,
            &[(0, [120.0625, 1.0, -50.0, 4.0])],
        );
        let offset_changed = run(
            48_000.0,
            1,
            500_000,
            512,
            &[
                (0, [120.0625, 1.0, -50.0, 4.0]),
                (1, [120.0625, 1.0, 50.0, 4.0]),
            ],
        );
        assert_eq!(fixed, offset_changed);
        // 120.0625·1000 = 120062.5 → bpm_milli 120063.
        let beats: Vec<u64> = (0..)
            .map(|k: u64| exact(u128::from(k) * 60_000 * 48_000, 120_063))
            .take_while(|&f| f < 500_000)
            .collect();
        assert_eq!(fixed.beat, beats);
        let offset = round_half_up(-50.0 * 48.0) as i128;
        assert_eq!(fixed.ghost, candidates(120_063, 48_000, offset, 500_000));
    }

    /// A tempo change takes effect from the next beat, anchored there: the
    /// beats before it are the old tempo's, the ones after it the new
    /// tempo's counted from that beat, with no jump.
    #[test]
    fn a_tempo_change_takes_effect_from_the_next_beat() {
        // 120 bpm at 48 kHz: a beat every 24 000 frames. 200 bpm from frame
        // 30 000: beat 1 (24 000) is before it, so beat 2 (48 000) is the
        // anchor; then a beat every 14 400 frames.
        let events = run(
            48_000.0,
            1,
            100_000,
            512,
            &[
                (0, [120.0, 0.0, 0.0, 1.0]),
                (30_000, [200.0, 0.0, 0.0, 1.0]),
            ],
        );
        assert_eq!(events.beat, [0, 24_000, 48_000, 62_400, 76_800, 91_200]);
        assert_eq!(events.accent, events.beat);
    }

    /// With tempo changes, the events are the same for any partition into
    /// calls, and in order: ghosts never precede the event before them.
    #[test]
    fn tempo_changes_do_not_depend_on_the_partition() {
        let segments = [
            (0, [132.0, 0.6, -50.0, 3.0]),
            (70_000, [400.0, 0.6, -50.0, 3.0]),
            (150_000, [20.0, 0.6, -50.0, 3.0]),
            (400_000, [97.3, 0.6, -50.0, 3.0]),
        ];
        let reference = run(96_000.0, 9, 800_000, 4096, &segments);
        for block in [1, 7, 512, 1000] {
            assert_eq!(
                run(96_000.0, 9, 800_000, block, &segments),
                reference,
                "block {block}"
            );
        }
        for list in [&reference.beat, &reference.ghost] {
            assert!(list.windows(2).all(|w| w[0] <= w[1]), "{list:?}");
        }
        assert!(reference.beat.len() > 10 && !reference.ghost.is_empty());
    }

    /// A negative offset clamps a candidate at frame 0 rather than dropping it.
    #[test]
    fn negative_offsets_clamp_at_frame_zero() {
        // 400 bpm at 192 kHz: a 16th is 7200 frames; −50 ms is −9600 frames.
        let events = run(192_000.0, 1, 30_000, 4096, &[(0, [400.0, 1.0, -50.0, 1.0])]);
        assert_eq!(events.ghost, [0, 4800, 12_000, 26_400]);
        assert_eq!(events.beat, [0, 28_800]);
        assert_eq!(events.accent, events.beat);
    }

    #[test]
    fn reset_restarts_the_clock_and_the_stream() {
        let mut transport = Transport::default();
        transport.prepare(&context(48_000.0, 64, 5));
        let parameters: Vec<f64> = [[100.0; 64], [0.5; 64], [0.0; 64], [3.0; 64]].concat();
        let mut sinks = vec![EventSink::with_capacity(64); 3];
        let mut first = Vec::new();
        for _ in 0..2 {
            let mut seen = Vec::new();
            for call in 0..2000u64 {
                transport.process(
                    &Inputs::new(64, &[], &parameters, &[], &[]),
                    &mut Outputs::new(64, &mut [], &mut sinks),
                );
                for (port, sink) in sinks.iter_mut().enumerate() {
                    seen.extend(
                        sink.events()
                            .iter()
                            .map(|e| (port, call * 64 + u64::from(e.offset))),
                    );
                    sink.clear();
                }
            }
            if first.is_empty() {
                first = seen;
                transport.reset();
            } else {
                assert_eq!(seen, first);
            }
        }
    }
}
