//! `std.merge` (ADR 0054).

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PrepareContext,
};

use crate::event_port;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.merge",
    doc: "Event merger: passes every event of its four inputs to out, unchanged; in a frame a's first, then b's, c's and d's, each in its own order (one input takes one connection, ADR 0054).",
    ports: &[
        event_port("a", Direction::In),
        event_port("b", Direction::In),
        event_port("c", Direction::In),
        event_port("d", Direction::In),
        event_port("out", Direction::Out),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::EVENT),
};

/// `std.merge`: four event inputs onto one output (ADR 0054: one input takes
/// one connection, so events from several sources meet here).
///
/// **Ports**
/// - `a`, `b`, `c`, `d` (event in): any events (`Trigger`, `Set`, `Ramp`).
/// - `out` (event out): each input event, unchanged, on its own frame.
///
/// **Order:** within a frame, `a`'s events first, then `b`'s, `c`'s and
/// `d`'s, each input's in the order it delivers them. The order is the
/// ports', never the sources' addresses.
///
/// **Feedthrough:** direct (an event on frame *n* is passed on frame *n*).
/// An output keeps at most 16 events per frame, as every event output; more
/// is an overflow (`docs/semantics.md` §4, §7). Stateless; work in
/// proportion to the events.
#[derive(Clone, Debug, Default)]
pub struct Merge;

impl DirectPrimitive for Merge {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let ins = [
            inputs.events(0),
            inputs.events(1),
            inputs.events(2),
            inputs.events(3),
        ];
        let mut next = [0usize; 4];
        let Some(sink) = outputs.events(0) else {
            return;
        };
        for n in 0..inputs.frames() {
            for (events, next) in ins.iter().zip(next.iter_mut()) {
                while let Some(event) = events.get(*next) {
                    if event.offset as usize > n {
                        break;
                    }
                    sink.push(*event);
                    *next += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signal_core::{Event, EventPayload, EventSink};

    fn set(offset: u32, value: f64) -> Event {
        Event {
            offset,
            payload: EventPayload::Set(value),
        }
    }

    fn trigger(offset: u32) -> Event {
        Event {
            offset,
            payload: EventPayload::Trigger,
        }
    }

    fn merge(frames: usize, events: &[Event], ends: &[usize; 4]) -> Vec<Event> {
        let mut sinks = vec![EventSink::with_capacity(64)];
        Merge.process(
            &Inputs::new(frames, &[], &[], events, ends),
            &mut Outputs::new(frames, &mut [], &mut sinks),
        );
        sinks[0].events().to_vec()
    }

    /// Every event passes unchanged, frame by frame; in a frame `a`'s, then
    /// `b`'s, `c`'s and `d`'s, each in its own order.
    #[test]
    fn passes_every_event_in_port_order_within_a_frame() {
        // a: Set(1) at 2, Trigger at 5; b: Set(2) at 2, Set(3) at 2; c: none; d: Trigger at 0, Set(4) at 5.
        let events = [
            set(2, 1.0),
            trigger(5),
            set(2, 2.0),
            set(2, 3.0),
            trigger(0),
            set(5, 4.0),
        ];
        let out = merge(8, &events, &[2, 4, 4, 6]);
        assert_eq!(
            out,
            [
                trigger(0),
                set(2, 1.0),
                set(2, 2.0),
                set(2, 3.0),
                trigger(5),
                set(5, 4.0)
            ]
        );
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(merge(4, &[], &[0, 0, 0, 0]).is_empty());
    }
}
