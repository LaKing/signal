//! `std.count`.

use signal_core::{
    DirectPrimitive, Direction, EventPayload, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    PortKind, PortShape, PrepareContext,
};

use crate::bridge::{SUM_SCALE_EXPONENT, WINDOW_FRAMES, WINDOW_ROWS, encode};
use crate::{event_port, port_at};

/// Leaves of the window's sum tree: the power of two above [`WINDOW_FRAMES`].
const LEAVES: usize = WINDOW_FRAMES.next_power_of_two();

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.count",
    doc: "The event bridge into the visual-rate types: counts the events of its last 800 frames and sums their values, output as a window [count_hi, count_lo, sum_mantissa, sum_exponent] for std.particles, std.mesh2d and std.body2d. One connection at scale 1 per bridge input.",
    ports: &[
        event_port("in", Direction::In),
        port_at(
            "window",
            Direction::Out,
            PortShape::Vector(WINDOW_ROWS as u32),
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
/// operator-request:007 and operator-request:017; `docs/module-contract.md`
/// "Events into divided-rate types"; the encoding is [`crate::bridge`]'s).
///
/// A type above rate divisor 1 cannot have event ports (`docs/semantics.md`
/// §16), so the visual-rate types [`Particles`](crate::Particles),
/// [`Mesh2d`](crate::Mesh2d) (`std.mesh2d`) and [`Body2d`](crate::Body2d)
/// take their events through this type: their `emit`, `impulse`, `strike`
/// and `kick` inputs are vectors of 4 at divisor 1, each connected to one
/// `std.count.window` at scale 1.
///
/// **Ports**
/// - `in` (event in): every event counts once, several on one frame
///   included. Its value: `Trigger` 1, `Set(v)` v, `Ramp { to, .. }` to; a
///   non-finite value counts as an event of value 0.
/// - `window` (vector of 4 out, generic): for frame *f*, the events of
///   frames (*f* − 800, *f*] (fewer before the instance's 800th frame):
///   `[⌊n/2¹⁹⌋, n mod 2¹⁹, m, e]` with *n* events and their summed value
///   `m·2^e`; `(m, e) = (0, 0)` when the sum is 0.
///
/// **Ticks.** 800 is [`VISUAL_DIVISOR`](crate::VISUAL_DIVISOR): a type at that
/// divisor reads the window at its ticks, every 800 frames from frame 0 of
/// the run, so at a tick it is exactly the events since its previous tick.
/// An event at frame *f* therefore acts at the first tick at or after *f*,
/// ⌈*f*/800⌉·800, 0 to 799 frames late (16.7 ms at 48 kHz), and never twice,
/// whatever the block size, partition or thread count, and wherever the
/// counter started: the window needs no knowledge of the ticks. **Only for
/// types at divisor 800**: a divisor-1 type would read each event on 800
/// frames; those take events directly (`std.mesh2d_audio`'s `strike`).
///
/// **Swaps.** A surviving counter keeps its window; a consumer that is
/// replaced or added reads, at its first tick, the events since the
/// previous tick, which no consumer has applied. Exception, accepted by the
/// operator (operator-request:017, 2026-10-15): a live edit that replaces or
/// removes the counter itself loses the events it counted since its
/// consumer's last tick, at most one visual tick's worth, as every replaced
/// module loses its state (`docs/semantics.md` §12). A consumer suspended
/// across one of its ticks does not apply that tick's window (§6, "Inputs
/// while suspended"), as with any suspended instance; the removal protocol's
/// suspension (§12, F frames before the swap) is the usual way that happens.
///
/// **Values.** Each value is scaled by 2⁻⁶⁴ and summed per frame in event
/// order; the frames' sums go into a fixed binary tree over the window, each
/// node the sum of its two children, recomputed along the one path a frame
/// changes, so a value that leaves the window leaves no residue. Every leaf
/// and node below `f64::MIN_POSITIVE` is flushed to +0: a value below 2⁻⁹⁵⁸
/// in magnitude contributes 0 (it still counts). The sum is exact whenever
/// its partial sums are representable (integers and dyadic values), else
/// within 10·2⁻⁵³·Σ|v|; it cannot overflow (2⁶⁴ largest values would be
/// needed), and no element of `window` approaches the fault guard. The
/// count is exact (*n* < 2³⁸ always holds: an input's events are held in
/// memory sized when the plan is built).
///
/// **Feedthrough:** direct. Work per frame: its events plus 10 additions.
#[derive(Clone, Debug)]
pub struct Count {
    /// Events per frame of the window, by frame mod 800.
    counts: [u32; WINDOW_FRAMES],
    /// The sum tree: node 1 is the root, the leaves start at [`LEAVES`].
    tree: [f64; 2 * LEAVES],
    /// The frame's slot.
    slot: usize,
    /// Events in the window.
    n: u64,
}

impl Default for Count {
    fn default() -> Count {
        Count {
            counts: [0; WINDOW_FRAMES],
            tree: [0.0; 2 * LEAVES],
            slot: 0,
            n: 0,
        }
    }
}

/// `x`, or +0 below the normal range.
fn flush(x: f64) -> f64 {
    if x.abs() < f64::MIN_POSITIVE { 0.0 } else { x }
}

impl Count {
    /// Stores frame slot `self.slot`'s count and scaled sum and returns the
    /// window.
    fn store(&mut self, count: u32, sum: f64) -> [f64; WINDOW_ROWS] {
        if let Some(old) = self.counts.get_mut(self.slot) {
            self.n = self.n - u64::from(*old) + u64::from(count);
            *old = count;
        }
        let mut node = LEAVES + self.slot;
        if let Some(leaf) = self.tree.get_mut(node) {
            *leaf = flush(sum);
        }
        while node > 1 {
            node /= 2;
            let left = self.tree.get(2 * node).copied().unwrap_or(0.0);
            let right = self.tree.get(2 * node + 1).copied().unwrap_or(0.0);
            if let Some(parent) = self.tree.get_mut(node) {
                *parent = flush(left + right);
            }
        }
        self.slot = (self.slot + 1) % WINDOW_FRAMES;
        encode(self.n, self.tree.get(1).copied().unwrap_or(0.0))
    }
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

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        *self = Count::default();
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let frames = inputs.frames();
        let events = inputs.events(0);
        let mut next = 0;
        for f in 0..frames {
            let (mut count, mut sum) = (0u32, 0.0);
            while let Some(event) = events.get(next) {
                if event.offset as usize > f {
                    break;
                }
                if event.offset as usize == f {
                    count = count.saturating_add(1);
                    sum =
                        flush(sum + flush(libm::ldexp(value(event.payload), -SUM_SCALE_EXPONENT)));
                }
                next += 1;
            }
            let window = self.store(count, sum);
            let rows = outputs.rows(0, WINDOW_ROWS);
            for (r, x) in window.into_iter().enumerate() {
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
    use crate::bridge::{decode, effect};
    use signal_core::Event;

    /// Runs `count` over `frames` frames of `events` (frame, value pairs as
    /// Set events, value 1 as triggers), one frame per call, and returns
    /// every frame's window.
    fn run(frames: usize, events: &[(usize, Option<f64>)]) -> Vec<[f64; 4]> {
        let mut count = Count::default();
        let mut windows = Vec::with_capacity(frames);
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
            let mut out = [0.0; 4];
            count.process(
                &Inputs::new(1, &[], &[], &these, &[these.len()]),
                &mut Outputs::new(1, &mut out, &mut []),
            );
            windows.push(out);
        }
        windows
    }

    /// At every frame the window is the events of the last 800 frames:
    /// counts always exact; dyadic values exact.
    #[test]
    fn the_window_is_the_events_of_the_last_800_frames() {
        let mut events = Vec::new();
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for f in 0..4000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            for k in 0..(x % 4) {
                let v = ((x >> 8) % 64) as f64 * 0.125 - 4.0;
                events.push((f, if k == 0 { None } else { Some(v) }));
            }
        }
        let windows = run(4000, &events);
        for (f, window) in windows.iter().enumerate() {
            let from = f.saturating_sub(WINDOW_FRAMES - 1);
            let inside = events.iter().filter(|(at, _)| (from..=f).contains(at));
            let (n, s) = inside.fold((0, 0.0), |(n, s), (_, v)| (n + 1, s + v.unwrap_or(1.0)));
            let w = decode(*window);
            assert_eq!(w.n, n, "frame {f}");
            assert_eq!(effect(w, 1.0), s, "frame {f}");
        }
    }

    /// Non-dyadic values: within the documented 10·2⁻⁵³·Σ|v|.
    #[test]
    fn non_dyadic_sums_are_within_the_bound() {
        let events: Vec<(usize, Option<f64>)> = (0..2400)
            .map(|f| (f, Some(0.1 + f as f64 * 1e-3)))
            .collect();
        let windows = run(2400, &events);
        for (f, window) in windows.iter().enumerate() {
            let from = f.saturating_sub(WINDOW_FRAMES - 1);
            let values: Vec<f64> = (from..=f).map(|g| 0.1 + g as f64 * 1e-3).collect();
            let exact: f64 = values.iter().sum();
            let bound = 10.0 * f64::EPSILON / 2.0 * values.iter().map(|v| v.abs()).sum::<f64>()
                + 2.0 * f64::EPSILON * exact.abs();
            let s = effect(decode(*window), 1.0);
            assert!((s - exact).abs() <= bound, "frame {f}: {s} vs {exact}");
        }
    }

    /// Finding auditor-1-004: 2.4 million events, past 10⁶ and many ring
    /// cycles; at every multiple of 800 the window holds exactly 1600 events
    /// worth 1600, and no element ever leaves the fault guard.
    #[test]
    fn outputs_stay_bounded_over_two_million_events() {
        let mut count = Count::default();
        let frames = 1_200_000;
        let events = [
            Event {
                offset: 0,
                payload: EventPayload::Trigger,
            },
            Event {
                offset: 0,
                payload: EventPayload::Trigger,
            },
        ];
        for f in 0..frames {
            let mut out = [0.0; 4];
            count.process(
                &Inputs::new(1, &[], &[], &events, &[2]),
                &mut Outputs::new(1, &mut out, &mut []),
            );
            assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1e6));
            if f >= WINDOW_FRAMES && f % WINDOW_FRAMES == 0 {
                let w = decode(out);
                assert_eq!((w.n, effect(w, 1.0)), (1600, 1600.0), "frame {f}");
            }
        }
    }

    /// 700 events per frame for 800 frames: 560 000, past 2¹⁹, exactly.
    #[test]
    fn the_count_is_exact_past_2_19_in_one_window() {
        let mut count = Count::default();
        let events: Vec<Event> = (0..700)
            .map(|_| Event {
                offset: 0,
                payload: EventPayload::Trigger,
            })
            .collect();
        let mut out = [0.0; 4];
        for _ in 0..WINDOW_FRAMES {
            count.process(
                &Inputs::new(1, &[], &[], &events, &[700]),
                &mut Outputs::new(1, &mut out, &mut []),
            );
        }
        assert_eq!(&out[..2], &[1.0, 35_712.0]);
        assert!(out.iter().all(|x| x.abs() <= 1e6));
        assert_eq!(decode(out).n, 560_000);
    }

    /// The auditor's two Set(600000) and two Set(1e308) are kept exactly.
    #[test]
    fn payloads_beyond_the_guard_are_kept() {
        let w = decode(run(1, &[(0, Some(600_000.0)), (0, Some(600_000.0))])[0]);
        assert_eq!((w.n, effect(w, 1.0)), (2, 1_200_000.0));
        let window = run(1, &[(0, Some(1e308)), (0, Some(1e308))])[0];
        assert!(window.iter().all(|x| x.is_finite() && x.abs() <= 1e6));
        let (m1, e1) = libm::frexp(1e308);
        assert_eq!(&window[2..], &[m1, f64::from(e1 + 1)]);
    }

    /// Finding auditor-2-tree-subnormals-flush: a value below 2⁻⁹⁵⁸ counts
    /// but adds 0, and no node of the tree is ever subnormal.
    #[test]
    fn subnormal_leaves_are_flushed() {
        let mut count = Count::default();
        let events = [Event {
            offset: 0,
            payload: EventPayload::Set(libm::ldexp(1.0, -1000)),
        }];
        let mut out = [0.0; 4];
        count.process(
            &Inputs::new(1, &[], &[], &events, &[1]),
            &mut Outputs::new(1, &mut out, &mut []),
        );
        let w = decode(out);
        assert_eq!((w.n, w.m, w.e), (1, 0.0, 0));
        assert!(
            count
                .tree
                .iter()
                .all(|x| *x == 0.0 || x.abs() >= f64::MIN_POSITIVE)
        );
    }
}
