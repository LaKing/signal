//! `std.edge`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, Range,
};

use crate::{at, event_port, parameter, push_trigger, signal_port};

const THRESHOLD: f64 = 0.5;
const HYSTERESIS: f64 = 0.01;

const RISE: usize = 0;
const FALL: usize = 1;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.edge",
    doc: "Schmitt trigger: the gate rises above threshold + hysteresis/2 and falls below threshold - hysteresis/2, with a trigger on each transition.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Generic, None, None),
        event_port("rise", Direction::Out),
        event_port("fall", Direction::Out),
        signal_port(
            "gate",
            Direction::Out,
            PortKind::Gate,
            None,
            Some(Range { min: 0.0, max: 1.0 }),
        ),
    ],
    parameters: &[
        parameter(
            "threshold",
            PortKind::Generic,
            None,
            -crate::GENERIC,
            crate::GENERIC,
            THRESHOLD,
            Combine::Add,
        ),
        parameter(
            "hysteresis",
            PortKind::Generic,
            None,
            0.0,
            crate::GENERIC,
            HYSTERESIS,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// `std.edge`: a Schmitt trigger that turns a signal into a gate and edge
/// triggers.
///
/// **Ports**
/// - `in` (signal in, generic, unbounded); unconnected it reads 0.
/// - `rise` (event out, trigger): one `Trigger` on each frame the gate turns on.
/// - `fall` (event out, trigger): one `Trigger` on each frame the gate turns off.
/// - `gate` (signal out, gate, `{0, 1}`).
///
/// **Parameters**
/// - `threshold` (generic, no unit, range `[−10⁵, 10⁵]` (ADR 0043), default 0.5, combine `add`).
/// - `hysteresis` (generic, no unit, range `[0, 10⁵]` (ADR 0043), default 0.01, combine
///   `add`): the width `h` of the band around the threshold.
///
/// **Feedthrough:** direct (`in[n]` decides `gate[n]` and the frame-*n* events).
///
/// **Formula** (the gate is off before frame 0, so an input already above the
/// upper level rises on frame 0):
///
/// ```text
/// off → on   when in[n] > threshold[n] + h[n]/2     (trigger on `rise` at n)
/// on  → off  when in[n] < threshold[n] − h[n]/2     (trigger on `fall` at n)
/// gate[n] = 1 when on, else 0
/// ```
///
/// Inside the band the gate keeps its state. With `h = 0` both levels are the
/// threshold, and an input equal to it changes nothing. A NaN input compares
/// false and changes nothing. At most one transition per frame; constant work
/// per frame.
#[derive(Clone, Debug, Default)]
pub struct Edge {
    on: bool,
}

impl DirectPrimitive for Edge {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.on = false;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let (threshold, hysteresis) = (inputs.parameter(0), inputs.parameter(1));
        for n in 0..outputs.frames() {
            let xn = at(x, n, 0.0);
            let level = at(threshold, n, THRESHOLD);
            let half = at(hysteresis, n, HYSTERESIS) / 2.0;
            if !self.on && xn > level + half {
                self.on = true;
                push_trigger(outputs, RISE, n);
            } else if self.on && xn < level - half {
                self.on = false;
                push_trigger(outputs, FALL, n);
            }
            if let Some(gate) = outputs.signal(0).get_mut(n) {
                *gate = if self.on { 1.0 } else { 0.0 };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// AC2c: on a slow sine with noise smaller than the hysteresis, the gate
    /// changes exactly once per crossing, at the frames where the sine passes
    /// the upper and lower levels, and the triggers are on those frames.
    #[test]
    fn hysteresis_on_a_slow_sine() {
        let frames = 4800;
        let (threshold, h) = (0.1, 0.2);
        // 2 Hz at 4800 Hz: 2400 frames per cycle; a ripple of ±0.05 at 1200 Hz.
        let sine = |n: usize| libm::sin(std::f64::consts::TAU * 2.0 * n as f64 / 4800.0);
        let x: Vec<f64> = (0..frames)
            .map(|n| sine(n) + 0.05 * libm::sin(std::f64::consts::TAU * n as f64 / 4.0 + 0.3))
            .collect();
        let mut edge = Edge::default();
        edge.prepare(&context(4800.0, 100, 0));
        let run = render(
            &mut edge,
            &Timeline {
                signals: &[&x],
                parameters: &[&vec![threshold; frames], &vec![h; frames]],
                triggers: &[],
            },
            frames,
            100,
        );
        // The reference: a plain Schmitt trigger over x.
        let (mut on, mut rises, mut falls, mut gate) = (false, vec![], vec![], vec![]);
        for (n, &xn) in x.iter().enumerate() {
            if !on && xn > threshold + h / 2.0 {
                on = true;
                rises.push(n as u64);
            } else if on && xn < threshold - h / 2.0 {
                on = false;
                falls.push(n as u64);
            }
            gate.push(if on { 1.0 } else { 0.0 });
        }
        assert_eq!(run.events[0], rises);
        assert_eq!(run.events[1], falls);
        assert_eq!(run.signals[0], gate);
        // Two cycles: two rises and two falls, despite the ripple.
        assert_eq!((rises.len(), falls.len()), (2, 2));
        // Each rise is where the input first exceeds 0.2, each fall where it
        // first drops below 0.0, and not before.
        for &r in &rises {
            assert!(x[r as usize] > 0.2 && x[r as usize - 1] <= 0.2);
        }
        for &f in &falls {
            assert!(x[f as usize] < 0.0 && x[f as usize - 1] >= 0.0);
        }
        // Without hysteresis the ripple chatters around the threshold.
        let mut edge = Edge::default();
        edge.prepare(&context(4800.0, 100, 0));
        let chatter = render(
            &mut edge,
            &Timeline {
                signals: &[&x],
                parameters: &[&vec![threshold; frames], &vec![0.0; frames]],
                triggers: &[],
            },
            frames,
            100,
        );
        assert!(chatter.events[0].len() > 2);
    }

    #[test]
    fn starts_off_and_rises_on_frame_zero_when_above() {
        let mut edge = Edge::default();
        edge.prepare(&context(48_000.0, 4, 0));
        let run = render(
            &mut edge,
            &Timeline {
                signals: &[&[1.0, 0.496, 0.494, 0.0]],
                parameters: &[&[0.5; 4], &[0.01; 4]],
                triggers: &[],
            },
            4,
            4,
        );
        assert_eq!(run.events, [vec![0], vec![2]]);
        assert_eq!(run.signals[0], [1.0, 1.0, 0.0, 0.0]);
    }
}
