//! `std.prefer` (ADR 0035 §5, ADR 0037 §6): an input that falls back to a
//! value of its own. A project wires its own value into `b` and exports `a`:
//! alone, `a` is unconnected and `out` is `b`; included and driven, it is
//! the host's. Which one is chosen is a property of the plan
//! (`Inputs::is_connected`), so it changes only at a swap, at its frame.

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PortShape, PrepareContext, RateDivisor,
};

use crate::{at, event_port, push_trigger, signal_port};

/// Signal inputs.
const A: usize = 0;
const B: usize = 1;
/// Event inputs.
const A_EVENTS: usize = 0;
const B_EVENTS: usize = 1;
/// The inputs' places among the descriptor's ports.
const A_PORT: usize = 0;
const B_PORT: usize = 1;
const A_EVENTS_PORT: usize = 2;
const B_EVENTS_PORT: usize = 3;
/// Event outputs.
const EVENTS: usize = 0;
const SWITCHED: usize = 1;

/// A master-rate event port of any events.
const fn any_events(name: &'static str, direction: Direction) -> signal_core::PortDescriptor {
    signal_core::PortDescriptor {
        name,
        direction,
        shape: PortShape::Events,
        rate: RateDivisor::ONE,
        kind: PortKind::Generic,
        unit: None,
        range: None,
    }
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.prefer",
    doc: "An input with a fallback: out is a exactly while a is connected, else b; events forwards a_events while that is connected, else b_events; switched carries one trigger when the choice changes at a swap, or when a swap gives the chosen input other sources.",
    ports: &[
        signal_port("a", Direction::In, PortKind::Generic, None, None),
        signal_port("b", Direction::In, PortKind::Generic, None, None),
        any_events("a_events", Direction::In),
        any_events("b_events", Direction::In),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
        any_events("events", Direction::Out),
        event_port("switched", Direction::Out),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.prefer`: the generic fallback for an exported input (ADR 0035 §5).
/// It is not the transport idiom: a playhead follows a position itself
/// (`std.playhead.follow`, ADR 0037 §6).
///
/// **Ports**
/// - `a`, `b` (signal in, generic): the preferred input and the fallback.
/// - `a_events`, `b_events` (event in, generic): the same for events.
/// - `out` (signal out, generic): `a` while `a` is connected (whatever it
///   carries, 0 included), else `b`; an exact copy, bit for bit.
/// - `events` (event out, generic): `a_events`' events while `a_events` is
///   connected, else `b_events`', each as it arrived (same frame, payload
///   and order).
/// - `switched` (event out, trigger): one `Trigger` on the first frame
///   after a swap that changed a choice (`a` connected or disconnected,
///   `a_events` likewise), or that gave a chosen input other sources
///   (`Inputs::sources_changed`, ADR 0037 §6): a rewired source is a switch
///   too. None on an instance's first frame (nothing was chosen before).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct. Constant work per frame and per event; the
/// choice depends only on the plan, never on the values, so the output does
/// not depend on how frames are split into calls.
#[derive(Clone, Debug, Default)]
pub struct Prefer {
    /// The last call's choices (`a`, `a_events`), once it has run.
    chosen: Option<(bool, bool)>,
}

impl DirectPrimitive for Prefer {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.chosen = None;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let choice = (
            inputs.is_connected(A_PORT),
            inputs.is_connected(A_EVENTS_PORT),
        );
        let (row, port) = if choice.0 { (A, A_PORT) } else { (B, B_PORT) };
        let (events, events_port) = if choice.1 {
            (A_EVENTS, A_EVENTS_PORT)
        } else {
            (B_EVENTS, B_EVENTS_PORT)
        };
        // Choices and sources change only at a swap, which starts a call.
        let switched = self.chosen.is_some_and(|before| before != choice)
            || (self.chosen.is_some()
                && (inputs.sources_changed(port) || inputs.sources_changed(events_port)));
        self.chosen = Some(choice);
        if switched && inputs.frames() > 0 {
            push_trigger(outputs, SWITCHED, 0);
        }
        let source = inputs.signal(row);
        for n in 0..inputs.frames() {
            if let Some(out) = outputs.signal(0).get_mut(n) {
                *out = at(source, n, 0.0);
            }
        }
        if let Some(sink) = outputs.events(EVENTS) {
            for event in inputs.events(events) {
                sink.push(*event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signal_core::{Event, EventPayload, EventSink};

    /// One call: `a`, `b` rows of `frames`, events on a_events then
    /// b_events, connected flags of the four inputs and their changed
    /// sources; returns `out`, the forwarded events and the switches.
    fn call(
        prefer: &mut Prefer,
        frames: usize,
        (a, b): (&[f64], &[f64]),
        (a_events, b_events): (&[Event], &[Event]),
        connected: [bool; 4],
        changed: [bool; 4],
    ) -> (Vec<f64>, Vec<Event>, Vec<Event>) {
        let signals = [a, b].concat();
        let events = [a_events, b_events].concat();
        let ends = [a_events.len(), events.len()];
        let connected = [connected.as_slice(), &[false; 3]].concat();
        let changed = [changed.as_slice(), &[false; 3]].concat();
        let inputs = Inputs::new(frames, &signals, &[], &events, &ends)
            .with_connected(&connected)
            .with_sources_changed(&changed);
        let mut out = vec![0.0; frames];
        let mut sinks = vec![
            EventSink::per_frame(16, frames),
            EventSink::per_frame(16, frames),
        ];
        for sink in &mut sinks {
            sink.start_call(frames);
        }
        prefer.process(&inputs, &mut Outputs::new(frames, &mut out, &mut sinks));
        (out, sinks[0].events().to_vec(), sinks[1].events().to_vec())
    }

    fn set(offset: u32, value: f64) -> Event {
        Event {
            offset,
            payload: EventPayload::Set(value),
        }
    }

    #[test]
    fn out_is_a_while_connected_else_b_bit_for_bit() {
        let mut prefer = Prefer::default();
        let a = [0.0, -0.0, 1.5];
        let b = [7.0, f64::MIN_POSITIVE, -3.0];
        let (out, _, switched) = call(&mut prefer, 3, (&a, &b), (&[], &[]), [false; 4], [false; 4]);
        assert_eq!(out.map_bits(), b.map_bits());
        assert!(switched.is_empty(), "nothing was chosen before");
        let (out, _, switched) = call(
            &mut prefer,
            3,
            (&a, &b),
            (&[], &[]),
            [true, true, false, false],
            [true, false, false, false],
        );
        assert_eq!(out.map_bits(), a.map_bits(), "a connected outputs 0 as 0");
        assert_eq!(switched.len(), 1, "a connected at the swap: one switch");
        assert_eq!(switched[0].offset, 0);
        let (_, _, switched) = call(
            &mut prefer,
            3,
            (&a, &b),
            (&[], &[]),
            [true, true, false, false],
            [false; 4],
        );
        assert!(switched.is_empty(), "no change, no switch");
        // Rewired: the chosen input's sources changed at a swap.
        let (_, _, switched) = call(
            &mut prefer,
            2,
            (&a[..2], &b[..2]),
            (&[], &[]),
            [true, true, false, false],
            [true, false, false, false],
        );
        assert_eq!(switched.len(), 1, "a rewired chosen input switches");
        // The fallback's sources changing is not heard: no switch.
        let (_, _, switched) = call(
            &mut prefer,
            2,
            (&a[..2], &b[..2]),
            (&[], &[]),
            [true, true, false, false],
            [false, true, false, false],
        );
        assert!(switched.is_empty());
    }

    #[test]
    fn events_follow_their_own_choice() {
        let mut prefer = Prefer::default();
        let a_events = [set(0, 1.0), set(2, 2.0)];
        let b_events = [set(1, 9.0)];
        let (_, events, _) = call(
            &mut prefer,
            3,
            (&[0.0; 3], &[0.0; 3]),
            (&a_events, &b_events),
            [false, false, false, true],
            [false; 4],
        );
        assert_eq!(events, b_events);
        let (_, events, switched) = call(
            &mut prefer,
            3,
            (&[0.0; 3], &[0.0; 3]),
            (&a_events, &b_events),
            [false, false, true, true],
            [false; 4],
        );
        assert_eq!(events, a_events);
        assert_eq!(switched.len(), 1);
    }

    trait Bits {
        fn map_bits(&self) -> Vec<u64>;
    }
    impl Bits for [f64] {
        fn map_bits(&self) -> Vec<u64> {
            self.iter().map(|x| x.to_bits()).collect()
        }
    }
    impl Bits for Vec<f64> {
        fn map_bits(&self) -> Vec<u64> {
            self.iter().map(|x| x.to_bits()).collect()
        }
    }
    impl<const N: usize> Bits for [f64; N] {
        fn map_bits(&self) -> Vec<u64> {
            self.iter().map(|x| x.to_bits()).collect()
        }
    }
}
