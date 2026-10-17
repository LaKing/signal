//! `std.step_seq`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    ParameterDescriptor, PortKind, PrepareContext,
};

use crate::{at, event_port, parameter, push_trigger, round_half_up, signal_port, triggered};

const LENGTH: f64 = 8.0;
/// The number of step values.
const STEPS: usize = 8;

const fn step(name: &'static str) -> ParameterDescriptor {
    parameter(name, PortKind::Control, None, -1e6, 1e6, 0.0, Combine::Add)
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.step_seq",
    doc: "Eight-step sequencer: outputs the current step's value; a step trigger advances modulo length, and wrap fires on the step back to 1.",
    ports: &[
        event_port("step", Direction::In),
        event_port("reset", Direction::In),
        signal_port("out", Direction::Out, PortKind::Control, None, None),
        event_port("wrap", Direction::Out),
    ],
    parameters: &[
        parameter(
            "length",
            PortKind::Generic,
            None,
            1.0,
            STEPS as f64,
            LENGTH,
            Combine::Add,
        ),
        step("s1"),
        step("s2"),
        step("s3"),
        step("s4"),
        step("s5"),
        step("s6"),
        step("s7"),
        step("s8"),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// `std.step_seq`: an eight-step sequencer of values.
///
/// **Ports**
/// - `step` (event in, trigger): each frame with at least one `Trigger`
///   advances one step; several on one frame count once.
/// - `reset` (event in, trigger): a frame with a `Trigger` returns to step 1
///   without firing `wrap`.
/// - `out` (signal out, control): the value of the current step.
/// - `wrap` (event out, trigger): one `Trigger` on each frame a `step`
///   trigger moves back to step 1.
///
/// `Set` and `Ramp` events on either input are ignored.
///
/// **Parameters**
/// - `length` (generic, integer, no unit, range `[1, 8]`, default 8, combine
///   `add`): rounded with `round_half_up` and read on each step trigger's frame.
/// - `s1` … `s8` (control, no unit, range `[−1e6, 1e6]`, default 0, combine
///   `add`): the step values, read on every frame, so a change is heard at
///   once.
///
/// **Feedthrough:** direct (a step trigger on frame *n* changes `out[n]`).
///
/// **Formula** (`i` is the 0-based current step, 0 at the start; per frame,
/// in this order):
///
/// ```text
/// reset trigger on n:  i = 0
/// step trigger on n:   i = i + 1, or 0 (and `wrap` at n) if i + 1 ≥ length[n]
/// out[n] = s(i+1)[n]
/// ```
///
/// It starts on step 1, so the first step trigger moves to step 2. A reset on
/// the same frame as a step trigger is applied first, so that frame ends on
/// step 2 (on step 1 with `wrap` when length is 1). When `length` is lowered
/// below the current step, the next step trigger goes back to step 1 (and
/// fires `wrap`); until then the current step keeps sounding. Constant work
/// per frame.
#[derive(Clone, Debug, Default)]
pub struct StepSeq {
    /// The current step, 0-based.
    index: usize,
}

impl DirectPrimitive for StepSeq {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.index = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (steps, reset) = (inputs.events(0), inputs.events(1));
        let length = inputs.parameter(0);
        let (mut next_step, mut next_reset) = (0, 0);
        for n in 0..inputs.frames() {
            if triggered(reset, &mut next_reset, n) {
                self.index = 0;
            }
            if triggered(steps, &mut next_step, n) {
                // NaN converts to 0, which `max(1)` lifts back into range.
                let length =
                    (round_half_up(at(length, n, LENGTH)).clamp(1.0, STEPS as f64) as usize).max(1);
                self.index += 1;
                if self.index >= length {
                    self.index = 0;
                    push_trigger(outputs, 0, n);
                }
            }
            if let Some(out) = outputs.signal(0).get_mut(n) {
                *out = at(inputs.parameter(1 + self.index), n, 0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Run, Timeline, context, render};

    const VALUES: [f64; STEPS] = [110.0, 220.0, 330.0, 440.0, 550.0, 660.0, 770.0, 880.0];

    fn sequence(length: f64, steps: &[u64], reset: &[u64], frames: usize) -> Run {
        let length = vec![length; frames];
        let values: Vec<Vec<f64>> = VALUES.iter().map(|&v| vec![v; frames]).collect();
        let mut parameters: Vec<&[f64]> = vec![&length];
        parameters.extend(values.iter().map(Vec::as_slice));
        let mut seq = StepSeq::default();
        seq.prepare(&context(48_000.0, 5, 0));
        render(
            &mut seq,
            &Timeline {
                signals: &[],
                parameters: &parameters,
                triggers: &[steps, reset],
            },
            frames,
            5,
        )
    }

    /// The step (1-based) sounding on each of `frames`.
    fn steps_at(run: &Run, frames: &[usize]) -> Vec<usize> {
        frames
            .iter()
            .map(|&n| VALUES.iter().position(|&v| v == run.signals[0][n]).unwrap() + 1)
            .collect()
    }

    /// AC2c: starts on step 1, advances on each trigger, wraps modulo length
    /// and fires `wrap` on the step back to 1.
    #[test]
    fn advances_and_wraps_modulo_length() {
        let steps: Vec<u64> = (1..=10).map(|k| 10 * k).collect();
        let run = sequence(4.0, &steps, &[], 110);
        let probes: Vec<usize> = (0..=10).map(|k| 10 * k + 5).collect();
        assert_eq!(steps_at(&run, &probes), [1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3]);
        // The change is on the trigger's frame.
        assert_eq!(run.signals[0][9], VALUES[0]);
        assert_eq!(run.signals[0][10], VALUES[1]);
        assert_eq!(run.events[0], [40, 80]);

        let run = sequence(8.0, &steps, &[], 110);
        assert_eq!(steps_at(&run, &[5, 75, 85, 105]), [1, 8, 1, 3]);
        assert_eq!(run.events[0], [80]);
        // Length 1 stays on step 1 and wraps on every step.
        let run = sequence(1.0, &steps, &[], 110);
        assert!(run.signals[0].iter().all(|&v| v == VALUES[0]));
        assert_eq!(run.events[0], steps);
    }

    /// AC2c: a reset returns to step 1 without `wrap`; on the same frame as a
    /// step it is applied first, so the frame ends on step 2.
    #[test]
    fn a_reset_returns_to_step_one_and_comes_first_on_the_same_frame() {
        let steps: Vec<u64> = (1..=6).map(|k| 10 * k).collect();
        // Alone: after steps at 10, 20, 30 (step 4), reset at 35 → step 1.
        let run = sequence(8.0, &steps, &[35], 70);
        assert_eq!(steps_at(&run, &[34, 35, 45, 55, 65]), [4, 1, 2, 3, 4]);
        assert!(run.events[0].is_empty());
        // On the same frame as the step at 40: reset, then step → step 2.
        let run = sequence(8.0, &steps, &[40], 70);
        assert_eq!(steps_at(&run, &[39, 40, 55, 65]), [4, 2, 3, 4]);
        assert!(run.events[0].is_empty());
        // With length 1 the step after the reset wraps.
        let run = sequence(1.0, &steps, &[40], 70);
        assert_eq!(run.events[0], steps);
    }

    #[test]
    fn lowering_the_length_wraps_on_the_next_step() {
        let mut seq = StepSeq::default();
        seq.prepare(&context(48_000.0, 8, 0));
        let mut parameters = vec![8.0; 8];
        for value in VALUES {
            parameters.extend([value; 8]);
        }
        let trigger = |offset| signal_core::Event {
            offset,
            payload: signal_core::EventPayload::Trigger,
        };
        let mut sinks = vec![signal_core::EventSink::with_capacity(8)];
        let mut out = [0.0; 8];
        // Five steps at length 8: step 6.
        let events: Vec<_> = (0..5).map(trigger).collect();
        seq.process(
            &Inputs::new(8, &[], &parameters, &events, &[5, 5]),
            &mut Outputs::new(8, &mut out, &mut sinks),
        );
        assert_eq!(out[7], VALUES[5]);
        // Length 4 now: step 6 keeps sounding until the next step goes to 1.
        parameters[..8].fill(4.0);
        seq.process(
            &Inputs::new(8, &[], &parameters, &[trigger(3)], &[1, 1]),
            &mut Outputs::new(8, &mut out, &mut sinks),
        );
        assert_eq!(&out[..3], [VALUES[5]; 3]);
        assert_eq!(&out[3..], [VALUES[0]; 5]);
        assert_eq!(sinks[0].events().len(), 1);
    }
}
