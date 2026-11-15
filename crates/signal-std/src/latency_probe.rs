//! `std.latency_probe` (ADR 0055 §2): a click out, the same click found in
//! what comes back: a round trip in frames.
//!
//! **Ports**
//! - `in` (signal in, audio): what comes back.
//! - `click` (signal out, audio): `level` on one frame every `period`
//!   seconds, the first at frame `period`/2 (counted from reset), 0 between.
//! - `frames` (signal out): the last measured delay in frames, held; −1
//!   until one is found.
//! - `found` (signal out): 1 while the last click came back within its
//!   period, else 0.
//!
//! **Parameters:** `period` (s, `[0.25, 10]`, default 1), `level` (`[0, 1]`,
//! default 0.5), `threshold` (`[0.001, 1]`, default 0.05), each read on the
//! frame a click goes out.
//!
//! **Detection:** after a click, the first frame whose |`in`| reaches
//! `threshold`, within the period, is its return: the delay is that frame
//! less the click's (0: a wire). A period without one sets `found` to 0 and
//! keeps the last `frames`. A measurement, not a correlator: anything above
//! the threshold reads as a return. Frames are counted from reset, so the
//! result never depends on how they are split into calls.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, parameter, signal_port};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.latency_probe",
    doc: "A click every period seconds on click; frames is how many frames later in first reaches threshold (a round trip), found whether the last click came back (ADR 0055).",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("click", Direction::Out, PortKind::Audio, None, None),
        signal_port("frames", Direction::Out, PortKind::Generic, None, None),
        signal_port("found", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[
        parameter(
            "period",
            PortKind::Control,
            Some("s"),
            0.25,
            10.0,
            1.0,
            Combine::Replace,
        ),
        parameter(
            "level",
            PortKind::Gain,
            None,
            0.0,
            1.0,
            0.5,
            Combine::Replace,
        ),
        parameter(
            "threshold",
            PortKind::Control,
            None,
            0.001,
            1.0,
            0.05,
            Combine::Replace,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    source: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::ANALYSIS),
};

/// `std.latency_probe`.
#[derive(Clone, Debug, Default)]
pub struct LatencyProbe {
    sample_rate: f64,
    /// Frames until the next click.
    until_click: u64,
    /// Frames since the last click, while its return is awaited.
    waiting: Option<u64>,
    /// The awaited click's period in frames and threshold.
    window: u64,
    threshold: f64,
    frames: f64,
    found: f64,
}

impl LatencyProbe {
    /// The period in frames at `seconds` (at least 1).
    fn period_frames(&self, seconds: f64) -> u64 {
        ((seconds.clamp(0.25, 10.0) * self.sample_rate).round() as u64).max(1)
    }
}

impl DirectPrimitive for LatencyProbe {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.until_click = self.period_frames(1.0) / 2;
        self.waiting = None;
        self.window = 0;
        self.threshold = 0.05;
        self.frames = -1.0;
        self.found = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let frames = inputs.frames();
        let (input, period, level, threshold) = (
            inputs.signal(0),
            inputs.parameter(0),
            inputs.parameter(1),
            inputs.parameter(2),
        );
        for n in 0..frames {
            let mut click = 0.0;
            if self.until_click == 0 {
                // A return still awaited when the next click goes: lost.
                if self.waiting.is_some() {
                    self.found = 0.0;
                }
                let period = self.period_frames(at(period, n, 1.0));
                click = at(level, n, 0.5).clamp(0.0, 1.0);
                self.window = period;
                self.threshold = at(threshold, n, 0.05).clamp(0.001, 1.0);
                self.waiting = Some(0);
                self.until_click = period;
            }
            self.until_click -= 1;
            if let Some(since) = self.waiting {
                if at(input, n, 0.0).abs() >= self.threshold {
                    self.frames = since as f64;
                    self.found = 1.0;
                    self.waiting = None;
                } else if since + 1 >= self.window {
                    self.found = 0.0;
                    self.waiting = None;
                } else {
                    self.waiting = Some(since + 1);
                }
            }
            for (row, value) in [click, self.frames, self.found].into_iter().enumerate() {
                if let Some(out) = outputs.signal(row).get_mut(n) {
                    *out = value;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// The probe's outputs over `frames` at 48 kHz, its `in` the click
    /// delayed by `delay` frames (none: silence), in calls of `block`.
    fn looped(frames: usize, delay: Option<usize>, block: usize) -> Vec<Vec<f64>> {
        // The probe's own clicks, as it emits them, delayed into `in`.
        let mut probe = LatencyProbe::default();
        probe.prepare(&context(48_000.0, frames, 0));
        let silence = vec![0.0; frames];
        let alone = render(
            &mut probe,
            &Timeline {
                signals: &[&silence],
                ..Timeline::default()
            },
            frames,
            frames,
        );
        let input: Vec<f64> = match delay {
            Some(d) => (0..frames)
                .map(|n| if n >= d { alone.signals[0][n - d] } else { 0.0 })
                .collect(),
            None => silence.clone(),
        };
        probe.prepare(&context(48_000.0, block, 0));
        render(
            &mut probe,
            &Timeline {
                signals: &[&input],
                ..Timeline::default()
            },
            frames,
            block,
        )
        .signals
    }

    /// ADR 0055 verification 3: a delay of d frames reads d, for a wire
    /// and for delays up to a period less one; nothing back reads −1 and 0.
    #[test]
    fn a_known_delay_is_measured_exactly() {
        for delay in [0usize, 1, 480, 4799] {
            let out = looped(96_000, Some(delay), 256);
            let (frames, found) = (&out[1], &out[2]);
            assert_eq!(out[0][24_000], 0.5, "the first click at half a period");
            assert_eq!(frames[24_000 + delay], delay as f64, "delay {delay}");
            assert_eq!(found[95_999], 1.0, "delay {delay}");
            assert_eq!(frames[95_999], delay as f64);
        }
        let out = looped(96_000, None, 100);
        assert_eq!(out[1][95_999], -1.0);
        assert_eq!(out[2][95_999], 0.0);
    }

    #[test]
    fn the_split_never_matters() {
        assert_eq!(
            looped(60_000, Some(300), 1),
            looped(60_000, Some(300), 4096)
        );
    }
}
