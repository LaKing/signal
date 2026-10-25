//! `std.latch`, `std.to_events` and `std.route`: the core set's value events
//! (ADR 0027 §1, "Events"). Merging is event fan-in: several sources into
//! one event input arrive merged in the canonical order of ADR 0016 §3, so
//! there is no merge type. Direct, at rate divisor 1, category `core`. Every
//! value they output or emit is clamped to `±1e6`, the engine's magnitude
//! bound (`docs/semantics.md` §7 checks emitted `set` and `ramp` values too),
//! so they never fault on their own.

use signal_core::{
    Combine, DirectPrimitive, Direction, Event, EventPayload, Feedthrough, Inputs,
    ModuleDescriptor, Outputs, PortDescriptor, PortKind, PortShape, PrepareContext, RateDivisor,
};

use crate::math::bounded;
use crate::{at, event_port, parameter, signal_port, triggered};

/// A master-rate event port carrying values (`Set`).
const fn value_events(name: &'static str, direction: Direction) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape: PortShape::Events,
        rate: RateDivisor::ONE,
        kind: PortKind::Generic,
        unit: None,
        range: None,
    }
}

/// Pushes `payload` on frame `n` of event output `port`.
fn push(outputs: &mut Outputs<'_>, port: usize, n: usize, payload: EventPayload) {
    if let Some(sink) = outputs.events(port) {
        sink.push(Event {
            offset: u32::try_from(n).unwrap_or(u32::MAX),
            payload,
        });
    }
}

/// `payload` with its value clamped to `±1e6`.
fn bounded_payload(payload: EventPayload) -> EventPayload {
    match payload {
        EventPayload::Set(v) => EventPayload::Set(bounded(v)),
        EventPayload::Ramp { to, frames, curve } => EventPayload::Ramp {
            to: bounded(to),
            frames,
            curve,
        },
        EventPayload::Trigger => EventPayload::Trigger,
    }
}

const LATCH: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.latch",
    doc: "Holds the value of the last Set event as a signal (initially 0), clamped to +-1e6.",
    ports: &[
        value_events("in", Direction::In),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.latch`: a `Set` event's value, held as a signal.
///
/// **Ports**
/// - `in` (event in, generic): each `Set(v)` sets the held value; several on
///   one frame apply in canonical order (ADR 0016 §3), so the last wins.
///   `Trigger` and `Ramp` events are ignored.
/// - `out` (signal out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct (a `Set` on frame *n* shows in `out[n]`).
///
/// **Formula:** `h = 0` after `prepare`; on frame *n*, `h = clamp(v, ±1e6)`
/// for each `Set(v)` at *n*, in order; `out[n] = h`. Constant work per frame
/// and event.
#[derive(Clone, Debug, Default)]
pub struct Latch {
    held: f64,
}

impl DirectPrimitive for Latch {
    fn describe() -> ModuleDescriptor {
        LATCH
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.held = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let mut events = inputs.events(0).iter().peekable();
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            while let Some(event) = events.next_if(|e| e.offset as usize <= n) {
                if let EventPayload::Set(v) = event.payload {
                    self.held = bounded(v);
                }
            }
            *out = self.held;
        }
    }
}

const TO_EVENTS: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.to_events",
    doc: "Signal to Set events: emits Set(in) on a trigger, or (on_change) when in moved more than threshold from the last value sent; at most one per frame.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Generic, None, None),
        event_port("trigger", Direction::In),
        value_events("out", Direction::Out),
    ],
    parameters: &[
        parameter(
            "threshold",
            PortKind::Generic,
            None,
            0.0,
            crate::GENERIC,
            0.0,
            Combine::Add,
        ),
        crate::choice(
            parameter(
                "on_change",
                PortKind::Gate,
                None,
                0.0,
                1.0,
                1.0,
                Combine::Replace,
            ),
            crate::OFF_ON,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.to_events`: a signal as `Set` events.
///
/// **Ports**
/// - `in` (signal in, generic, unbounded); unconnected it reads 0.
/// - `trigger` (event in, trigger): a frame with a `Trigger` sends the value.
///   `Set` and `Ramp` events are ignored.
/// - `out` (event out, generic): `Set` events.
///
/// **Parameters**
/// - `threshold` (generic, range `[0, 10⁵]` (ADR 0043), default 0, combine `add`).
/// - `on_change` (gate, range `[0, 1]`, default 1, combine `replace`): above
///   0, a change sends.
///
/// **Feedthrough:** direct (the value of frame *n* is sent at *n*).
///
/// **Formula** (`x = in[n]`, as received; `last` the input at the frame sent
/// last, none after `prepare`): on frame *n* send `Set(clamp(x, ±1e6))` and
/// set `last = x` if there is a trigger at *n*, or if `on_change[n] > 0` and
/// (`last` is none or `|x − last| > threshold[n]`). A change is measured on
/// the input itself; only the value sent is clamped. At most one event per
/// frame, so the first frame sends when `on_change` is on. Constant work per
/// frame.
#[derive(Clone, Debug, Default)]
pub struct ToEvents {
    last: Option<f64>,
}

impl DirectPrimitive for ToEvents {
    fn describe() -> ModuleDescriptor {
        TO_EVENTS
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.last = None;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (x, trigger) = (inputs.signal(0), inputs.events(0));
        let (threshold, on_change) = (inputs.parameter(0), inputs.parameter(1));
        let mut next = 0;
        for n in 0..inputs.frames() {
            let x = at(x, n, 0.0);
            let changed = at(on_change, n, 1.0) > 0.0
                && self
                    .last
                    .is_none_or(|last| (x - last).abs() > at(threshold, n, 0.0));
            if triggered(trigger, &mut next, n) || changed {
                push(outputs, 0, n, EventPayload::Set(bounded(x)));
                self.last = Some(x);
            }
        }
    }
}

const ROUTE: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.route",
    doc: "Routes each event to output a if gate > 0 at its frame, else to b, in order; values clamped to +-1e6.",
    ports: &[
        value_events("in", Direction::In),
        signal_port("gate", Direction::In, PortKind::Gate, None, None),
        value_events("a", Direction::Out),
        value_events("b", Direction::Out),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.route`: events to one of two outputs by a gate.
///
/// **Ports**
/// - `in` (event in, generic): any events, in canonical order.
/// - `gate` (signal in, gate, unbounded); unconnected it reads 0, so
///   everything goes to `b`.
/// - `a`, `b` (event out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** each event at frame *n* goes to `a` if `gate[n] > 0`, else to
/// `b`, at frame *n*, in its input order; `Set` and `Ramp` values are
/// clamped to `±1e6`, `Trigger` passes as is. More events on one frame than
/// an output's sink holds are the engine's `event_overflow` fault (§7).
/// Constant work per event.
#[derive(Clone, Debug, Default)]
pub struct Route;

impl DirectPrimitive for Route {
    fn describe() -> ModuleDescriptor {
        ROUTE
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let gate = inputs.signal(0);
        for event in inputs.events(0) {
            let n = event.offset as usize;
            let port = if at(gate, n, 0.0) > 0.0 { 0 } else { 1 };
            push(outputs, port, n, bounded_payload(event.payload));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signal_core::{Curve, EventSink};

    fn set(offset: u32, v: f64) -> Event {
        Event {
            offset,
            payload: EventPayload::Set(v),
        }
    }

    fn trigger(offset: u32) -> Event {
        Event {
            offset,
            payload: EventPayload::Trigger,
        }
    }

    /// One call of `frames` frames; the signal outputs and each event output's events.
    #[allow(clippy::too_many_arguments)]
    fn call<P: DirectPrimitive>(
        p: &mut P,
        frames: usize,
        signals: &[f64],
        parameters: &[f64],
        events: &[Event],
        event_ends: &[usize],
        signal_outputs: usize,
        event_outputs: usize,
    ) -> (Vec<f64>, Vec<Vec<Event>>) {
        let mut out = vec![f64::NAN; signal_outputs * frames];
        let mut sinks = vec![EventSink::with_capacity(64); event_outputs];
        p.process(
            &Inputs::new(frames, signals, parameters, events, event_ends),
            &mut Outputs::new(frames, &mut out, &mut sinks),
        );
        (out, sinks.iter().map(|s| s.events().to_vec()).collect())
    }

    /// AC4: the last Set of a frame is held, clamped; triggers and ramps are
    /// ignored; the hold carries across calls.
    #[test]
    fn latch_holds_the_last_set() {
        let mut l = Latch::default();
        let ramp = Event {
            offset: 3,
            payload: EventPayload::Ramp {
                to: 9.0,
                frames: 4,
                curve: Curve::Linear,
            },
        };
        let events = [set(1, 0.5), set(1, 0.75), trigger(2), ramp, set(3, 5e6)];
        let (out, _) = call(&mut l, 4, &[], &[], &events, &[5], 1, 0);
        assert_eq!(out, [0.0, 0.75, 0.75, 1e6]);
        let (out, _) = call(&mut l, 2, &[], &[], &[], &[0], 1, 0);
        assert_eq!(out, [1e6, 1e6]);
    }

    /// AC4: a change beyond the threshold sends, a smaller one does not, a
    /// trigger always sends, and on_change 0 sends on triggers only.
    #[test]
    fn to_events_sends_on_change_or_trigger() {
        let mut t = ToEvents::default();
        let x = [1.0, 1.05, 1.2, 1.2, 3e6];
        let params = [[0.1; 5], [1.0; 5]].concat();
        let (_, events) = call(&mut t, 5, &x, &params, &[trigger(3)], &[1], 0, 1);
        assert_eq!(
            events[0],
            [set(0, 1.0), set(2, 1.2), set(3, 1.2), set(4, 1e6)]
        );
        let mut t = ToEvents::default();
        let params = [[0.0; 5], [0.0; 5]].concat();
        let (_, events) = call(&mut t, 5, &x, &params, &[trigger(1)], &[1], 0, 1);
        assert_eq!(events[0], [set(1, 1.05)]);
        // A change beyond the bound is measured on the input; the value sent
        // is clamped.
        let mut t = ToEvents::default();
        let params = [[0.1; 2], [1.0; 2]].concat();
        let (_, events) = call(&mut t, 2, &[2e6, 3e6], &params, &[], &[0], 0, 1);
        assert_eq!(events[0], [set(0, 1e6), set(1, 1e6)]);
    }

    /// AC4: each event goes to a or b by the gate at its frame, in order,
    /// with values clamped.
    #[test]
    fn route_splits_by_the_gate() {
        let gate = [1.0, 0.0, 0.5];
        let events = [set(0, 1.0), trigger(1), set(2, -7e6), set(2, 2.0)];
        let (_, out) = call(&mut Route, 3, &gate, &[], &events, &[4], 0, 2);
        assert_eq!(out[0], [set(0, 1.0), set(2, -1e6), set(2, 2.0)]);
        assert_eq!(out[1], [trigger(1)]);
    }
}
