//! `std.clock_div`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, event_port, parameter, push_trigger, round_half_up, triggered};

const DIVIDE: f64 = 2.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.clock_div",
    doc: "Trigger divider: passes the 1st, (n+1)th, (2n+1)th, ... input trigger since the last reset; a same-frame reset comes first.",
    ports: &[
        event_port("in", Direction::In),
        event_port("reset", Direction::In),
        event_port("out", Direction::Out),
    ],
    parameters: &[parameter(
        "divide",
        PortKind::Generic,
        None,
        1.0,
        256.0,
        DIVIDE,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
};

/// `std.clock_div`: a trigger divider.
///
/// **Ports**
/// - `in` (event in, trigger): the clock. Each frame with at least one
///   `Trigger` counts as one input trigger; several on one frame count once.
/// - `reset` (event in, trigger): a frame with a `Trigger` restarts the count.
/// - `out` (event out, trigger).
///
/// `Set` and `Ramp` events on either input are ignored.
///
/// **Parameters**
/// - `divide` (generic, integer, no unit, range `[1, 256]`, default 2, combine
///   `add`): rounded with `round_half_up` and read on each input trigger's frame.
///
/// **Feedthrough:** direct (an input trigger on frame *n* is passed on frame *n*).
///
/// **Formula** (`c` counts the input triggers since the last reset, 0 at the
/// start; per frame, in this order):
///
/// ```text
/// reset trigger on n:  c = 0
/// input trigger on n:  emit on `out` at n if c mod divide[n] = 0;  c = c + 1
/// ```
///
/// So it emits on the 1st, (n+1)th, (2n+1)th, … input trigger since the last
/// reset. **A reset on the same frame as an input trigger is applied first**:
/// that input trigger is the 1st of the new count and is passed. A change of
/// `divide` takes effect on the next input trigger, against the same count.
/// Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct ClockDiv {
    count: u64,
}

impl DirectPrimitive for ClockDiv {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.count = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (clock, reset) = (inputs.events(0), inputs.events(1));
        let divide = inputs.parameter(0);
        let (mut next_clock, mut next_reset) = (0, 0);
        for n in 0..inputs.frames() {
            if triggered(reset, &mut next_reset, n) {
                self.count = 0;
            }
            if triggered(clock, &mut next_clock, n) {
                // NaN converts to 0, which `max(1)` lifts back into range.
                let divide = (round_half_up(at(divide, n, DIVIDE)).clamp(1.0, 256.0) as u64).max(1);
                if self.count.is_multiple_of(divide) {
                    push_trigger(outputs, 0, n);
                }
                self.count = self.count.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn divide(clock: &[u64], reset: &[u64], divide: &[f64], frames: usize) -> Vec<u64> {
        let mut div = ClockDiv::default();
        div.prepare(&context(48_000.0, 7, 0));
        let run = render(
            &mut div,
            &Timeline {
                signals: &[],
                parameters: &[divide],
                triggers: &[clock, reset],
            },
            frames,
            7,
        );
        run.events.into_iter().next().unwrap()
    }

    /// AC2c: divide by 3 passes the 1st, 4th, 7th, … trigger.
    #[test]
    fn passes_every_nth_trigger_starting_with_the_first() {
        let clock: Vec<u64> = (0..12).map(|k| 10 * k + 2).collect();
        let out = divide(&clock, &[], &[3.0; 120], 120);
        assert_eq!(out, [2, 32, 62, 92]);
        // divide 1 passes everything; 2.5 rounds half up to 3.
        assert_eq!(divide(&clock, &[], &[1.0; 120], 120), clock);
        assert_eq!(divide(&clock, &[], &[2.5; 120], 120), out);
    }

    /// AC2c: a reset restarts the count; on the same frame as an input
    /// trigger it is applied first, so that trigger passes.
    #[test]
    fn a_reset_restarts_the_count_and_comes_first_on_the_same_frame() {
        let clock: Vec<u64> = (0..10).map(|k| 10 * k).collect();
        // Triggers 0..9 at frames 0, 10, …, 90; divide 4.
        // Without reset: 0, 40, 80.
        assert_eq!(divide(&clock, &[], &[4.0; 100], 100), [0, 40, 80]);
        // Reset at 30, alone: count restarts, so 30 is the 1st again.
        assert_eq!(divide(&clock, &[25], &[4.0; 100], 100), [0, 30, 70]);
        // Reset on frame 30 with trigger 3: applied first, 30 passes.
        assert_eq!(divide(&clock, &[30], &[4.0; 100], 100), [0, 30, 70]);
        // Reset on frame 40, the trigger that would pass anyway, and on 50.
        assert_eq!(divide(&clock, &[40, 50], &[4.0; 100], 100), [0, 40, 50, 90]);
    }

    #[test]
    fn several_triggers_on_one_frame_count_once() {
        let mut div = ClockDiv::default();
        div.prepare(&context(48_000.0, 4, 0));
        let trigger = |offset| signal_core::Event {
            offset,
            payload: signal_core::EventPayload::Trigger,
        };
        let events = [trigger(0), trigger(0), trigger(1), trigger(2)];
        let mut sinks = vec![signal_core::EventSink::with_capacity(8)];
        div.process(
            &Inputs::new(4, &[], &[2.0; 4], &events, &[4, 4]),
            &mut Outputs::new(4, &mut [], &mut sinks),
        );
        let frames: Vec<u32> = sinks[0].events().iter().map(|e| e.offset).collect();
        assert_eq!(frames, [0, 2]);
    }
}
