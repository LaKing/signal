//! `std.const`, `std.sample_rate` and `std.frames`: the core set's constants
//! and rate (ADR 0027 §1). Direct, at rate divisor 1, category `core`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::{at, parameter, signal_port};

/// The bound of `std.const`'s `value`: the engine's magnitude bound
/// (`docs/semantics.md` §7), as for `std.scale` and `std.clamp`. A larger
/// constant would only fault and mute the instance on its first frame; every
/// integer up to it is exact, and it holds constants such as 2¹⁹ or 10⁶.
const VALUE_BOUND: f64 = crate::GENERIC;

const CONST: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.const",
    doc: "A constant: out = value (a parameter, so it can be set, ramped and modulated).",
    ports: &[signal_port(
        "out",
        Direction::Out,
        PortKind::Generic,
        None,
        None,
    )],
    parameters: &[parameter(
        "value",
        PortKind::Generic,
        None,
        -VALUE_BOUND,
        VALUE_BOUND,
        0.0,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.const`: a constant signal.
///
/// **Ports**
/// - `out` (signal out, generic).
///
/// **Parameters**
/// - `value` (generic, no unit, range `[−10⁵, 10⁵]` (ADR 0043; a larger constant through a connection's scale), the engine's magnitude
///   bound, default 0, combine `add`).
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = value[n]`, the parameter's effective value (its base,
/// with any ramp and modulation, clamped to its range; ADR 0016 §4).
/// Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Const;

impl DirectPrimitive for Const {
    fn describe() -> ModuleDescriptor {
        CONST
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let value = inputs.parameter(0);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = at(value, n, 0.0);
        }
    }
}

const SAMPLE_RATE: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.sample_rate",
    doc: "The master rate: sr in Hz and period = 1/sr in seconds, from prepare.",
    ports: &[
        signal_port("sr", Direction::Out, PortKind::Frequency, Some("Hz"), None),
        signal_port("period", Direction::Out, PortKind::Generic, Some("s"), None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.sample_rate`: the master rate as signals.
///
/// **Ports**
/// - `sr` (signal out, frequency, Hz): the master rate.
/// - `period` (signal out, generic, s): the duration of one frame.
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `sr[n] = rate`, `period[n] = 1 / rate`, where `rate` is the
/// master rate `prepare` was given (`PrepareContext::sample_rate`). Both are
/// set in `prepare`, which `reset` leaves them at. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct SampleRate {
    sr: f64,
    period: f64,
}

impl DirectPrimitive for SampleRate {
    fn describe() -> ModuleDescriptor {
        SAMPLE_RATE
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sr = ctx.sample_rate;
        self.period = 1.0 / ctx.sample_rate;
    }

    fn reset(&mut self) {}

    fn process(&mut self, _inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(self.sr);
        outputs.signal(1).fill(self.period);
    }
}

const FRAMES: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.frames",
    doc: "The frames this instance has run before the current one, count = hi*1e6 + lo, exact to 1e12; kept across live swaps.",
    ports: &[
        signal_port("lo", Direction::Out, PortKind::Generic, None, None),
        signal_port("hi", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.frames`: the instance's own running time in frames.
///
/// **Ports**
/// - `lo` (signal out, generic): the count modulo 10⁶, in `[0, 10⁶)`.
/// - `hi` (signal out, generic): the count divided by 10⁶, rounded down, in
///   `[0, 10⁶]`.
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** the count *c*[*n*] is the number of frames the instance ran
/// before frame *n*: 0 on its first frame, then one more per frame it runs.
/// `lo[n] = c mod 10⁶` and `hi[n] = ⌊c / 10⁶⌋`, so `c = hi·10⁶ + lo`. The
/// count is kept as an integer (`u64`), and both outputs are exact integers.
/// Two outputs because the engine faults a signal output beyond 10⁶ in
/// magnitude (`docs/semantics.md` §7): a single count would mute the instance
/// after 10⁶ frames (about 21 s at 48 kHz). The count is exact up to 10¹²
/// frames (about 241 days at 48 kHz, 60 days at 192 kHz), where it stops:
/// from then on `lo = 0` and `hi = 10⁶`, never beyond the bound. It counts
/// frames, not calls, so it is the same for every partition into calls.
/// While suspended an instance is not called, so suspended frames do not
/// count (`docs/semantics.md` §6). A plan swap that keeps the instance keeps
/// its count (ADR 0006, `docs/semantics.md` §12); an instance a swap adds
/// starts at 0. `prepare` and `reset` set it to 0. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Frames {
    count: u64,
}

/// `std.frames`'s split: `lo` and `hi` are the count modulo and divided by
/// this.
const FRAMES_SPLIT: u64 = 1_000_000;

/// The largest count `std.frames` reaches: `hi` is then 10⁶, the bound.
const FRAMES_TOP: u64 = FRAMES_SPLIT * FRAMES_SPLIT;

#[cfg(test)]
impl Frames {
    /// An instance that has already run `count` frames.
    fn having_run(count: u64) -> Frames {
        Frames { count }
    }
}

impl DirectPrimitive for Frames {
    fn describe() -> ModuleDescriptor {
        FRAMES
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.count = 0;
    }

    fn reset(&mut self) {
        self.count = 0;
    }

    fn process(&mut self, _inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        // The count at frame n of the call; it stops at FRAMES_TOP. Both
        // outputs are below 2⁵³, so exact as f64.
        let start = self.count;
        let count = |n: usize| start.saturating_add(n as u64).min(FRAMES_TOP);
        for (n, lo) in outputs.signal(0).iter_mut().enumerate() {
            *lo = (count(n) % FRAMES_SPLIT) as f64;
        }
        for (n, hi) in outputs.signal(1).iter_mut().enumerate() {
            *hi = (count(n) / FRAMES_SPLIT) as f64;
        }
        self.count = count(outputs.frames());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    #[test]
    fn const_outputs_its_value() {
        let value = [0.0, 524_288.0, 1e6, -1e6, 0.25, -3.5];
        let mut c = Const;
        c.prepare(&context(48_000.0, 4, 0));
        let timeline = Timeline {
            parameters: &[&value],
            ..Timeline::default()
        };
        assert_eq!(render(&mut c, &timeline, 6, 4).signals[0], value);
    }

    /// The range is the engine's magnitude bound, ±1e6 (`docs/semantics.md`
    /// §7): every value in it renders without a fault, and a larger one could
    /// only fault.
    #[test]
    fn const_range_is_the_generic_range() {
        // ADR 0043: ±10⁵, well inside the engine's magnitude bound.
        let range = CONST.parameters[0].range;
        assert_eq!((range.min, range.max), (-1e5, 1e5));
        assert_eq!(range.max, crate::GENERIC);
        assert!(range.max < crate::SIGNAL_BOUND);
    }

    /// AC6: sr and 1/sr from prepare, at every rate, and kept by reset.
    #[test]
    fn sample_rate_and_period() {
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut s = SampleRate::default();
            s.prepare(&context(rate, 4, 0));
            s.reset();
            let run = render(&mut s, &Timeline::default(), 5, 4);
            assert_eq!(run.signals[0], [rate; 5]);
            assert_eq!(run.signals[1], [1.0 / rate; 5]);
        }
    }

    /// The count as `c = hi·1e6 + lo`.
    fn counts(run: &crate::testing::Run) -> Vec<f64> {
        run.signals[0]
            .iter()
            .zip(&run.signals[1])
            .map(|(lo, hi)| hi * 1e6 + lo)
            .collect()
    }

    /// AC6: the count of frames run, independent of the call sizes; reset
    /// and prepare start it again.
    #[test]
    fn frames_counts_frames_in_any_partition() {
        let expected: Vec<f64> = (0..300).map(f64::from).collect();
        for block in [1, 7, 64, 256] {
            let mut f = Frames::default();
            f.prepare(&context(48_000.0, block, 0));
            let run = render(&mut f, &Timeline::default(), 300, block);
            assert_eq!(run.signals[0], expected, "calls of {block}");
            assert_eq!(run.signals[1], [0.0; 300], "calls of {block}");
            f.reset();
            assert_eq!(
                render(&mut f, &Timeline::default(), 2, block).signals[0],
                [0.0, 1.0]
            );
            f.prepare(&context(96_000.0, block, 0));
            assert_eq!(
                render(&mut f, &Timeline::default(), 1, block).signals[0],
                [0.0]
            );
        }
    }

    /// AC6: lo wraps at 1e6 and hi carries, exactly; both stay within the
    /// bound.
    #[test]
    fn frames_splits_at_a_million() {
        let mut f = Frames::having_run(999_998);
        let run = render(&mut f, &Timeline::default(), 4, 3);
        assert_eq!(run.signals[0], [999_998.0, 999_999.0, 0.0, 1.0]);
        assert_eq!(run.signals[1], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(counts(&run), [999_998.0, 999_999.0, 1e6, 1e6 + 1.0]);
        let mut f = Frames::having_run(123_456_789_012);
        let run = render(&mut f, &Timeline::default(), 1, 1);
        assert_eq!(
            (run.signals[0][0], run.signals[1][0]),
            (789_012.0, 123_456.0)
        );
    }

    /// AC6: exact to 1e12 frames, where the count stops at lo = 0, hi = 1e6.
    #[test]
    fn frames_is_exact_to_ten_to_the_twelve_and_stops_there() {
        let mut f = Frames::having_run(FRAMES_TOP - 2);
        let run = render(&mut f, &Timeline::default(), 4, 3);
        assert_eq!(run.signals[0], [999_998.0, 999_999.0, 0.0, 0.0]);
        assert_eq!(run.signals[1], [999_999.0, 999_999.0, 1e6, 1e6]);
        assert_eq!(counts(&run)[1], 999_999_999_999.0);
        assert!(
            run.signals
                .iter()
                .flatten()
                .all(|v| v.abs() <= crate::SIGNAL_BOUND)
        );
    }
}
