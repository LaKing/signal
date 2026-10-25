//! `std.random`: the core set's triggered randomness (ADR 0027 §1). Direct,
//! at rate divisor 1, category `core`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, RngStream,
};

use crate::{at, event_port, parameter, signal_port, triggered};

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.random",
    doc: "On a trigger, a uniform value in [lo, hi) from the instance's seeded random stream, held until the next trigger.",
    ports: &[
        event_port("trigger", Direction::In),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[
        parameter(
            "lo",
            PortKind::Generic,
            None,
            -crate::GENERIC,
            crate::GENERIC,
            0.0,
            Combine::Add,
        ),
        parameter(
            "hi",
            PortKind::Generic,
            None,
            -crate::GENERIC,
            crate::GENERIC,
            1.0,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// A uniform value of `[lo, hi)` for a draw `u` in `[0, 1)`: `lo + (hi − lo)·u`,
/// or the largest double below `hi` where that rounds up to `hi`. Reversed
/// bounds give `[hi, lo)`; equal bounds give `lo`.
pub(crate) fn uniform(lo: f64, hi: f64, u: f64) -> f64 {
    let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    if a == b {
        return lo;
    }
    let v = a + (b - a) * u;
    if v >= b { b.next_down() } else { v }
}

/// `std.random`: a triggered random value, held (ADR 0008).
///
/// **Ports**
/// - `trigger` (event in, trigger): each frame with at least one `Trigger`
///   draws once. `Set` and `Ramp` events are ignored.
/// - `out` (signal out, generic, within `±1e6`).
///
/// **Parameters**
/// - `lo`, `hi` (generic, range `[−10⁵, 10⁵]` (ADR 0043), defaults 0 and 1, combine
///   `add`): read on trigger frames (and on the first frame).
///
/// **Feedthrough:** direct (a trigger on frame *n* sets `out[n]`).
///
/// **Formula** (`u = next_f64()` of the instance's stream
/// `RngStream::derive(seed, address)`, on the 53-bit grid in `[0, 1)`):
///
/// ```text
/// first frame:     h = min(lo, hi)                 (no draw)
/// trigger on n:    h = uniform(lo[n], hi[n], u)
/// out[n]         = h
/// uniform(lo, hi, u) = v = lo + (hi − lo)·u,  or hi.next_down() if v ≥ hi    (lo < hi)
///                      uniform(hi, lo, u)                                     (lo > hi)
///                      lo                                                     (lo = hi)
/// ```
///
/// So a drawn value is in `[lo, hi)` of its trigger frame (`[hi, lo)` for
/// reversed bounds), never `hi` even where the product rounds up to it.
/// **The value is held**: a change of `lo` or `hi` without a trigger does
/// not move the output, so a held value can lie outside bounds changed after
/// its trigger, until the next trigger. Every value is within `±1e6`, the
/// engine's magnitude bound. One draw per trigger frame, so the output
/// depends only on the seed, the address and the trigger frames. `reset`
/// restarts the stream. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Random {
    /// The stream as `prepare` received it, and the stream being drawn from.
    /// `None` before `prepare`, when a trigger draws nothing.
    streams: Option<(RngStream, RngStream)>,
    /// The held value, `None` until the first frame.
    held: Option<f64>,
}

impl DirectPrimitive for Random {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.streams = Some((ctx.rng.clone(), ctx.rng.clone()));
        self.held = None;
    }

    fn reset(&mut self) {
        if let Some((initial, stream)) = &mut self.streams {
            stream.clone_from(initial);
        }
        self.held = None;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (trigger, lo, hi) = (inputs.events(0), inputs.parameter(0), inputs.parameter(1));
        let mut next = 0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let (lo, hi) = (at(lo, n, 0.0), at(hi, n, 1.0));
            let mut held = self.held.unwrap_or(lo.min(hi));
            if triggered(trigger, &mut next, n)
                && let Some((_, stream)) = &mut self.streams
            {
                held = uniform(lo, hi, stream.next_f64());
            }
            self.held = Some(held);
            *out = held;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render, stream};

    /// 1 − 2⁻⁵³, the largest draw.
    const U_MAX: f64 = 1.0 - f64::EPSILON / 2.0;

    /// AC5 (auditor-1-random-exclusive-upper-bound): the upper bound is never
    /// reached, including where `lo + (hi − lo)·u` rounds to `hi`.
    #[test]
    fn uniform_never_reaches_hi() {
        let hi = 1.0000000000000002;
        assert_eq!(1.0 + (hi - 1.0) * 0.75, hi, "the counterexample rounds up");
        assert_eq!(uniform(1.0, hi, 0.75), 1.0);
        assert!(uniform(0.0, 1e6, U_MAX) < 1e6);
        assert!(uniform(-1e6, 1e6, U_MAX) < 1e6);
        assert_eq!(uniform(0.0, 1.0, 0.0), 0.0);
        for u in [0.0, 0.5, U_MAX] {
            assert_eq!(uniform(3.0, 3.0, u), 3.0);
            let v = uniform(2.0, 1.0, u);
            assert!((1.0..2.0).contains(&v), "{v}");
        }
        let mut draws = stream(9);
        let mut bounds = stream(10);
        for _ in 0..100_000 {
            let mut bound = || 2e6 * bounds.next_f64() - 1e6;
            let (lo, hi) = (bound(), bound());
            let v = uniform(lo, hi, draws.next_f64());
            if lo == hi {
                assert_eq!(v, lo);
            } else {
                assert!((lo.min(hi)..lo.max(hi)).contains(&v), "{lo} {hi} {v}");
            }
        }
    }

    fn random(lo: &[f64], hi: &[f64], triggers: &[u64], block: usize) -> Vec<f64> {
        let mut r = Random::default();
        r.prepare(&context(48_000.0, block, 3));
        render(
            &mut r,
            &Timeline {
                signals: &[],
                parameters: &[lo, hi],
                triggers: &[triggers],
            },
            lo.len(),
            block,
        )
        .signals[0]
            .clone()
    }

    /// AC5: each trigger frame draws once from the instance's stream, and
    /// before the first trigger the output is the bottom of the range.
    #[test]
    fn draws_from_the_instance_stream() {
        let out = random(&[-2.0; 6], &[2.0; 6], &[1, 3, 3, 4], 4);
        let mut s = stream(3);
        let a = uniform(-2.0, 2.0, s.next_f64());
        let b = uniform(-2.0, 2.0, s.next_f64());
        let c = uniform(-2.0, 2.0, s.next_f64());
        assert_eq!(out, [-2.0, a, a, b, c, c]);
    }

    /// AC5 (auditor-1-random-held-value): changing lo or hi without a
    /// trigger does not move the held value; the next trigger uses the new
    /// bounds.
    #[test]
    fn holds_the_value_taken_at_the_trigger() {
        let lo = [0.0, 0.0, 0.0, -5.0, -5.0, -5.0];
        let hi = [1.0, 1.0, 2.0, 2.0, 2.0, 2.0];
        let out = random(&lo, &hi, &[0, 5], 2);
        let mut s = stream(3);
        let first = uniform(0.0, 1.0, s.next_f64());
        assert!(out[..5].iter().all(|v| v.to_bits() == first.to_bits()));
        assert_eq!(out[5], uniform(-5.0, 2.0, s.next_f64()));
    }
}
