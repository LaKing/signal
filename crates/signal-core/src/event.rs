//! Events: small generic messages on event ports (ADR 0004, ADR 0016 §3, §4a).

use serde::Serialize;

/// An event inside one `process`/`absorb` or `emit` call: a payload stamped
/// with its frame offset in the call's block (0 ≤ `offset` < frames of the call).
///
/// `Copy` and heap-free so it can move through bounded buffers on the audio
/// thread; its size is at most 32 bytes (asserted by a test).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    /// Frame offset inside the current block.
    pub offset: u32,
    /// What the event says.
    pub payload: EventPayload,
}

/// The content of an [`Event`].
///
/// `Set` and `Ramp` sent to a parameter change its base value, applied by the
/// executor exactly as ADR 0016 §4a defines; modules never apply them.
///
/// Serializes as `"trigger"`, `{"set": v}` or
/// `{"ramp": {"to": v, "frames": n, "curve": "linear"}}` (render logs).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventPayload {
    /// A momentary impulse.
    Trigger,
    /// Set a value at once.
    Set(f64),
    /// Move from the current value to `to` over `frames` frames along `curve`;
    /// `frames == 0` is the same as `Set(to)`.
    Ramp {
        /// Target value, reached exactly.
        to: f64,
        /// Duration in master-clock frames the receiver runs.
        frames: u64,
        /// Shape of the movement.
        curve: Curve,
    },
}

/// The shape of a ramp: a function `c` on `[0, 1]` with `c(0) = 0` and
/// `c(1) = 1` exactly (ADR 0016 §4a). Every future curve keeps this constraint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Curve {
    /// `c(x) = x`.
    #[default]
    Linear,
}

impl Curve {
    /// `c(x)` for `x` in `[0, 1]`.
    pub fn eval(self, x: f64) -> f64 {
        match self {
            Curve::Linear => x,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_fits_32_bytes() {
        assert!(
            size_of::<Event>() <= 32,
            "Event is {} bytes",
            size_of::<Event>()
        );
    }

    #[test]
    fn linear_curve_endpoints_are_exact() {
        assert_eq!(Curve::Linear.eval(0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(Curve::Linear.eval(1.0).to_bits(), 1.0f64.to_bits());
        assert_eq!(Curve::Linear.eval(0.25), 0.25);
    }

    #[test]
    fn events_are_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Event>();
        let e = Event {
            offset: 3,
            payload: EventPayload::Ramp {
                to: 1.0,
                frames: 96,
                curve: Curve::Linear,
            },
        };
        let f = e;
        assert_eq!(e, f);
    }
}
