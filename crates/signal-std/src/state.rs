//! `std.phasor`, `std.counter` and `std.integrator`: the core set's state
//! types (ADR 0027 §1, "State (hides the loop)"). Each is a one-sample
//! recurrence that would otherwise be a feedback loop of several instances,
//! about 22 ns per node and frame (ADR 0027 §2). Direct, at rate divisor 1,
//! category `core`. Their outputs stay within the engine's magnitude bound
//! (`docs/semantics.md` §7), so they never fault on their own. A plan swap
//! that keeps the instance keeps its state (ADR 0006).

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, Range,
};

use crate::arith::fraction;
use crate::math::bounded;
use crate::sine_osc::FREQ_MAX;
use crate::{at, event_port, parameter, push_trigger, round_half_up, signal_port, triggered};

/// Where the integrator's gain comes from, by `mode` (ADR 0043: its value set).
const MODE_VALUES: &[signal_core::ValueLabel] =
    &[crate::named(0.0, "cutoff"), crate::named(1.0, "gain")];

const PHASOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.phasor",
    doc: "Phase accumulator: phase in [0, 1) advancing by freq/sr per frame (f64); a reset trigger sets it to reset_phase; a trigger on wrap.",
    ports: &[
        event_port("reset", Direction::In),
        signal_port(
            "phase",
            Direction::Out,
            PortKind::Phase,
            Some("cycles"),
            Some(Range { min: 0.0, max: 1.0 }),
        ),
        event_port("wrap", Direction::Out),
    ],
    parameters: &[
        parameter(
            "freq",
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            FREQ_MAX,
            1.0,
            Combine::Add,
        ),
        parameter(
            "reset_phase",
            PortKind::Phase,
            Some("cycles"),
            0.0,
            1.0,
            0.0,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.phasor`: a phase accumulator, the base for oscillators and LFOs.
///
/// **Ports**
/// - `reset` (event in, trigger): a frame with a `Trigger` sets the phase to
///   `reset_phase` before that frame's output; several on one frame count
///   once. `Set` and `Ramp` events are ignored.
/// - `phase` (signal out, phase, cycles, `[0, 1)`).
/// - `wrap` (event out, trigger): one `Trigger` on each frame whose phase came
///   from a sum outside `[0, 1)`; never on a reset frame.
///
/// **Parameters**
/// - `freq` (frequency, Hz, range `[0, FREQ_MAX]` as `std.osc`, default 1,
///   combine `add`).
/// - `reset_phase` (phase, cycles, range `[0, 1]`, default 0, combine `add`):
///   read on reset frames; 1 is the same phase as 0.
///
/// **Feedthrough:** direct (a reset on frame *n* sets `phase[n]`).
///
/// **Formula** (f64 accumulator `p`, 0 after `prepare`; sr the sample rate;
/// `fraction(x) = x − ⌊x⌋`, kept below 1):
///
/// ```text
/// reset on n:  p = fraction(reset_phase[n])
/// phase[n]   = p
/// s          = p + freq[n]/sr
/// p          = fraction(s)        wrap on n+1 if s ∉ [0, 1)
/// ```
///
/// The phase is in `[0, 1)` on every frame, at any frequency (several cycles
/// per frame give one wrap trigger). Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Phasor {
    period: f64,
    phase: f64,
    /// The last sum left `[0, 1)`: the next frame emits `wrap`.
    wrapped: bool,
}

impl DirectPrimitive for Phasor {
    fn describe() -> ModuleDescriptor {
        PHASOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.period = 1.0 / ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.wrapped = false;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (reset, freq, reset_phase) =
            (inputs.events(0), inputs.parameter(0), inputs.parameter(1));
        let mut next = 0;
        for n in 0..inputs.frames() {
            if triggered(reset, &mut next, n) {
                self.phase = fraction(at(reset_phase, n, 0.0));
            } else if self.wrapped {
                push_trigger(outputs, 0, n);
            }
            if let Some(out) = outputs.signal(0).get_mut(n) {
                *out = self.phase;
            }
            let sum = self.phase + at(freq, n, 0.0) * self.period;
            self.wrapped = !(0.0..1.0).contains(&sum);
            self.phase = fraction(sum);
        }
    }
}

/// The largest `modulo` and `step` magnitude of `std.counter`: the engine's
/// magnitude bound.
const COUNT_BOUND: f64 = crate::SIGNAL_BOUND;

const COUNTER: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.counter",
    doc: "Trigger counter: each trigger frame adds step to the count, wrapped modulo `modulo` (a trigger on wrap); a reset trigger sets it to 0.",
    ports: &[
        event_port("trigger", Direction::In),
        event_port("reset", Direction::In),
        signal_port(
            "count",
            Direction::Out,
            PortKind::Generic,
            None,
            Some(Range {
                min: 0.0,
                max: COUNT_BOUND - 1.0,
            }),
        ),
        event_port("wrap", Direction::Out),
    ],
    parameters: &[
        parameter(
            "step",
            PortKind::Generic,
            None,
            -crate::GENERIC,
            crate::GENERIC,
            1.0,
            Combine::Add,
        ),
        parameter(
            "modulo",
            PortKind::Generic,
            None,
            1.0,
            COUNT_BOUND,
            COUNT_BOUND,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.counter`: counts triggers, as an integer signal.
///
/// **Ports**
/// - `trigger` (event in, trigger): each frame with at least one `Trigger`
///   counts once.
/// - `reset` (event in, trigger): a frame with a `Trigger` sets the count to 0.
/// - `count` (signal out, generic, an integer in `[0, modulo)`).
/// - `wrap` (event out, trigger): on a trigger frame whose sum left
///   `[0, modulo)`.
///
/// `Set` and `Ramp` events are ignored.
///
/// **Parameters**
/// - `step` (generic, integer, range `[−10⁵, 10⁵]` (ADR 0043), default 1, combine
///   `add`): rounded with `round_half_up`; negative counts down.
/// - `modulo` (generic, integer, range `[1, 1e6]`, default 1e6, combine
///   `add`): rounded with `round_half_up`.
///
/// **Feedthrough:** direct (a trigger on frame *n* counts in `count[n]`).
///
/// **Formula** (integer `c`, 0 after `prepare`; `m = modulo[n]`, per frame):
///
/// ```text
/// reset on n:          c = 0             (that frame's trigger is ignored, no wrap)
/// else trigger on n:   s = c + step[n];  c = s mod m (floored);  wrap if s ∉ [0, m)
/// else:                c = c mod m       (a lowered modulo applies at once, no wrap)
/// count[n] = c
/// ```
///
/// **Bound:** the count is in `[0, modulo) ⊆ [0, 1e6)`, inside the engine's
/// magnitude bound, so the type never faults on its own. A larger count is
/// two counters: the second counts the first's `wrap`, so count =
/// second·modulo + first (as `std.frames`' `hi` and `lo`). Constant work per
/// frame.
#[derive(Clone, Debug, Default)]
pub struct Counter {
    count: i64,
}

impl DirectPrimitive for Counter {
    fn describe() -> ModuleDescriptor {
        COUNTER
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.reset();
    }

    fn reset(&mut self) {
        self.count = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (trigger, reset) = (inputs.events(0), inputs.events(1));
        let (step, modulo) = (inputs.parameter(0), inputs.parameter(1));
        let (mut next_trigger, mut next_reset) = (0, 0);
        for n in 0..inputs.frames() {
            let m = round_half_up(at(modulo, n, COUNT_BOUND)).clamp(1.0, COUNT_BOUND) as i64;
            let reset = triggered(reset, &mut next_reset, n);
            let counted = triggered(trigger, &mut next_trigger, n);
            if reset {
                self.count = 0;
            } else if counted {
                let step = round_half_up(at(step, n, 1.0)).clamp(-COUNT_BOUND, COUNT_BOUND) as i64;
                let sum = self.count + step;
                if !(0..m).contains(&sum) {
                    push_trigger(outputs, 0, n);
                }
                self.count = sum.rem_euclid(m);
            } else {
                self.count = self.count.rem_euclid(m);
            }
            if let Some(out) = outputs.signal(0).get_mut(n) {
                *out = self.count as f64;
            }
        }
    }
}

/// The highest cutoff of `std.integrator`, as a fraction of the sample rate:
/// below Nyquist, where `tan` diverges.
const CUTOFF_MAX_RATIO: f64 = 0.49;

const INTEGRATOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.integrator",
    doc: "TPT (trapezoidal) integrator: y = g*x + s, s = y + g*x, g = tan(pi*cutoff/sr) or gain/(2*sr) by mode; clamped to +-1e6; a reset trigger clears s.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Generic, None, None),
        event_port("reset", Direction::In),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[
        crate::choice(
            parameter(
                "mode",
                PortKind::Generic,
                None,
                0.0,
                1.0,
                0.0,
                Combine::Replace,
            ),
            MODE_VALUES,
        ),
        parameter(
            "cutoff",
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            100_000.0,
            100.0,
            Combine::Add,
        ),
        parameter(
            "gain",
            PortKind::Gain,
            Some("1/s"),
            0.0,
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

/// `std.integrator`: one topology-preserving-transform (trapezoidal)
/// integrator step, the building block of zero-delay-feedback filters.
///
/// **Ports**
/// - `in` (signal in, generic, unbounded); unconnected it reads 0.
/// - `reset` (event in, trigger): a frame with a `Trigger` clears the state
///   before that frame's step. `Set` and `Ramp` events are ignored.
/// - `out` (signal out, generic).
///
/// **Parameters**
/// - `mode` (generic, choice, range `[0, 1]`, default 0, combine `replace`):
///   `round_half_up(mode)`: 0 takes `g` from `cutoff`, 1 from `gain`.
/// - `cutoff` (frequency, Hz, range `[0, 100 000]`, default 100, combine
///   `add`): clamped to `0.49·sr`; `g = tan(π·cutoff/sr)`, the prewarped gain
///   of a one-pole at that cutoff.
/// - `gain` (gain, 1/s, range `[0, 10⁵]` (ADR 0043), default 1, combine `add`):
///   `g = gain/(2·sr)`, so `out` approximates `∫ gain·in dt` (the trapezoidal
///   rule).
///
/// **Feedthrough:** direct (`out[n]` depends on `in[n]`).
///
/// **Formula** (state `s`, 0 after `prepare`):
///
/// ```text
/// reset on n:  s = 0
/// v      = g[n]·in[n]
/// out[n] = clamp(v + s, ±1e6)
/// s      = clamp(out[n] + v, ±1e6)
/// ```
///
/// Clamped to `±1e6`, the engine's magnitude bound, so the type never faults
/// on its own; an integrator of a constant input saturates there. A pure
/// integrator does not decay, so nothing is flushed. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Integrator {
    sample_rate: f64,
    state: f64,
}

impl DirectPrimitive for Integrator {
    fn describe() -> ModuleDescriptor {
        INTEGRATOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.state = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (x, reset) = (inputs.signal(0), inputs.events(0));
        let (mode, cutoff, gain) = (
            inputs.parameter(0),
            inputs.parameter(1),
            inputs.parameter(2),
        );
        let sr = self.sample_rate;
        let mut next = 0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            if triggered(reset, &mut next, n) {
                self.state = 0.0;
            }
            let g = if sr <= 0.0 {
                0.0
            } else if crate::chosen(MODE_VALUES, signal_core::Snap::Nearest, at(mode, n, 0.0))
                >= 1.0
            {
                at(gain, n, 0.0) / (2.0 * sr)
            } else {
                let fc = at(cutoff, n, 0.0).clamp(0.0, CUTOFF_MAX_RATIO * sr);
                libm::tan(std::f64::consts::PI * fc / sr)
            };
            let v = g * at(x, n, 0.0);
            *out = bounded(v + self.state);
            self.state = bounded(*out + v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    const SR: f64 = 48_000.0;

    fn phasor(
        freq: f64,
        reset_phase: f64,
        resets: &[u64],
        frames: usize,
        block: usize,
    ) -> (Vec<f64>, Vec<u64>) {
        let mut p = Phasor::default();
        p.prepare(&context(SR, block, 0));
        let run = render(
            &mut p,
            &Timeline {
                signals: &[],
                parameters: &[&vec![freq; frames], &vec![reset_phase; frames]],
                triggers: &[resets],
            },
            frames,
            block,
        );
        (run.signals[0].clone(), run.events[0].clone())
    }

    /// AC1: the phase advances by freq/sr from 0, stays in [0, 1) and wraps
    /// with a trigger on the frame the wrapped phase is shown.
    #[test]
    fn phasor_accumulates_and_wraps() {
        let (phase, wraps) = phasor(12_000.0, 0.0, &[], 10, 3);
        assert_eq!(
            phase,
            [0.0, 0.25, 0.5, 0.75, 0.0, 0.25, 0.5, 0.75, 0.0, 0.25]
        );
        assert_eq!(wraps, [4, 8]);
        // Several cycles per frame: still in [0, 1), one wrap per frame.
        let (phase, wraps) = phasor(1e9, 0.0, &[], 64, 64);
        assert!(phase.iter().all(|p| (0.0..1.0).contains(p)));
        assert_eq!(wraps, (1..64).collect::<Vec<_>>());
    }

    /// AC1: a reset sets the phase to reset_phase on its own frame, emits no
    /// wrap, and reset_phase 1 is phase 0.
    #[test]
    fn phasor_reset_sets_the_phase() {
        let (phase, wraps) = phasor(12_000.0, 0.5, &[2, 7], 9, 4);
        assert_eq!(phase, [0.0, 0.25, 0.5, 0.75, 0.0, 0.25, 0.5, 0.5, 0.75]);
        assert_eq!(wraps, [4]);
        let (phase, _) = phasor(0.0, 1.0, &[0], 2, 1);
        assert_eq!(phase, [0.0, 0.0]);
    }

    fn counter(
        step: f64,
        modulo: &[f64],
        triggers: &[u64],
        resets: &[u64],
        block: usize,
    ) -> (Vec<f64>, Vec<u64>) {
        let frames = modulo.len();
        let mut c = Counter::default();
        c.prepare(&context(SR, block, 0));
        let run = render(
            &mut c,
            &Timeline {
                signals: &[],
                parameters: &[&vec![step; frames], modulo],
                triggers: &[triggers, resets],
            },
            frames,
            block,
        );
        (run.signals[0].clone(), run.events[0].clone())
    }

    /// AC1: triggers count by step, wrap modulo `modulo` with a trigger, and
    /// several triggers on one frame count once.
    #[test]
    fn counter_counts_and_wraps() {
        let (count, wraps) = counter(1.0, &[3.0; 8], &[1, 2, 2, 4, 6], &[], 3);
        assert_eq!(count, [0.0, 1.0, 2.0, 2.0, 0.0, 0.0, 1.0, 1.0]);
        assert_eq!(wraps, [4]);
        // A negative step counts down and wraps below 0.
        let (count, wraps) = counter(-1.0, &[4.0; 4], &[0, 1, 2], &[], 2);
        assert_eq!(count, [3.0, 2.0, 1.0, 1.0]);
        assert_eq!(wraps, [0]);
    }

    /// AC1: a reset wins over a same-frame trigger, and a lowered modulo
    /// applies at once without a wrap.
    #[test]
    fn counter_reset_wins_and_modulo_applies_at_once() {
        let (count, wraps) = counter(1.0, &[10.0; 5], &[0, 1, 2, 3], &[2], 5);
        assert_eq!(count, [1.0, 2.0, 0.0, 1.0, 1.0]);
        assert!(wraps.is_empty());
        let modulo = [10.0, 10.0, 10.0, 2.0];
        let (count, wraps) = counter(1.0, &modulo, &[0, 1, 2], &[], 1);
        assert_eq!(count, [1.0, 2.0, 3.0, 1.0]);
        assert!(wraps.is_empty());
    }

    /// AC10: the count stays below 1e6 with the largest step and modulo.
    #[test]
    fn counter_stays_inside_the_bound() {
        let triggers: Vec<u64> = (0..100).collect();
        let (count, wraps) = counter(1e6 - 1.0, &[1e6; 100], &triggers, &[], 7);
        assert!(
            count
                .iter()
                .all(|&c| (0.0..1e6).contains(&c) && c.fract() == 0.0)
        );
        assert_eq!(wraps.len(), 99);
    }

    fn integrate(
        x: &[f64],
        mode: f64,
        cutoff: f64,
        gain: f64,
        resets: &[u64],
        block: usize,
    ) -> Vec<f64> {
        let frames = x.len();
        let mut i = Integrator::default();
        i.prepare(&context(SR, block, 0));
        render(
            &mut i,
            &Timeline {
                signals: &[x],
                parameters: &[
                    &vec![mode; frames],
                    &vec![cutoff; frames],
                    &vec![gain; frames],
                ],
                triggers: &[resets],
            },
            frames,
            block,
        )
        .signals[0]
            .clone()
    }

    /// AC1: in gain mode a constant input integrates to the trapezoidal sum
    /// g·x·(2n + 1), the reset clears the state, and in cutoff mode
    /// g = tan(π·fc/sr).
    #[test]
    fn integrator_is_a_trapezoidal_step() {
        let g = 4800.0 / (2.0 * SR);
        let y = integrate(&[1.0; 6], 1.0, 0.0, 4800.0, &[3], 4);
        let expected = [g, 3.0 * g, 5.0 * g, g, 3.0 * g, 5.0 * g];
        for (a, b) in y.iter().zip(expected) {
            assert!((a - b).abs() < 1e-15, "{a} vs {b}");
        }
        let g = libm::tan(std::f64::consts::PI * 1000.0 / SR);
        let y = integrate(&[1.0, 0.0, 0.0], 0.0, 1000.0, 0.0, &[], 1);
        assert_eq!(y, [g, 2.0 * g, 2.0 * g]);
    }

    /// AC10: a constant input at the bound saturates at 1e6, and so does the
    /// state.
    #[test]
    fn integrator_is_clamped() {
        let y = integrate(&[1e6; 50], 1.0, 0.0, 1e6, &[], 8);
        assert!(y.iter().all(|v| v.abs() <= 1e6));
        assert_eq!(*y.last().unwrap(), 1e6);
        let y = integrate(&[-1e6; 50], 0.0, 1e5, 0.0, &[], 8);
        assert_eq!(*y.last().unwrap(), -1e6);
    }
}
