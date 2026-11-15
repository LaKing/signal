//! `std.sampler` (ADR 0056 §2): plays a sample pattern (`kind = "sample"`,
//! a WAV in the project folder, read when the project is loaded and swapped
//! in with the plan) on a trigger.
//!
//! **Ports**
//! - `trigger` (events in): each `Trigger` starts the sample at `start`,
//!   cutting the voice playing, which fades out over [`FADE`] frames (a
//!   raised cosine) so the cut does not click.
//! - `rate` (signal in): frames of the sample per frame, `[0, 4]`; 1 while
//!   it is unconnected. Between frames, linear interpolation.
//! - `l`, `r` (signals out): the sample's channels (a mono sample on both),
//!   times `gain`.
//!
//! **Parameters:** `gain` (`[0, 4]`, default 1), `start` (`[0, 1]` of the
//! length, read at each trigger), `loop` (0 or 1: at the end, back to
//! `start`; else the voice stops).
//!
//! No table (no pattern), silence. Bit-exact; `process` allocates nothing.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, Table, TableRule,
};

use crate::{at, event_port, parameter, signal_port, triggered};

/// The kind of pattern `std.sampler` takes.
pub const SAMPLE: &str = "sample";

/// The rule `std.sampler` is registered with.
pub const SAMPLE_RULE: TableRule = TableRule {
    kind: SAMPLE,
    compile: compile_sample,
};

/// The frames a cut voice fades out over.
pub const FADE: usize = 64;

/// A sample's table is its frames, one row per frame of its channels.
pub fn compile_sample(header: &[f64], rows: &Table) -> Result<Table, String> {
    if header.is_empty() || rows.rows() == 0 {
        return Err(
            "its WAV was not read (the project was read without its folder, or the file is empty)"
                .to_owned(),
        );
    }
    if !(1..=2).contains(&rows.width()) {
        return Err("a sample is mono or stereo".to_owned());
    }
    Ok(rows.clone())
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.sampler",
    doc: "Plays a sample pattern (a WAV) from start on each trigger at rate, linear between frames, a cut voice faded over 64 frames; loop repeats it (ADR 0056).",
    ports: &[
        event_port("trigger", Direction::In),
        signal_port("rate", Direction::In, PortKind::Control, None, None),
        signal_port("l", Direction::Out, PortKind::Audio, None, None),
        signal_port("r", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "gain",
            PortKind::Gain,
            None,
            0.0,
            4.0,
            1.0,
            Combine::Multiply,
        ),
        parameter(
            "start",
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.0,
            Combine::Replace,
        ),
        parameter(
            "loop",
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.0,
            Combine::Replace,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    source: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::GENERATOR),
};

/// A voice: its position in frames of the sample, and how it ends.
#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    position: f64,
    /// Frames of its fade done (`None`: not fading).
    fade: Option<usize>,
    looping: bool,
    start: f64,
}

/// `std.sampler`.
#[derive(Clone, Debug, Default)]
pub struct Sampler {
    voice: Option<Voice>,
    fading: Option<Voice>,
}

/// The value of channel `channel` of `table` at a fractional `position`,
/// linear between frames; past the end 0, or the start's frames when it
/// loops.
fn read(table: &Table, position: f64, channel: usize, wrap: Option<f64>) -> f64 {
    let frames = table.rows();
    let index = position.floor();
    let frac = position - index;
    let frame = |i: f64| -> f64 {
        let i = match wrap {
            Some(start) if i >= frames as f64 => start.floor() + (i - frames as f64),
            _ => i,
        };
        if i < 0.0 || i >= frames as f64 {
            return 0.0;
        }
        let row = table.row(i as usize);
        row.get(channel.min(row.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0.0)
    };
    frame(index) * (1.0 - frac) + frame(index + 1.0) * frac
}

/// The fade's gain after `done` of [`FADE`] frames: a raised cosine from 1
/// to 0.
fn fade_gain(done: usize) -> f64 {
    let x = (done as f64 / FADE as f64).min(1.0);
    0.5 * (1.0 + libm::cos(core::f64::consts::PI * x))
}

impl Sampler {
    /// Advances `voice` by `rate` in a table of `frames`; whether it plays on.
    fn advance(voice: &mut Voice, rate: f64, frames: f64) -> bool {
        voice.position += rate;
        if voice.position >= frames {
            if voice.looping && frames > voice.start {
                voice.position = voice.start + (voice.position - frames) % (frames - voice.start);
            } else {
                return false;
            }
        }
        if let Some(done) = voice.fade.as_mut() {
            *done += 1;
            if *done >= FADE {
                return false;
            }
        }
        true
    }
}

impl DirectPrimitive for Sampler {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.voice = None;
        self.fading = None;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let table = inputs.table().filter(|t| t.rows() > 0);
        let frames = table.map_or(0.0, |t| t.rows() as f64);
        let (trigger, rate, gain, start, looping) = (
            inputs.events(0),
            inputs.signal(0),
            inputs.parameter(0),
            inputs.parameter(1),
            inputs.parameter(2),
        );
        let rate_connected = inputs.is_connected(1);
        let mut next = 0;
        for n in 0..inputs.frames() {
            if triggered(trigger, &mut next, n) && table.is_some() {
                if let Some(mut old) = self.voice.take() {
                    old.fade = Some(0);
                    self.fading = Some(old);
                }
                let start = at(start, n, 0.0).clamp(0.0, 1.0) * frames;
                self.voice = Some(Voice {
                    position: start.floor(),
                    fade: None,
                    looping: at(looping, n, 0.0) >= 0.5,
                    start: start.floor(),
                });
            }
            let mut out = [0.0; 2];
            if let Some(table) = table {
                for voice in [&self.voice, &self.fading].into_iter().flatten() {
                    let level = voice.fade.map_or(1.0, fade_gain);
                    let wrap = voice.looping.then_some(voice.start);
                    for (channel, value) in out.iter_mut().enumerate() {
                        *value += level * read(table, voice.position, channel, wrap);
                    }
                }
            }
            let g = at(gain, n, 1.0);
            for (row, value) in out.into_iter().enumerate() {
                if let Some(slot) = outputs.signal(row).get_mut(n) {
                    *slot = g * value;
                }
            }
            // Unconnected, the rate is 1 (the descriptor's port 1).
            let step = if rate_connected {
                at(rate, n, 0.0)
            } else {
                1.0
            };
            let step = if step.is_finite() {
                step.clamp(0.0, 4.0)
            } else {
                0.0
            };
            if let Some(voice) = self.voice.as_mut()
                && !Self::advance(voice, step, frames)
            {
                self.voice = None;
            }
            if let Some(voice) = self.fading.as_mut()
                && !Self::advance(voice, step, frames)
            {
                self.fading = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::context;
    use signal_core::{Event, EventPayload, EventSink};

    /// Runs a sampler over `table` with triggers at `at`, `rate` everywhere,
    /// in calls of `block`: `(l, r)`.
    fn run(
        table: &Table,
        triggers: &[u32],
        rate: f64,
        looping: f64,
        frames: usize,
        block: usize,
    ) -> (Vec<f64>, Vec<f64>) {
        let mut sampler = Sampler::default();
        sampler.prepare(&context(48_000.0, block, 0));
        let (mut l, mut r) = (Vec::new(), Vec::new());
        let mut start = 0;
        while start < frames {
            let k = block.min(frames - start);
            let events: Vec<Event> = triggers
                .iter()
                .filter(|&&t| (start..start + k).contains(&(t as usize)))
                .map(|&t| Event {
                    offset: t - start as u32,
                    payload: EventPayload::Trigger,
                })
                .collect();
            let signals = vec![rate; k];
            let parameters: Vec<f64> = [vec![1.0; k], vec![0.0; k], vec![looping; k]].concat();
            let mut out = vec![f64::NAN; 2 * k];
            let ends = [events.len()];
            sampler.process(
                &Inputs::new(k, &signals, &parameters, &events, &ends)
                    .with_table(Some(table))
                    .with_connected(&[true, true, false, false]),
                &mut Outputs::new(k, &mut out, &mut Vec::<EventSink>::new()),
            );
            l.extend_from_slice(&out[..k]);
            r.extend_from_slice(&out[k..]);
            start += k;
        }
        (l, r)
    }

    fn ramp(frames: usize) -> Table {
        Table::new(1, (0..frames).map(|i| (i + 1) as f64 / 100.0).collect()).unwrap()
    }

    /// ADR 0056 verification 2: from the trigger's frame, sample for sample.
    #[test]
    fn a_trigger_plays_the_sample_frame_for_frame() {
        let table = ramp(10);
        let (l, r) = run(&table, &[3], 1.0, 0.0, 16, 4);
        assert_eq!(&l[..3], [0.0; 3]);
        assert_eq!(
            &l[3..13],
            (1..=10).map(|i| i as f64 / 100.0).collect::<Vec<_>>()
        );
        assert_eq!(&l[13..], [0.0; 3], "it stops at its end");
        assert_eq!(l, r, "mono on both");
    }

    #[test]
    fn a_retrigger_fades_the_cut_voice_and_loops_repeat() {
        let table = Table::new(1, vec![1.0; 200]).unwrap();
        let (l, _) = run(&table, &[0, 100], 1.0, 0.0, 300, 64);
        assert_eq!(l[99], 1.0);
        assert!(
            (l[100] - 2.0).abs() < 1e-12,
            "the new voice and the old at full"
        );
        assert!(l[150] > 1.0 && l[150] < 2.0, "fading");
        assert_eq!(l[170], 1.0, "the cut voice gone after 64 frames");
        let (l, _) = run(&ramp(10), &[0], 1.0, 1.0, 25, 7);
        assert_eq!(l[10], l[0], "a loop starts again");
    }

    #[test]
    fn the_split_never_matters() {
        let table =
            Table::new(2, (0..400).map(|i| ((i * 7) % 13) as f64 / 13.0).collect()).unwrap();
        let a = run(&table, &[5, 90, 91, 150], 0.7, 1.0, 600, 1);
        let b = run(&table, &[5, 90, 91, 150], 0.7, 1.0, 600, 256);
        assert_eq!(a, b);
    }
}
