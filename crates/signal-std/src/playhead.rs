//! `std.playhead` (ADR 0033): musical time as a signal, a position in beats
//! carried on a wire. Tempo belongs in a library, never in the core (ADR 0003).

use signal_core::{
    Combine, DirectPrimitive, Direction, EventPayload, Feedthrough, Inputs, ModuleDescriptor,
    Outputs, PortKind, PortShape, PrepareContext, Range,
};

use crate::{SIGNAL_BOUND, at, event_port, parameter, port_at, push_trigger, signal_port};

/// A playhead stopped or running, by `run` (ADR 0043: its value set).
const RUN_VALUES: &[signal_core::ValueLabel] =
    &[crate::named(0.0, "stopped"), crate::named(1.0, "running")];

const SEEK: usize = 0;

/// Signal outputs, in order.
const POSITION: usize = 0;
const RUNNING: usize = 1;
const TEMPO: usize = 2;

/// Event outputs.
const JUMP: usize = 0;

const BPM: usize = 0;
const RUN: usize = 1;
const RATE: usize = 2;

const DEFAULT_BPM: f64 = 120.0;
/// The fastest `rate`, either way: a sixteenfold fast-forward or rewind.
const MAX_RATE: f64 = 16.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.playhead",
    doc: "Musical time as a signal: position in beats (exact from the last change of bpm, rate or run, never drifting), running gate and tempo; bpm, run (0/1) and rate (a multiplier of the tempo: 0 holds, negative plays backwards; unconnected 1) set its speed, and a seek event jumps it (Set(beat), or Trigger to 0), announced by a jump trigger on that frame for followers to take as a seek.",
    ports: &[
        port_at(
            "seek",
            Direction::In,
            PortShape::Events,
            1,
            PortKind::Generic,
            Some("beats"),
            None,
        ),
        signal_port(
            "position",
            Direction::Out,
            PortKind::Control,
            Some("beats"),
            Some(Range {
                min: -SIGNAL_BOUND,
                max: SIGNAL_BOUND,
            }),
        ),
        signal_port(
            "running",
            Direction::Out,
            PortKind::Gate,
            None,
            Some(Range { min: 0.0, max: 1.0 }),
        ),
        signal_port(
            "tempo",
            Direction::Out,
            PortKind::Control,
            Some("bpm"),
            Some(Range {
                min: -400.0 * MAX_RATE,
                max: 400.0 * MAX_RATE,
            }),
        ),
        event_port("jump", Direction::Out),
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
        crate::choice(
            parameter("run", PortKind::Gate, None, 0.0, 1.0, 1.0, Combine::Replace),
            RUN_VALUES,
        ),
        parameter(
            "rate",
            PortKind::Control,
            None,
            -MAX_RATE,
            MAX_RATE,
            1.0,
            Combine::Replace,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// `std.playhead`: a position in beats as a signal, for sequencers to follow
/// (`std.note_seq.follow`, `std.step_seq8.follow`) and anything else to read
/// (ADR 0033).
///
/// **Ports**
/// - `seek` (event in, beats): `Set(b)` jumps the position to *b* (clamped
///   to ±10⁶) on its frame; `Trigger` jumps it to 0 (a rewind); a `Ramp`
///   or a non-finite value does nothing. Of several on one frame the last
///   (canonical order) wins.
/// - `position` (signal out, beats): the position at the frame, *P(n)*.
/// - `running` (signal out, gate): 1 while `run` ≥ 0.5, else 0.
/// - `tempo` (signal out, bpm): the speed in beats per minute, `bpm·rate`
///   while running, 0 when stopped.
/// - `jump` (event out, trigger): one `Trigger` on every frame a seek lands
///   on (whatever its distance, a seek to the same place included), so a
///   follower can take that frame as a seek rather than infer it from the
///   motion: wire it into `std.note_seq.reset` and `std.step_seq8.reset`,
///   which mark a seek while following (`lib.transport` exports it). A
///   forward seek of at most one beat looks like motion on `position`
///   alone.
///
/// **Parameters**
/// - `bpm` (control, bpm, `[20, 400]`, default 120, combine `add`).
/// - `run` (gate, `[0, 1]`, default 1, combine **`replace`**): runs at
///   ≥ 0.5 (a non-finite value stops it); a project without a transport
///   panel plays as soon as it starts. Unconnected it is its base (the
///   panel's play toggle); a connected gate (OSC, MIDI, a sequencer)
///   replaces the base, so it starts and stops the transport.
/// - `rate` (control, a factor of the tempo, `[−16, 16]`, default 1, combine
///   **`replace`**): the speed multiplier. Unconnected it is its base (1
///   unless set); a connected signal replaces the base (a turntable's
///   platter, a scrub wheel, a fast-forward). 0 holds the position;
///   negative values play backwards.
///
/// All three are read on every frame. A non-finite `bpm` reads as 120 and
/// a non-finite `rate` as 0 (held); the engine keeps both within their
/// ranges otherwise.
///
/// **Feedthrough:** direct (a `seek` on frame *n* moves `position` at *n*).
///
/// **Formula.** With *sr* the sample rate, the speed of frame *n* is
/// `v(n) = run(n)·bpm(n)·rate(n) / (60·sr)` beats per frame (run as 0 or 1),
/// and `P(n + 1) = P(n) + v(n)` from `P(0) = 0`; a seek sets `P(n)`. A
/// **segment** is a run of frames over which `bpm`, `rate` and `run` stay
/// the same and no seek arrives. Within a segment that started at frame
/// *f₀* at position *p₀*:
///
/// ```text
/// P(n) = p₀ + ((n − f₀)·(bpm·rate·run)) / (60·sr)
/// ```
///
/// computed from the segment's start, never summed frame by frame, so a
/// constant tempo has **no accumulated drift**: at an integer `bpm·rate`
/// the product and the division are each rounded once, so *P(n)* is the
/// exact position correctly rounded (at 120 bpm and 48 kHz beat *k* is
/// exactly *k* at frame 24 000·*k*), however long it runs. At every change
/// the position is **rebased**: the new segment starts where the old one
/// reached, `p₀ ← p₀ + (n − f₀)·v`, added with Neumaier's compensated
/// summation into a pair (sum, compensation). So when `rate` (or `bpm`)
/// varies on every frame, each frame is a segment of one and the position
/// is a compensated sum of the per-frame speeds since the last seek: its
/// error stays within a few units in the last place of the position,
/// instead of growing with the number of frames as a plain sum's does.
/// The state depends only on the frame values, never on how the frames
/// are split into calls (k-partition) or threads.
///
/// **Bound.** The position is clamped to ±10⁶ beats, the engine's
/// magnitude bound (about 5.8 days at 120 bpm, 2.6 hours at 400 bpm and
/// rate 16), where it stays until it moves back or a seek moves it.
#[derive(Clone, Debug, Default)]
pub struct Playhead {
    /// 60·sr: the divisor of the speed.
    per_minute: f64,
    /// The segment's start position, as a compensated pair: sum and
    /// compensation.
    base: (f64, f64),
    /// Frames since the segment started.
    frames: u64,
    /// The segment's speed numerator `bpm·rate` (0 when stopped), its
    /// `(bpm, rate, running)` as read; `None` before the first frame.
    segment: Option<Segment>,
}

/// What one segment moves by.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Segment {
    bpm: f64,
    rate: f64,
    running: bool,
}

impl Segment {
    /// Beats per minute: `bpm·rate` while running, else 0.
    fn tempo(&self) -> f64 {
        if self.running {
            self.bpm * self.rate
        } else {
            0.0
        }
    }
}

/// Neumaier's compensated sum: adds `x` into `(sum, compensation)`.
fn add(pair: &mut (f64, f64), x: f64) {
    let (sum, compensation) = *pair;
    let t = sum + x;
    let lost = if sum.abs() >= x.abs() {
        (sum - t) + x
    } else {
        (x - t) + sum
    };
    *pair = (t, compensation + lost);
}

impl Playhead {
    /// The position `frames` frames into the current segment.
    fn position(&self) -> f64 {
        let (sum, compensation) = self.base;
        let moved = match self.segment {
            Some(segment) if self.frames > 0 => {
                (self.frames as f64 * segment.tempo()) / self.per_minute
            }
            _ => 0.0,
        };
        sum + (compensation + moved)
    }

    /// Starts a new segment here, where the current one reached.
    fn rebase(&mut self) {
        if let Some(segment) = self.segment
            && self.frames > 0
        {
            let moved = (self.frames as f64 * segment.tempo()) / self.per_minute;
            add(&mut self.base, moved);
        }
        self.frames = 0;
    }
}

impl DirectPrimitive for Playhead {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.per_minute = 60.0 * ctx.sample_rate.max(1.0);
        self.reset();
    }

    fn reset(&mut self) {
        self.base = (0.0, 0.0);
        self.frames = 0;
        self.segment = None;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let seeks = inputs.events(SEEK);
        let mut next = 0;
        for n in 0..inputs.frames() {
            let finite = |value: f64, fallback: f64| {
                if value.is_finite() { value } else { fallback }
            };
            let segment = Segment {
                bpm: finite(at(inputs.parameter(BPM), n, DEFAULT_BPM), DEFAULT_BPM)
                    .clamp(20.0, 400.0),
                rate: finite(at(inputs.parameter(RATE), n, 1.0), 0.0).clamp(-MAX_RATE, MAX_RATE),
                running: crate::chosen(
                    RUN_VALUES,
                    signal_core::Snap::Nearest,
                    at(inputs.parameter(RUN), n, 1.0),
                ) >= 1.0,
            };
            // The last seek of this frame, if any.
            let mut seek = None;
            while let Some(event) = seeks.get(next) {
                if event.offset as usize > n {
                    break;
                }
                if event.offset as usize == n {
                    match event.payload {
                        EventPayload::Trigger => seek = Some(0.0),
                        EventPayload::Set(b) if b.is_finite() => {
                            seek = Some(b.clamp(-SIGNAL_BOUND, SIGNAL_BOUND));
                        }
                        _ => {}
                    }
                }
                next += 1;
            }
            if let Some(b) = seek {
                self.base = (b, 0.0);
                self.frames = 0;
                self.segment = Some(segment);
                push_trigger(outputs, JUMP, n);
            } else if self.segment != Some(segment) {
                self.rebase();
                self.segment = Some(segment);
            }
            let mut position = self.position();
            if !(-SIGNAL_BOUND..=SIGNAL_BOUND).contains(&position) {
                // Held at the bound, from which it can move back.
                position = position.clamp(-SIGNAL_BOUND, SIGNAL_BOUND);
                self.base = (position, 0.0);
                self.frames = 0;
            }
            let values = [
                position,
                f64::from(u8::from(segment.running)),
                segment.tempo(),
            ];
            for (port, value) in [POSITION, RUNNING, TEMPO].into_iter().zip(values) {
                if let Some(out) = outputs.signal(port).get_mut(n) {
                    *out = value;
                }
            }
            self.frames = self.frames.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::context;
    use signal_core::{Event, EventSink};

    /// Per frame (bpm, run, rate), and the seeks as (frame, payload).
    struct Script<'a> {
        values: &'a dyn Fn(u64) -> [f64; 3],
        seeks: &'a [(u64, EventPayload)],
    }

    /// Runs a playhead for `frames` frames at `sample_rate` in calls whose
    /// sizes cycle through `blocks`, returning every frame's (position,
    /// running, tempo), or only every `keep`-th frame's position when
    /// `keep` > 1 (for long runs).
    fn run(
        sample_rate: f64,
        frames: u64,
        blocks: &[usize],
        script: &Script<'_>,
        keep: u64,
    ) -> Vec<[f64; 3]> {
        let most = blocks.iter().copied().max().unwrap();
        let mut playhead = Playhead::default();
        playhead.prepare(&context(sample_rate, most, 1));
        let mut parameters = vec![0.0; 3 * most];
        let mut signals = vec![0.0; 3 * most];
        let mut sinks: Vec<EventSink> = Vec::new();
        let mut out = Vec::new();
        let mut start = 0u64;
        let mut sizes = blocks.iter().copied().cycle();
        while start < frames {
            let k = (sizes.next().unwrap() as u64).min(frames - start) as usize;
            for i in 0..k {
                let v = (script.values)(start + i as u64);
                for p in 0..3 {
                    parameters[p * k + i] = v[p];
                }
            }
            let events: Vec<Event> = script
                .seeks
                .iter()
                .filter(|(f, _)| (start..start + k as u64).contains(f))
                .map(|&(f, payload)| Event {
                    offset: (f - start) as u32,
                    payload,
                })
                .collect();
            let ends = [events.len()];
            playhead.process(
                &Inputs::new(k, &[], &parameters[..3 * k], &events, &ends),
                &mut Outputs::new(k, &mut signals[..3 * k], &mut sinks),
            );
            for i in 0..k {
                let n = start + i as u64;
                if keep <= 1 || n.is_multiple_of(keep) || n + 1 == frames {
                    out.push([signals[i], signals[k + i], signals[2 * k + i]]);
                }
            }
            start += k as u64;
        }
        out
    }

    fn steady(bpm: f64) -> impl Fn(u64) -> [f64; 3] {
        move |_| [bpm, 1.0, 1.0]
    }

    /// At 120 bpm and 48 kHz beat k is exactly k at frame 24 000·k, every
    /// frame is the formula's value, and `running` and `tempo` say so.
    #[test]
    fn a_steady_tempo_is_exact_on_every_frame() {
        let values = steady(120.0);
        let script = Script {
            values: &values,
            seeks: &[],
        };
        let out = run(48_000.0, 100_000, &[4096], &script, 1);
        for (n, frame) in out.iter().enumerate() {
            assert_eq!(frame[0], (n as f64 * 120.0) / 2_880_000.0, "frame {n}");
            assert_eq!(frame[1], 1.0);
            assert_eq!(frame[2], 120.0);
        }
        for k in 0..4 {
            assert_eq!(out[24_000 * k][0], k as f64);
        }
    }

    /// No accumulated drift over hours: one hour at 132 bpm and 48 kHz
    /// (172.8 million frames, a speed of 132/2 880 000 beats per frame that
    /// no binary fraction holds) is, on every sampled frame, the exact
    /// position correctly rounded, and ends on exactly 7920 beats. A plain
    /// per-frame sum of the same speed drifts measurably by then.
    #[test]
    fn a_steady_tempo_does_not_drift_over_an_hour() {
        let frames = 3600 * 48_000;
        let values = steady(132.0);
        let script = Script {
            values: &values,
            seeks: &[],
        };
        let keep = 48_000;
        let out = run(48_000.0, frames + 1, &[65_536], &script, keep);
        for (i, frame) in out.iter().enumerate() {
            let n = (i as u64 * keep).min(frames);
            assert_eq!(frame[0], (n as f64 * 132.0) / 2_880_000.0, "frame {n}");
        }
        assert_eq!(out.last().unwrap()[0], 7920.0);
        // The plain sum, for comparison.
        let v = 132.0 / 2_880_000.0;
        let naive = (0..frames).fold(0.0f64, |p, _| p + v);
        eprintln!(
            "after 1 h: formula 7920 exactly, plain sum off by {:e} beats",
            naive - 7920.0
        );
        assert!((naive - 7920.0).abs() > 1e-7, "a plain sum drifts: {naive}");
    }

    /// With `rate` changing on every frame (1.25, 0.75, … at 120 bpm and
    /// 48 kHz: 1/19 200 and 1/32 000 beats a frame, neither a binary
    /// fraction), every frame is a segment of one, summed with
    /// compensation: after one hour the position is within 4 units in the
    /// last place of the exact m/12 000 (m frame pairs), where a plain sum
    /// is thousands of times further off.
    #[test]
    fn a_rate_changing_on_every_frame_does_not_drift_over_an_hour() {
        let frames: u64 = 3600 * 48_000;
        let values = |n: u64| [120.0, 1.0, if n.is_multiple_of(2) { 1.25 } else { 0.75 }];
        let script = Script {
            values: &values,
            seeks: &[],
        };
        let keep = 480_000;
        let out = run(48_000.0, frames + 1, &[65_536], &script, keep);
        let mut worst = 0.0f64;
        for (i, frame) in out.iter().enumerate() {
            let n = (i as u64 * keep).min(frames);
            let exact = (n / 2) as f64 / 12_000.0;
            let ulp = f64::EPSILON * exact.abs().max(f64::MIN_POSITIVE);
            let error = (frame[0] - exact).abs();
            assert!(error <= 4.0 * ulp, "frame {n}: {} vs {exact}", frame[0]);
            worst = worst.max(error);
        }
        assert_eq!(out.last().unwrap()[0], 7200.0);
        let (even, odd) = (150.0 / 2_880_000.0, 90.0 / 2_880_000.0);
        let naive = (0..frames).fold(0.0f64, |p, n| p + if n % 2 == 0 { even } else { odd });
        let naive_error = (naive - 7200.0).abs();
        eprintln!(
            "after 1 h: compensated off by at most {worst:e} beats, plain sum by {naive_error:e}"
        );
        assert!(
            naive_error > 1000.0 * worst.max(f64::EPSILON * 7200.0),
            "plain {naive_error} vs compensated {worst}"
        );
    }

    /// The outputs do not depend on how the frames are split into calls:
    /// tempo, rate and run changes (steps and per-frame sweeps), seeks by
    /// Set and Trigger, a reverse and a stop, in calls of 1, 7, 64, 4096
    /// frames and a mixed cycle, are bit-identical.
    #[test]
    fn the_position_does_not_depend_on_the_partition() {
        let values = |n: u64| {
            let bpm = if n < 30_000 { 97.3 } else { 133.0 };
            let run = if (50_000..60_000).contains(&n) {
                0.0
            } else {
                1.0
            };
            let rate = match n {
                0..20_000 => 1.0,
                20_000..40_000 => 0.5 + (n % 977) as f64 / 977.0,
                40_000..45_000 => -2.0,
                _ => 3.0,
            };
            [bpm, run, rate]
        };
        let seeks = [
            (10_000, EventPayload::Set(7.25)),
            (10_000, EventPayload::Set(3.5)),
            (41_000, EventPayload::Trigger),
            (70_000, EventPayload::Set(-2.0)),
            (
                70_001,
                EventPayload::Ramp {
                    to: 9.0,
                    frames: 10,
                    curve: signal_core::Curve::Linear,
                },
            ),
        ];
        let script = Script {
            values: &values,
            seeks: &seeks,
        };
        let reference = run(48_000.0, 90_000, &[4096], &script, 1);
        for blocks in [&[1][..], &[7], &[64], &[1, 300, 2, 4096, 17]] {
            let other = run(48_000.0, 90_000, blocks, &script, 1);
            assert!(
                other
                    .iter()
                    .zip(&reference)
                    .all(|(a, b)| a.map(f64::to_bits) == b.map(f64::to_bits)),
                "{blocks:?}"
            );
        }
        // The last seek of a frame wins; a Trigger rewinds to 0; a Ramp
        // does nothing.
        assert_eq!(reference[10_000][0], 3.5);
        assert_eq!(reference[41_000][0], 0.0);
        assert!(reference[41_001][0] < 0.0, "rate −2 plays backwards");
        assert_eq!(reference[70_000][0], -2.0);
        assert!(reference[70_001][0] > -2.0 && reference[70_001][0] < -1.99);
        // Stopped: held, running 0, tempo 0.
        assert_eq!(reference[50_000][0], reference[59_999][0]);
        assert_eq!(reference[55_000][1..], [0.0, 0.0]);
        assert_eq!(reference[60_000][0], reference[59_999][0]);
        assert!(reference[60_001][0] > reference[60_000][0]);
        assert_eq!(reference[80_000][2], 399.0);
    }

    /// A rebase keeps the position continuous: a tempo change moves the
    /// speed from its frame on, never the position.
    #[test]
    fn a_tempo_change_moves_the_speed_not_the_position() {
        let values = |n: u64| [if n < 24_000 { 120.0 } else { 60.0 }, 1.0, 1.0];
        let script = Script {
            values: &values,
            seeks: &[],
        };
        let out = run(48_000.0, 72_001, &[1000], &script, 1);
        assert_eq!(out[24_000][0], 1.0);
        assert_eq!(out[24_001][0], 1.0 + 60.0 / 2_880_000.0);
        assert_eq!(out[72_000][0], 2.0);
    }

    /// `jump` holds one `Trigger` on each frame a seek lands on (two seeks
    /// on one frame: one), and none for motion, whatever its speed.
    #[test]
    fn a_seek_is_announced_on_jump() {
        let mut playhead = Playhead::default();
        playhead.prepare(&context(48_000.0, 8, 1));
        let parameters = [vec![120.0; 8], vec![1.0; 8], vec![16.0; 8]].concat();
        let events = [
            Event {
                offset: 2,
                payload: EventPayload::Set(0.5),
            },
            Event {
                offset: 2,
                payload: EventPayload::Trigger,
            },
            Event {
                offset: 5,
                payload: EventPayload::Set(3.0),
            },
            Event {
                offset: 6,
                payload: EventPayload::Ramp {
                    to: 1.0,
                    frames: 2,
                    curve: signal_core::Curve::Linear,
                },
            },
        ];
        let mut signals = vec![0.0; 3 * 8];
        let mut sinks = vec![EventSink::per_frame(16, 8)];
        sinks[0].start_call(8);
        playhead.process(
            &Inputs::new(8, &[], &parameters, &events, &[events.len()]),
            &mut Outputs::new(8, &mut signals, &mut sinks),
        );
        let jumps: Vec<(u32, EventPayload)> = sinks[0]
            .events()
            .iter()
            .map(|e| (e.offset, e.payload))
            .collect();
        assert_eq!(
            jumps,
            [(2, EventPayload::Trigger), (5, EventPayload::Trigger)]
        );
        assert_eq!((signals[2], signals[5]), (0.0, 3.0));
    }

    /// The position stops at ±10⁶ beats and comes back from there.
    #[test]
    fn the_position_stops_at_the_bound() {
        let values = |n: u64| [400.0, 1.0, if n < 20 { 16.0 } else { -16.0 }];
        let seeks = [(0, EventPayload::Set(1e6 - 1.0))];
        let script = Script {
            values: &values,
            seeks: &seeks,
        };
        // At 1 Hz, 400·16/60 ≈ 107 beats a frame.
        let out = run(1.0, 40, &[3], &script, 1);
        assert_eq!(out[1][0], 1e6);
        assert_eq!(out[19][0], 1e6);
        assert!(out[21][0] < 1e6);
        assert!(out.iter().all(|f| f[0].abs() <= 1e6));
    }
}
