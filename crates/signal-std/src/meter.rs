//! `std.meter` (ADR 0056 §3): hierarchical musical time, bars, sections and
//! phrases, from a position in beats (`std.playhead.position`), so every
//! level is exact from the playhead and follows its tempo and jumps.
//!
//! **Ports**
//! - `position` (signal in): beats since 0.
//! - Signals out: `bar` (whole bars), `beat_in_bar` (0 ≤ x < `beats_per_bar`,
//!   fractional), `bar_phase` (0 ≤ x < 1), `section`, `bar_in_section`,
//!   `phrase`, `section_in_phrase` (whole numbers; negative positions count
//!   down from −1).
//! - Events out: `bar_start`, `section_start`, `phrase_start`, on the frame
//!   the bar, section or phrase index rises (one event however far a jump
//!   goes forward, none for a jump back); on the first frame of a run, the
//!   ones whose index is reached there (a run from 0 starts all three).
//!
//! **Parameters:** `beats_per_bar` (1–32, default 4), `bars_per_section`
//! (1–256, default 8), `sections_per_phrase` (1–64, default 4), whole
//! numbers (rounded), read each frame.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, event_port, parameter, push_trigger, signal_port};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.meter",
    doc: "Bars, sections and phrases from a position in beats: bar, beat_in_bar, bar_phase, section, bar_in_section, phrase, section_in_phrase, and bar_start, section_start, phrase_start events as each index rises (ADR 0056).",
    ports: &[
        signal_port(
            "position",
            Direction::In,
            PortKind::Control,
            Some("beats"),
            None,
        ),
        signal_port("bar", Direction::Out, PortKind::Generic, None, None),
        signal_port(
            "beat_in_bar",
            Direction::Out,
            PortKind::Generic,
            Some("beats"),
            None,
        ),
        signal_port("bar_phase", Direction::Out, PortKind::Phase, None, None),
        signal_port("section", Direction::Out, PortKind::Generic, None, None),
        signal_port(
            "bar_in_section",
            Direction::Out,
            PortKind::Generic,
            None,
            None,
        ),
        signal_port("phrase", Direction::Out, PortKind::Generic, None, None),
        signal_port(
            "section_in_phrase",
            Direction::Out,
            PortKind::Generic,
            None,
            None,
        ),
        event_port("bar_start", Direction::Out),
        event_port("section_start", Direction::Out),
        event_port("phrase_start", Direction::Out),
    ],
    parameters: &[
        parameter(
            "beats_per_bar",
            PortKind::Control,
            None,
            1.0,
            32.0,
            4.0,
            Combine::Replace,
        ),
        parameter(
            "bars_per_section",
            PortKind::Control,
            None,
            1.0,
            256.0,
            8.0,
            Combine::Replace,
        ),
        parameter(
            "sections_per_phrase",
            PortKind::Control,
            None,
            1.0,
            64.0,
            4.0,
            Combine::Replace,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    source: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// The largest index the meter counts to: positions beyond read as it.
const MAX_INDEX: f64 = 1e12;

/// `std.meter`.
#[derive(Clone, Debug, Default)]
pub struct Meter {
    /// The bar, section and phrase indices of the previous frame; `None`
    /// before the first.
    last: Option<[f64; 3]>,
}

/// A whole-number parameter, rounded and clamped.
fn whole(values: &[f64], n: usize, fallback: f64, max: f64) -> f64 {
    at(values, n, fallback).round().clamp(1.0, max)
}

impl DirectPrimitive for Meter {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.last = None;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (position, beats, bars, sections) = (
            inputs.signal(0),
            inputs.parameter(0),
            inputs.parameter(1),
            inputs.parameter(2),
        );
        for n in 0..inputs.frames() {
            let p = at(position, n, 0.0);
            let p = if p.is_finite() { p } else { 0.0 };
            let beats_per_bar = whole(beats, n, 4.0, 32.0);
            let bars_per_section = whole(bars, n, 8.0, 256.0);
            let sections_per_phrase = whole(sections, n, 4.0, 64.0);
            let bars_total = (p / beats_per_bar).clamp(-MAX_INDEX, MAX_INDEX);
            let bar = bars_total.floor();
            let beat_in_bar = (p - bar * beats_per_bar).clamp(0.0, beats_per_bar);
            let bar_phase = (bars_total - bar).clamp(0.0, 1.0 - f64::EPSILON);
            let section = (bar / bars_per_section).floor();
            let bar_in_section = bar - section * bars_per_section;
            let phrase = (section / sections_per_phrase).floor();
            let section_in_phrase = section - phrase * sections_per_phrase;
            let now = [bar, section, phrase];
            for (row, value) in [
                bar,
                beat_in_bar,
                bar_phase,
                section,
                bar_in_section,
                phrase,
                section_in_phrase,
            ]
            .into_iter()
            .enumerate()
            {
                if let Some(out) = outputs.signal(row).get_mut(n) {
                    *out = value;
                }
            }
            for (port, level) in now.iter().enumerate() {
                let started = match self.last {
                    // A run's first frame starts what begins exactly there.
                    None => match port {
                        0 => beat_in_bar == 0.0,
                        1 => beat_in_bar == 0.0 && bar_in_section == 0.0,
                        _ => {
                            beat_in_bar == 0.0 && bar_in_section == 0.0 && section_in_phrase == 0.0
                        }
                    },
                    Some(last) => last.get(port).is_some_and(|before| level > before),
                };
                if started {
                    push_trigger(outputs, port, n);
                }
            }
            self.last = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// The meter's run over `position` (4/4, 2 bars a section, 2 sections a
    /// phrase) in calls of `block`.
    fn run(position: &[f64], block: usize) -> crate::testing::Run {
        let mut meter = Meter::default();
        meter.prepare(&context(48_000.0, block, 0));
        let frames = position.len();
        let (beats, bars, sections) = (vec![4.0; frames], vec![2.0; frames], vec![2.0; frames]);
        render(
            &mut meter,
            &Timeline {
                signals: &[position],
                parameters: &[&beats, &bars, &sections],
                ..Timeline::default()
            },
            frames,
            block,
        )
    }

    /// ADR 0056 verification 3: hand-computed levels and boundaries.
    #[test]
    fn the_levels_follow_the_position() {
        // Beat 0, 3.5, 4 (bar 1), 8 (bar 2: section 1), 16 (bar 4: phrase 1), 21.
        let position = [0.0, 3.5, 4.0, 8.0, 16.0, 21.0];
        let out = run(&position, 64);
        assert_eq!(out.signals[0], [0.0, 0.0, 1.0, 2.0, 4.0, 5.0], "bar");
        assert_eq!(
            out.signals[1],
            [0.0, 3.5, 0.0, 0.0, 0.0, 1.0],
            "beat_in_bar"
        );
        assert_eq!(
            out.signals[2],
            [0.0, 0.875, 0.0, 0.0, 0.0, 0.25],
            "bar_phase"
        );
        assert_eq!(out.signals[3], [0.0, 0.0, 0.0, 1.0, 2.0, 2.0], "section");
        assert_eq!(
            out.signals[4],
            [0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            "bar_in_section"
        );
        assert_eq!(out.signals[5], [0.0, 0.0, 0.0, 0.0, 1.0, 1.0], "phrase");
        assert_eq!(
            out.signals[6],
            [0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            "section_in_phrase"
        );
        assert_eq!(out.events[0], [0, 2, 3, 4, 5], "bar_start");
        assert_eq!(out.events[1], [0, 3, 4], "section_start");
        assert_eq!(out.events[2], [0, 4], "phrase_start");
    }

    #[test]
    fn a_jump_forward_starts_once_and_a_jump_back_never() {
        let position = [5.0, 5.5, 40.0, 40.5, 2.0, 2.5, 4.0];
        let out = run(&position, 3);
        assert!(
            out.events[0].is_empty() || out.events[0][0] != 0,
            "mid-bar: no start"
        );
        assert_eq!(
            out.events[0],
            [2, 6],
            "forward once; back none; then the next bar"
        );
        assert_eq!(out.events[1], [2]);
    }

    #[test]
    fn the_split_never_matters() {
        let position: Vec<f64> = (0..5000).map(|n| n as f64 * 0.013).collect();
        let a = run(&position, 1);
        let b = run(&position, 777);
        assert_eq!(a.signals, b.signals);
        assert_eq!(a.events, b.events);
    }
}
