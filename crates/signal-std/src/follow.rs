//! Following a position (ADR 0033): how `std.note_seq` and `std.step_seq8`
//! read their `follow` input, a position in beats (`std.playhead.position`),
//! frame by frame.
//!
//! Each frame compares the position with the previous frame's. Forward by at
//! most [`MAX_JUMP`] beats, the beats crossed, `[previous, current)`, fire
//! what starts inside them, so an event fires on the first frame whose
//! position is past its beat: every beat is crossed by exactly one frame,
//! since consecutive frames share their bound. Not moving fires nothing.
//! A frame with a `Trigger` on `reset` is a **seek**: the playhead's `jump`
//! marks every seek there (`lib.transport` exports it), a short forward
//! one included, which the position alone cannot tell from motion. As a
//! fallback, a move backward, further than [`MAX_JUMP`], across more than
//! [`MAX_LOOPS`] loops, a non-finite value, and the first frame (after
//! `prepare` or a swap that connected `follow`) are seeks too. On a seek
//! nothing fires retroactively, and the next frame resumes from the new
//! place.

/// The furthest a position moves forward in one frame and is still played
/// through, in beats: further is a seek. 1 beat a frame is 2880 times the
/// speed of 120 bpm at 48 kHz.
pub(crate) const MAX_JUMP: f64 = 1.0;

/// The most loops of a pattern one frame plays through; more is a seek,
/// which keeps a frame's work bounded for a very short loop.
pub(crate) const MAX_LOOPS: f64 = 64.0;

/// How the position moved since the previous frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Motion {
    /// A jump, a reverse, or no previous frame: nothing fires.
    Seek,
    /// It did not move.
    Still,
    /// Forward over the beats `[from, to)`.
    Forward { from: f64, to: f64 },
}

/// How a position moved from `last` (the previous frame's, if any) to
/// `now`, for a loop of `period` beats (> 0).
pub(crate) fn motion(last: Option<f64>, now: f64, period: f64) -> Motion {
    match last {
        Some(from) if from.is_finite() && now.is_finite() => {
            if now == from {
                Motion::Still
            } else if now < from || now - from > MAX_JUMP || (now - from) / period > MAX_LOOPS {
                Motion::Seek
            } else {
                Motion::Forward { from, to: now }
            }
        }
        _ => Motion::Seek,
    }
}

/// The start beat of every loop of `period` beats that `[from, to)` can
/// reach, in order: one before the loop of `from` (for a start that rounds
/// below `from`'s loop) up to the loop of `to`. A forward motion reaches at
/// most [`MAX_LOOPS`] + 3 of them.
pub(crate) fn loops(from: f64, to: f64, period: f64) -> impl Iterator<Item = f64> {
    let first = libm::floor(from / period) - 1.0;
    let last = libm::floor(to / period);
    let count = if last >= first {
        ((last - first) as u64).min(MAX_LOOPS as u64 + 2) + 1
    } else {
        0
    };
    (0..count).map(move |i| (first + i as f64) * period)
}

/// The place of `position` in a loop of `period` beats, in `[0, period)`.
pub(crate) fn place(position: f64, period: f64) -> f64 {
    if !position.is_finite() {
        return 0.0;
    }
    let p = position - libm::floor(position / period) * period;
    if (0.0..period).contains(&p) { p } else { 0.0 }
}

/// The loop length a table gives, if it is a usable one (finite, above 0),
/// else 1 beat.
pub(crate) fn period(length: f64) -> f64 {
    if length.is_finite() && length > 0.0 {
        length
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motions_are_forward_still_or_seeks() {
        assert_eq!(motion(None, 0.0, 4.0), Motion::Seek);
        assert_eq!(motion(Some(1.0), 1.0, 4.0), Motion::Still);
        assert_eq!(
            motion(Some(1.0), 1.5, 4.0),
            Motion::Forward { from: 1.0, to: 1.5 }
        );
        assert_eq!(
            motion(Some(1.0), 2.0, 4.0),
            Motion::Forward { from: 1.0, to: 2.0 }
        );
        assert_eq!(
            motion(Some(1.0), 2.0 + 1e-9, 4.0),
            Motion::Seek,
            "more than a beat"
        );
        assert_eq!(motion(Some(1.0), 0.999, 4.0), Motion::Seek, "backwards");
        assert_eq!(motion(Some(1.0), f64::NAN, 4.0), Motion::Seek);
        assert_eq!(motion(Some(0.0), 0.5, 0.001), Motion::Seek, "500 loops");
    }

    #[test]
    fn loops_cover_the_crossed_beats() {
        let starts: Vec<f64> = loops(3.5, 4.5, 2.0).collect();
        assert_eq!(starts, [0.0, 2.0, 4.0]);
        assert_eq!(
            loops(-0.5, 0.25, 1.0).collect::<Vec<_>>(),
            [-2.0, -1.0, 0.0]
        );
        assert_eq!(place(4.5, 2.0), 0.5);
        assert_eq!(place(-0.5, 2.0), 1.5);
        assert_eq!(place(f64::INFINITY, 2.0), 0.0);
        assert_eq!(period(0.0), 1.0);
        assert_eq!(period(3.0), 3.0);
    }

    use crate::pattern::{compile_notes, compile_steps};
    use crate::testing::context;
    use crate::{NoteSeq, Playhead, StepSeq8, Transport};
    use signal_core::{DirectPrimitive, Event, EventPayload, EventSink, Inputs, Outputs, Table};

    const RATE: f64 = 48_000.0;

    fn notes(length: f64, notes: &[[f64; 4]]) -> Table {
        let source = Table::new(4, notes.concat()).unwrap();
        compile_notes(&[length], &source).unwrap()
    }

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

    /// One sequencer's run: per frame [gate, key, hz, velocity, position]
    /// (note_seq) or [step] (step_seq8), and every event output's (frame,
    /// payload) events.
    #[derive(Debug, Default, PartialEq)]
    struct Played {
        signals: Vec<Vec<f64>>,
        events: Vec<Vec<(u64, EventPayload)>>,
    }

    impl Played {
        fn new(signals: usize, events: usize) -> Played {
            Played {
                signals: vec![Vec::new(); signals],
                events: vec![Vec::new(); events],
            }
        }

        fn take(&mut self, start: u64, k: usize, signals: &[f64], sinks: &mut [EventSink]) {
            for (row, out) in self.signals.iter_mut().enumerate() {
                out.extend_from_slice(&signals[row * k..(row + 1) * k]);
            }
            for (list, sink) in self.events.iter_mut().zip(sinks.iter_mut()) {
                assert_eq!(sink.overflows(), 0);
                list.extend(
                    sink.events()
                        .iter()
                        .map(|e| (start + u64::from(e.offset), e.payload)),
                );
                sink.clear();
            }
        }

        /// The frames of the `Trigger`s on event output `port`.
        fn triggers(&self, port: usize) -> Vec<u64> {
            self.events[port]
                .iter()
                .filter(|e| e.1 == EventPayload::Trigger)
                .map(|e| e.0)
                .collect()
        }
    }

    /// What one rig run gives: the note sequencer and the step sequencer
    /// counting `std.transport`'s ticks, and the same two following
    /// `std.playhead`'s position, and the position itself.
    struct Rig {
        ticked: (Played, Played),
        followed: (Played, Played),
        position: Vec<f64>,
    }

    /// The playhead's (bpm, run, rate) per frame, its seeks, and resets
    /// into the following sequencers.
    struct Drive<'a> {
        playhead: &'a dyn Fn(u64) -> [f64; 3],
        seeks: &'a [(u64, EventPayload)],
        resets: &'a [u64],
    }

    /// Runs the rig at 48 kHz for `frames` frames in calls whose sizes
    /// cycle through `blocks`: a transport at 120 bpm and 96 ppq, a
    /// playhead driven by `drive`, each sequencer pair on `melody` and
    /// `beat` (swing `swing`).
    fn rig(
        melody: &Table,
        beat: &Table,
        swing: f64,
        frames: u64,
        blocks: &[usize],
        drive: &Drive<'_>,
    ) -> Rig {
        let most = blocks.iter().copied().max().unwrap();
        let ctx = context(RATE, most, 1);
        let mut transport = Transport::default();
        let mut playhead = Playhead::default();
        let mut seqs = [NoteSeq::default(), NoteSeq::default()];
        let mut drums = [StepSeq8::default(), StepSeq8::default()];
        transport.prepare(&ctx);
        playhead.prepare(&ctx);
        for s in &mut seqs {
            s.prepare(&ctx);
        }
        for d in &mut drums {
            d.prepare(&ctx);
        }
        let mut out = Rig {
            ticked: (Played::new(5, 2), Played::new(1, 9)),
            followed: (Played::new(5, 2), Played::new(1, 9)),
            position: Vec::new(),
        };
        let mut tick_sinks = vec![EventSink::per_frame(16, most); 4];
        let mut note_sinks = vec![EventSink::per_frame(16, most); 2];
        let mut step_sinks = vec![EventSink::per_frame(16, most); 9];
        // tick, reset and follow connected to the followers; outputs not.
        let mut connected = [false; 12];
        connected[2] = true;
        let mut sizes = blocks.iter().copied().cycle();
        let mut start = 0u64;
        while start < frames {
            let k = (sizes.next().unwrap() as u64).min(frames - start) as usize;
            let range = start..start + k as u64;
            // The transport's ticks.
            let parameters: Vec<f64> = [120.0, 0.0, 0.0, 4.0, 96.0]
                .iter()
                .flat_map(|&v| std::iter::repeat_n(v, k))
                .collect();
            for sink in &mut tick_sinks {
                sink.start_call(k);
            }
            transport.process(
                &Inputs::new(k, &[], &parameters, &[], &[]),
                &mut Outputs::new(k, &mut [], &mut tick_sinks),
            );
            let ticks: Vec<Event> = tick_sinks[3].events().to_vec();
            for sink in &mut tick_sinks {
                sink.clear();
            }
            // The playhead's position.
            let mut parameters = vec![0.0; 3 * k];
            for i in 0..k {
                let v = (drive.playhead)(start + i as u64);
                for p in 0..3 {
                    parameters[p * k + i] = v[p];
                }
            }
            let seeks: Vec<Event> = drive
                .seeks
                .iter()
                .filter(|(f, _)| range.contains(f))
                .map(|&(f, payload)| Event {
                    offset: (f - start) as u32,
                    payload,
                })
                .collect();
            let mut signals = vec![0.0; 3 * k];
            playhead.process(
                &Inputs::new(k, &[], &parameters, &seeks, &[seeks.len()]),
                &mut Outputs::new(k, &mut signals, &mut []),
            );
            let position = signals[..k].to_vec();
            out.position.extend_from_slice(&position);
            let resets: Vec<Event> = drive
                .resets
                .iter()
                .filter(|f| range.contains(f))
                .map(|&f| Event {
                    offset: (f - start) as u32,
                    payload: EventPayload::Trigger,
                })
                .collect();
            let zeros = vec![0.0; k];
            for (which, followed) in [(0, false), (1, true)] {
                let (events, ends, input, links): (Vec<Event>, [usize; 2], &[f64], &[bool]) =
                    if followed {
                        let n = resets.len();
                        (resets.clone(), [0, n], &position, &connected)
                    } else {
                        (ticks.clone(), [ticks.len(), ticks.len()], &zeros, &[])
                    };
                let ppq = vec![96.0; k];
                let mut signals = vec![0.0; 5 * k];
                for sink in &mut note_sinks {
                    sink.start_call(k);
                }
                seqs[which].process(
                    &Inputs::new(k, input, &ppq, &events, &ends)
                        .with_table(Some(melody))
                        .with_connected(links),
                    &mut Outputs::new(k, &mut signals, &mut note_sinks),
                );
                let played = if followed {
                    &mut out.followed.0
                } else {
                    &mut out.ticked.0
                };
                played.take(start, k, &signals, &mut note_sinks);
                let settings: Vec<f64> = [96.0, 1.5, swing]
                    .iter()
                    .flat_map(|&v| std::iter::repeat_n(v, k))
                    .collect();
                let mut signals = vec![0.0; k];
                for sink in &mut step_sinks {
                    sink.start_call(k);
                }
                drums[which].process(
                    &Inputs::new(k, input, &settings, &events, &ends)
                        .with_table(Some(beat))
                        .with_connected(links),
                    &mut Outputs::new(k, &mut signals, &mut step_sinks),
                );
                let played = if followed {
                    &mut out.followed.1
                } else {
                    &mut out.ticked.1
                };
                played.take(start, k, &signals, &mut step_sinks);
            }
            start += k as u64;
        }
        out
    }

    /// A melody on the 16th grid (exact frames at 120 bpm and 48 kHz):
    /// touching notes, a take-over, a note across the loop end.
    fn melody() -> Table {
        notes(
            4.0,
            &[
                [60.0, 0.0, 0.5, 0.8],
                [62.0, 0.5, 0.25, 0.5],
                [64.0, 0.75, 1.0, 1.0],
                [65.0, 1.25, 0.5, 0.6],
                [67.0, 2.5, 0.125, 0.7],
                [69.0, 3.5, 1.0, 0.9],
            ],
        )
    }

    fn beat() -> Table {
        steps(
            0.25,
            &[
                (0.9, "x...x...x...x..."),
                (0.7, "....X.......X..x"),
                (0.5, "xxxxxxxxxxxxxxxx"),
            ],
        )
    }

    fn steady(_: u64) -> [f64; 3] {
        [120.0, 1.0, 1.0]
    }

    /// ADR 0033: on a grid that falls on whole frames (16ths and 32nds at
    /// 120 bpm and 48 kHz, swing 50 % = 12 of 24 ticks), a sequencer
    /// following `std.playhead` plays exactly what the same sequencer
    /// counting `std.transport`'s ticks plays, one frame later: an event
    /// fires on the first frame whose position is past its beat, the tick
    /// on the frame of the beat itself. Every gate, key, hz and velocity
    /// sample, every on and off, every lane hit and accent, over three
    /// loops, in calls of 1, 64, 4096 frames and a mixed cycle; `step` (a
    /// place, not an event) equal on the same frame.
    #[test]
    fn following_the_playhead_plays_the_ticks_one_frame_later() {
        let (melody, beat) = (melody(), beat());
        let drive = Drive {
            playhead: &steady,
            seeks: &[],
            resets: &[],
        };
        let frames = 3 * 4 * 24_000 + 100;
        let reference = rig(&melody, &beat, 50.0, frames, &[4096], &drive);
        for (ticked, followed) in [
            (&reference.ticked.0, &reference.followed.0),
            (&reference.ticked.1, &reference.followed.1),
        ] {
            for (row, (a, b)) in ticked.signals.iter().zip(&followed.signals).enumerate() {
                if ticked.signals.len() == 5 && row == 4 || ticked.signals.len() == 1 {
                    continue;
                }
                assert_eq!(a[..a.len() - 1], b[1..], "row {row}");
            }
            for (port, (a, b)) in ticked.events.iter().zip(&followed.events).enumerate() {
                let later: Vec<(u64, EventPayload)> = a
                    .iter()
                    .filter(|e| e.0 + 1 < frames)
                    .map(|&(f, p)| (f + 1, p))
                    .collect();
                assert_eq!(later, *b, "event output {port}");
            }
        }
        assert_eq!(
            reference.ticked.1.signals[0], reference.followed.1.signals[0],
            "step"
        );
        let on = reference.followed.0.triggers(0);
        assert_eq!(&on[..3], [1, 12_001, 18_001]);
        assert_eq!(
            reference.followed.1.triggers(2).len(),
            3 * 16 + 1,
            "and the fourth loop's first"
        );
        // The same in every partition.
        for blocks in [&[1][..], &[64], &[1, 999, 7, 4096]] {
            let other = rig(&melody, &beat, 50.0, frames, blocks, &drive);
            assert_eq!(other.followed.0, reference.followed.0, "{blocks:?}");
            assert_eq!(other.followed.1, reference.followed.1, "{blocks:?}");
        }
    }

    /// A stop releases the sounding note and fires nothing; playing on, the
    /// next note starts on time and the one that spanned the stop is not
    /// restarted. A seek ends the note, fires nothing, and resumes from the
    /// new place, where a note starting exactly there fires on the next
    /// frame. A reverse plays nothing. A reset is a seek.
    #[test]
    fn stops_seeks_reverses_and_resets() {
        let (melody, beat) = (melody(), beat());
        // 120 bpm: beat b at frame 24 000·b. Stopped from beat 1 (frame
        // 24 000, inside the note at 0.75) to frame 30 000; a seek to 2.5
        // at frame 40 000; backwards from frame 60 000 to 70 000; a reset
        // at 90 000.
        let playhead = |n: u64| {
            let run = if (24_000..30_000).contains(&n) {
                0.0
            } else {
                1.0
            };
            let rate = if (60_000..70_000).contains(&n) {
                -1.0
            } else {
                1.0
            };
            [120.0, run, rate]
        };
        let seeks = [(40_000, EventPayload::Set(2.5))];
        let drive = Drive {
            playhead: &playhead,
            seeks: &seeks,
            resets: &[90_000],
        };
        let run = rig(&melody, &beat, 0.0, 120_000, &[256], &drive);
        let notes = &run.followed.0;
        let gate = &notes.signals[0];
        let (on, off) = (notes.triggers(0), notes.triggers(1));
        // Note 64 (0.75 to 1.75) starts at 18 001; the stop at 24 000 is
        // seen at 24 001 (the position did not move): off there.
        assert!(on.contains(&18_001));
        assert!(off.contains(&24_001));
        assert_eq!(gate[24_001], 0.0);
        // Stopped: nothing fires until it moves again (frame 30 001).
        assert!(on.iter().chain(&off).all(|f| !(24_002..30_001).contains(f)));
        // Played on: note 65 at 1.25 is 6000 frames after 1.0, which the
        // position reaches at frame 30 000.
        let after: Vec<u64> = on.iter().copied().filter(|&f| f > 24_001).collect();
        assert_eq!(after[0], 36_001, "65 on time, 64 not restarted");
        // The seek to 2.5 at frame 40 000: note 65 (to 1.75) ends there,
        // nothing fires; note 67 at exactly 2.5 fires on the next frame.
        assert!(off.contains(&40_000));
        assert!(on.contains(&40_001));
        assert_eq!(notes.signals[1][40_001], 67.0);
        // Backwards: an off on the first backward frame (if a note sounds),
        // nothing fires after it until forward again.
        assert!(on.iter().all(|f| !(60_001..70_001).contains(f)));
        assert!(run.position[69_999] < run.position[60_000]);
        assert!(
            run.followed
                .1
                .events
                .iter()
                .flatten()
                .all(|e| !(60_001..70_001).contains(&e.0))
        );
        // The reset is a seek: nothing starts on its frame, nor a hit.
        assert!(!on.contains(&90_000));
        assert!(
            run.followed
                .1
                .events
                .iter()
                .flatten()
                .all(|e| e.0 != 90_000)
        );
    }

    /// A forward seek of less than a beat looks like motion on the position
    /// alone: the steps it skips all fire on its frame, and the last skipped
    /// note starts late. Marked on `reset` (the playhead's `jump`, as
    /// `lib.transport` wires it), it is a seek: nothing fires on its frame,
    /// the sounding note ends, and the next frame plays from the new place.
    #[test]
    fn a_short_forward_seek_marked_on_reset_skips_what_it_jumps_over() {
        let (melody, beat) = (melody(), beat());
        // At frame 12 000 (beat 0.5) a seek to 1.25: 0.75 beats forward.
        let seeks = [(12_000, EventPayload::Set(1.25))];
        let unmarked = Drive {
            playhead: &steady,
            seeks: &seeks,
            resets: &[],
        };
        let marked = Drive {
            resets: &[12_000],
            ..unmarked
        };
        let frames = 40_000;
        let inferred = rig(&melody, &beat, 0.0, frames, &[512], &unmarked);
        let told = rig(&melody, &beat, 0.0, frames, &[512], &marked);
        // Unmarked: the 16ths at 0.5, 0.75 and 1.0 all fire on frame 12 000.
        assert!(inferred.followed.1.triggers(2).contains(&12_000));
        // Marked: nothing on 12 000; the 16th at 1.25 on the next frame.
        let lane: Vec<u64> = told.followed.1.triggers(2);
        assert!(!lane.contains(&12_000), "{lane:?}");
        assert!(lane.contains(&12_001), "{lane:?}");
        let notes = &told.followed.0;
        assert!(!notes.triggers(0).contains(&12_000));
        assert!(
            notes.triggers(0).contains(&12_001),
            "note 65 at 1.25 on time"
        );
        assert_eq!(notes.signals[1][12_001], 65.0);
        assert!(
            notes.triggers(1).contains(&12_000),
            "the sounding note ends"
        );
    }

    /// A swap that connects `follow` to a counting sequencer makes its first
    /// frame a seek (the counted note ends, nothing fires), and the next
    /// frame plays from the position; one that disconnects it ends the
    /// followed note on its first frame and counts ticks again.
    #[test]
    fn a_swap_between_ticks_and_a_position_ends_the_note() {
        let melody = notes(4.0, &[[60.0, 0.0, 4.0, 1.0], [62.0, 2.0, 2.0, 1.0]]);
        let mut seq = NoteSeq::default();
        seq.prepare(&context(RATE, 4, 1));
        let mut connected = [false; 12];
        connected[2] = true;
        let ppq = [1.0; 4];
        // Calls of 4 frames: (follow connected, follow values, ticks).
        let mut call = |linked: bool, follow: [f64; 4], ticks: &[u32]| {
            let events: Vec<Event> = ticks
                .iter()
                .map(|&offset| Event {
                    offset,
                    payload: EventPayload::Trigger,
                })
                .collect();
            let ends = [events.len(), events.len()];
            let mut signals = [0.0; 20];
            let mut sinks = vec![EventSink::per_frame(16, 4); 2];
            for sink in &mut sinks {
                sink.start_call(4);
            }
            let links: &[bool] = if linked { &connected } else { &[] };
            seq.process(
                &Inputs::new(4, &follow, &ppq, &events, &ends)
                    .with_table(Some(&melody))
                    .with_connected(links),
                &mut Outputs::new(4, &mut signals, &mut sinks),
            );
            let at = |s: &EventSink| s.events().iter().map(|e| e.offset).collect::<Vec<_>>();
            (signals[..4].to_vec(), at(&sinks[0]), at(&sinks[1]))
        };
        // Counting: note 60 from the first tick.
        let (gate, on, _) = call(false, [0.0; 4], &[0]);
        assert_eq!((gate, on), (vec![1.0; 4], vec![0]));
        // Connected: a seek on its first frame, then from beat 2 on.
        let (gate, on, off) = call(true, [2.0, 2.1, 2.2, 2.3], &[]);
        assert_eq!(off, [0], "the counted note ends");
        assert_eq!(on, [1], "62 at beat 2, crossed on the next frame");
        assert_eq!(gate, [0.0, 1.0, 1.0, 1.0]);
        // Disconnected: the followed note ends on the first frame.
        let (gate, on, off) = call(false, [0.0; 4], &[]);
        assert_eq!((on, off), (vec![], vec![0]));
        assert_eq!(gate, [0.0; 4]);
    }
}
