//! `std.count`.

use signal_core::{
    DirectPrimitive, Direction, EventPayload, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    PortKind, PortShape, PrepareContext,
};

use crate::bridge::{BRIDGE_ROWS, Reading};
use crate::{event_port, port_at};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.count",
    doc: "The event bridge into the visual-rate types: counts its events and sums their values since it was created, output as a reading of 24 integer rows (stamp, generation, age, count, sum at 2^-32, two check values) from which std.particles, std.mesh2d and std.body2d apply exactly what was counted since their previous tick. One connection at scale 1 per bridge input; the plan refuses any other.",
    ports: &[
        event_port("in", Direction::In),
        port_at(
            "window",
            Direction::Out,
            PortShape::Vector(BRIDGE_ROWS as u32),
            1,
            PortKind::Generic,
            None,
            None,
        ),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.count`: the event bridge into divided-rate types (operator decisions
/// operator-request:007, :017, :014 and :020; `docs/module-contract.md`
/// "Events into divided-rate types"; the encoding is [`crate::bridge`]'s).
///
/// A type above rate divisor 1 cannot have event ports (`docs/semantics.md`
/// §16), so the visual-rate types [`Particles`](crate::Particles),
/// [`Mesh2d`](crate::Mesh2d) (`std.mesh2d`) and [`Body2d`](crate::Body2d)
/// take their events through this type: their `emit`, `impulse`, `strike`
/// and `kick` inputs are vectors of 24 at divisor 1, each connected to one
/// `std.count.window` at scale 1: building the plan refuses a scale other
/// than 1, more than one connection and any other source there (ADR 0024).
///
/// **Ports**
/// - `in` (event in): every event counts once, several on one frame
///   included. Its value: `Trigger` 1, `Set(v)` v, `Ramp { to, .. }` to; a
///   non-finite value counts as an event of value 0.
/// - `window` (vector of 24 out, generic; the name is M2.8-7's): at every
///   frame, the reading of [`crate::bridge`]: a stamp drawn from the
///   instance's random stream, the plan version it was created for
///   (`PrepareContext::generation`, used only to recognise a re-created
///   counter), the frames it has run, and the number and summed value of
///   every event since it was created. All only grow; a suspended counter
///   holds its reading (§6), which adds nothing.
///
/// **Ticks.** A consumer at divisor 800 ([`VISUAL_DIVISOR`](crate::VISUAL_DIVISOR))
/// reads the reading at its ticks, every 800 frames from frame 0 of the run,
/// and applies what the same counter counted since the reading it applied
/// last. An event at frame *f* therefore acts at the first tick at or after
/// *f*, ⌈*f*/800⌉·800, 0 to 799 frames late (16.7 ms at 48 kHz), once,
/// whatever the block size, partition or thread count. A counter suspended
/// for any time applies nothing twice; a consumer suspended across its ticks
/// applies, at its first tick after resuming, every event since its last
/// tick, once (so such an event acts at the first tick at or after both its
/// frame and the resume). **Only for types at divisor 800**: a divisor-1
/// type takes events directly (`std.mesh2d_audio`'s `strike`).
///
/// **Swaps and the accepted losses.**
/// - *Counter replacement* (the accepted loss, `docs/semantics.md` §12): a
///   live edit that replaces or removes a std.count instance, including
///   removing it and adding it again at the same address, loses, for each
///   consumer that runs across the edit, at most one visual tick of events:
///   those the old counter counted after the consumer's last tick of it,
///   and those the new counter counted before the consumer's first tick of
///   it. Nothing is applied twice. A counter added again at the same
///   address is a new counter: its creation generation differs.
/// - *A new or replaced consumer* applies nothing a counter counted before
///   the consumer's first tick of it, so at most the events since the
///   previous tick are not applied, with one exception (E2).
/// - *E2* (operator-request:014): if a consumer's first reading of a
///   counter is that counter's very first frame (age 1), it applies that
///   frame's events. A counter suspended at its second frame keeps showing
///   its first frame, so a consumer added later applies that one frame's
///   events once.
/// - *E1* (operator-request:014): a consumer that is suspended while its
///   counter is replaced or removed loses what the old counter counted after
///   the consumer's last tick and what the new counter counted before the
///   consumer's first tick after resuming, however long the suspension.
///   Nothing is applied twice.
/// - *Disconnection*: a consumer that reads its bridge input disconnected
///   (at a tick) applies nothing of what the counter counted before it
///   reads it connected again. A disconnection and a reconnection that both
///   fall between two of its ticks, or while it is suspended, are not seen:
///   the events counted meanwhile are applied once, at its next tick.
/// - *Wrap*: a window of 2¹⁷ events or more since the reading a consumer
///   applied last (about 2.7 s with an event on every frame while the
///   consumer is suspended), or of a saturated count, may have wrapped
///   modulo 2¹¹⁴: it applies nothing, and the consumer counts it in its
///   `lost` output.
/// - *Identity*: counters are told apart by (stamp, generation). The
///   generation is exact for a counter re-created at one address (it
///   saturates at 2⁵⁷ − 1 plan versions). Counters at different addresses
///   differ by a 64-bit draw from their random streams, which two addresses
///   could in principle share (about 2⁻⁶⁴ per pair).
///
/// **Values.** Each value is rounded to a multiple of 2⁻³² (ties to even)
/// and limited to ±2⁶⁴, and summed exactly as an integer modulo 2¹¹⁴. The
/// sum a consumer applies is **deterministic**: exact as an integer, then
/// converted to binary64 once, which is exact up to 2⁵³ units (about 2²¹ in
/// value) and rounds to nearest beyond. A value of at most 2⁻³³ in
/// magnitude adds 0 (exactly 2⁻³³ is a tie, rounded to even; it still
/// counts).
/// Age and count are exact below 2⁵⁷ (counts saturate there).
///
/// **Feedthrough:** direct. Work per frame: its events plus one reading.
#[derive(Clone, Debug, Default)]
pub struct Count {
    reading: Reading,
}

/// An event's value.
pub(crate) fn value(payload: EventPayload) -> f64 {
    let v = match payload {
        EventPayload::Trigger => 1.0,
        EventPayload::Set(v) => v,
        EventPayload::Ramp { to, .. } => to,
    };
    if v.is_finite() { v } else { 0.0 }
}

impl DirectPrimitive for Count {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        let mut rng = ctx.rng.clone();
        self.reading = Reading {
            stamp: rng.next_u64().max(1),
            generation: ctx.generation,
            ..Reading::default()
        };
    }

    fn reset(&mut self) {
        // What events set goes; what the context gave stays.
        self.reading = Reading {
            stamp: self.reading.stamp,
            generation: self.reading.generation,
            ..Reading::default()
        };
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let frames = inputs.frames();
        let events = inputs.events(0);
        let mut next = 0;
        for f in 0..frames {
            while let Some(event) = events.get(next) {
                if event.offset as usize > f {
                    break;
                }
                if event.offset as usize == f {
                    self.reading.count(value(event.payload));
                }
                next += 1;
            }
            self.reading.age = self.reading.age.saturating_add(1);
            let rows = outputs.rows(0, BRIDGE_ROWS);
            for (r, x) in self.reading.encode().into_iter().enumerate() {
                if let Some(out) = rows.get_mut(r * frames + f) {
                    *out = x;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::quantize;
    use crate::testing::context;
    use signal_core::Event;

    /// Runs `count` over `frames` frames of `events` (frame, value pairs as
    /// Set events, value 1 as triggers), one frame per call, and returns
    /// every frame's reading.
    fn run(count: &mut Count, frames: usize, events: &[(usize, Option<f64>)]) -> Vec<Reading> {
        let mut readings = Vec::with_capacity(frames);
        let mut i = 0;
        for f in 0..frames {
            let mut these = Vec::new();
            while let Some(&(at, v)) = events.get(i) {
                if at != f {
                    break;
                }
                these.push(Event {
                    offset: 0,
                    payload: v.map_or(EventPayload::Trigger, EventPayload::Set),
                });
                i += 1;
            }
            let mut out = [0.0; BRIDGE_ROWS];
            count.process(
                &Inputs::new(1, &[], &[], &these, &[these.len()]),
                &mut Outputs::new(1, &mut out, &mut []),
            );
            readings.push(Reading::decode(out).unwrap());
        }
        readings
    }

    fn prepared(generation: u64) -> Count {
        let mut count = Count::default();
        let mut ctx = context(48_000.0, 256, 5);
        ctx.generation = generation;
        count.prepare(&ctx);
        count
    }

    /// At every frame the reading is every event so far: count and sum
    /// (in units of 2⁻³²) against a brute force, the age, the generation.
    #[test]
    fn the_reading_is_every_event_so_far() {
        let mut events = Vec::new();
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for f in 0..4000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            for k in 0..(x % 4) {
                let v = ((x >> 8) % 64) as f64 * 0.1 - 4.0;
                events.push((f, if k == 0 { None } else { Some(v) }));
            }
        }
        let readings = run(&mut prepared(3), 4000, &events);
        for (f, r) in readings.iter().enumerate() {
            let inside = events.iter().filter(|(at, _)| *at <= f);
            let (n, s) = inside.fold((0u64, 0i128), |(n, s), (_, v)| {
                (n + 1, s + quantize(v.unwrap_or(1.0)))
            });
            assert_eq!(
                (r.n, r.sum, r.age, r.generation),
                (n, s as u128 & ((1 << 114) - 1), f as u64 + 1, 3)
            );
        }
    }

    /// 2.4 million events: every element stays an integer below 2¹⁹.
    #[test]
    fn outputs_stay_bounded_over_two_million_events() {
        let mut count = prepared(0);
        let events = [Event {
            offset: 0,
            payload: EventPayload::Set(-7.25),
        }; 2];
        let mut out = [0.0; BRIDGE_ROWS];
        for _ in 0..1_200_000 {
            count.process(
                &Inputs::new(1, &[], &[], &events, &[2]),
                &mut Outputs::new(1, &mut out, &mut []),
            );
            assert!(out.iter().all(|x| (0.0..524_288.0).contains(x)));
        }
        let r = Reading::decode(out).unwrap();
        assert_eq!(r.n, 2_400_000);
        assert_eq!(
            r.sum as i128,
            (-7.25 * 2.4e6 * 4_294_967_296.0) as i128 & ((1 << 114) - 1)
        );
    }

    /// Finding auditor-2-origin-reset: reset clears what events set and
    /// keeps the stamp and generation the context gave, so a render after
    /// reset equals the first.
    #[test]
    fn reset_clears_what_events_set() {
        let mut count = prepared(4);
        let events = [(0, Some(2.0)), (3, None), (3, Some(0.1))];
        let first = run(&mut count, 10, &events);
        count.reset();
        assert_eq!(run(&mut count, 10, &events), first);
        assert_eq!(first[0].generation, 4);
        assert_eq!(first[0].stamp, prepared(0).reading.stamp);
        let mut other = Count::default();
        other.prepare(&context(48_000.0, 256, 6));
        assert_ne!(other.reading.stamp, first[0].stamp);
    }
}
