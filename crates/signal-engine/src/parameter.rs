//! A parameter's base value frame by frame, and its effective value (ADR 0016
//! §1a, §4, §4a). Events change only the base; modulation is combined in
//! [`effective`].

use signal_core::{Combine, Curve, EventPayload, Range};

/// A running ramp: `b`, `to`, `frames` (*N* ≥ 1), the curve, and `i`, the frames
/// the instance has run since the ramp started.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ramp {
    b: f64,
    to: f64,
    frames: u64,
    curve: Curve,
    i: u64,
}

/// A ramp received while the instance was suspended, started at resume.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    to: f64,
    frames: u64,
    curve: Curve,
}

/// Whether an event acted on the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    /// `set` or `ramp`: applied (or held pending).
    Yes,
    /// A trigger: no meaning for a parameter.
    NotAParameterEvent,
}

/// The base value of one parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Base {
    value: f64,
    ramp: Option<Ramp>,
    pending: Option<Pending>,
}

impl Base {
    /// A base at `value` with no ramp.
    pub(crate) fn new(value: f64) -> Base {
        Base {
            value,
            ramp: None,
            pending: None,
        }
    }

    /// The current base value.
    pub(crate) fn value(&self) -> f64 {
        self.value
    }

    /// §4a step 1, on a frame the instance runs: a running ramp moves to its
    /// value `b + (to − b)·c(i/N)`, or to exactly `to` once `i ≥ N`. Then a ramp
    /// held while the instance was suspended starts, before the frame's events
    /// (§1a).
    pub(crate) fn advance(&mut self) {
        if let Some(mut ramp) = self.ramp {
            ramp.i = ramp.i.saturating_add(1);
            if ramp.i >= ramp.frames {
                self.value = ramp.to;
                self.ramp = None;
            } else {
                let x = ramp.i as f64 / ramp.frames as f64;
                self.value = ramp.b + (ramp.to - ramp.b) * ramp.curve.eval(x);
                self.ramp = Some(ramp);
            }
        }
        if let Some(pending) = self.pending.take() {
            self.start(pending.to, pending.frames, pending.curve);
        }
    }

    /// §4a step 2: applies one event of a frame the instance runs.
    pub(crate) fn apply(&mut self, payload: EventPayload) -> Applied {
        match payload {
            EventPayload::Set(value)
            | EventPayload::Ramp {
                to: value,
                frames: 0,
                ..
            } => {
                self.value = value;
                self.ramp = None;
            }
            EventPayload::Ramp { to, frames, curve } => self.start(to, frames, curve),
            EventPayload::Trigger => return Applied::NotAParameterEvent,
        }
        Applied::Yes
    }

    /// §1a: applies an event that arrives while the instance is suspended. A
    /// `set` takes effect at once and cancels any ramp, running or pending; a
    /// `ramp` is held pending (the latest only) and starts at resume.
    pub(crate) fn apply_suspended(&mut self, payload: EventPayload) -> Applied {
        match payload {
            EventPayload::Set(value)
            | EventPayload::Ramp {
                to: value,
                frames: 0,
                ..
            } => {
                self.value = value;
                self.ramp = None;
                self.pending = None;
            }
            EventPayload::Ramp { to, frames, curve } => {
                self.pending = Some(Pending { to, frames, curve });
            }
            EventPayload::Trigger => return Applied::NotAParameterEvent,
        }
        Applied::Yes
    }

    /// Starts a ramp of `frames` ≥ 1 from the current value, which stays the
    /// base for this frame.
    fn start(&mut self, to: f64, frames: u64, curve: Curve) {
        self.ramp = Some(Ramp {
            b: self.value,
            to,
            frames,
            curve,
            i: 0,
        });
    }
}

/// The effective value (§4): `base` combined with the modulation sum, or `base`
/// alone when the parameter has no signal sources (`modulation` is `None`), then
/// clamped to `range` in every case. Never panics, unlike `f64::clamp`.
pub(crate) fn effective(base: f64, modulation: Option<f64>, combine: Combine, range: Range) -> f64 {
    let value = match modulation {
        None => base,
        Some(m) => match combine {
            Combine::Add => base + m,
            Combine::Multiply => base * m,
            Combine::Replace => m,
        },
    };
    if value < range.min {
        range.min
    } else if value > range.max {
        range.max
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(to: f64, frames: u64) -> EventPayload {
        EventPayload::Ramp {
            to,
            frames,
            curve: Curve::Linear,
        }
    }

    #[test]
    fn ramp_follows_the_closed_form_and_ends_exactly() {
        let mut base = Base::new(0.25);
        assert_eq!(base.apply(ramp(1.0, 7)), Applied::Yes);
        assert_eq!(base.value(), 0.25);
        for i in 1..7u64 {
            base.advance();
            let expected = 0.25 + (1.0 - 0.25) * (i as f64 / 7.0);
            assert_eq!(base.value().to_bits(), expected.to_bits(), "i = {i}");
        }
        base.advance();
        assert_eq!(base.value().to_bits(), 1.0f64.to_bits());
        base.advance();
        assert_eq!(base.value(), 1.0);
    }

    #[test]
    fn zero_frame_ramp_is_a_set_and_set_cancels_a_ramp() {
        let mut base = Base::new(0.0);
        base.apply(ramp(0.5, 0));
        assert_eq!(base.value(), 0.5);
        base.apply(ramp(1.0, 4));
        base.advance();
        base.apply(EventPayload::Set(-2.0));
        base.advance();
        assert_eq!(base.value(), -2.0);
    }

    #[test]
    fn interrupting_ramp_starts_from_the_interrupted_value() {
        let mut base = Base::new(0.0);
        base.apply(ramp(1.0, 4));
        base.advance();
        base.advance(); // 0.5
        base.apply(ramp(0.0, 2));
        assert_eq!(base.value(), 0.5);
        base.advance();
        assert_eq!(base.value(), 0.25);
        base.advance();
        assert_eq!(base.value(), 0.0);
    }

    #[test]
    fn suspended_ramp_waits_and_set_clears_it() {
        let mut base = Base::new(0.0);
        assert_eq!(base.apply_suspended(ramp(1.0, 2)), Applied::Yes);
        assert_eq!(base.value(), 0.0);
        base.advance(); // resume: the pending ramp starts from 0
        assert_eq!(base.value(), 0.0);
        base.advance();
        assert_eq!(base.value(), 0.5);

        let mut base = Base::new(0.0);
        base.apply_suspended(ramp(1.0, 2));
        base.apply_suspended(EventPayload::Set(0.75));
        base.advance();
        base.advance();
        assert_eq!(base.value(), 0.75);
        assert_eq!(
            base.apply_suspended(EventPayload::Trigger),
            Applied::NotAParameterEvent
        );
    }

    #[test]
    fn effective_combines_then_clamps_and_never_clamps_the_base() {
        let range = Range { min: 0.0, max: 1.0 };
        assert_eq!(effective(0.5, Some(0.25), Combine::Add, range), 0.75);
        assert_eq!(effective(0.5, Some(0.5), Combine::Multiply, range), 0.25);
        assert_eq!(effective(0.5, Some(0.125), Combine::Replace, range), 0.125);
        assert_eq!(effective(0.75, Some(0.75), Combine::Add, range), 1.0);
        // No signal sources: clamp(base), whatever `combine` says.
        for combine in [Combine::Add, Combine::Multiply, Combine::Replace] {
            assert_eq!(effective(7.0, None, combine, range), 1.0);
            assert_eq!(effective(-3.0, None, combine, range), 0.0);
            assert_eq!(effective(0.5, None, combine, range), 0.5);
        }
    }
}
