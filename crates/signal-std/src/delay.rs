//! `std.delay`.

use signal_core::{
    Combine, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind, PrepareContext,
    StateOutputPrimitive,
};

use crate::{at, parameter, signal_port};

const TIME_MS: f64 = 250.0;
/// The longest delay, in seconds.
const MAX_SECONDS: f64 = 10.0;
/// Frames the buffer holds beyond the longest delay, for the interpolator.
const GUARD: usize = 4;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.delay",
    doc: "Fractional delay line of up to 10 s with 4-point, 3rd-order Hermite interpolation; at least 1 frame, so it may close a feedback loop.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[parameter(
        "time_ms",
        PortKind::Control,
        Some("ms"),
        0.0,
        MAX_SECONDS * 1000.0,
        TIME_MS,
        Combine::Add,
    )],
    feedthrough: Feedthrough::StateOutput,
    sink: None,
};

/// `std.delay`: a fractional delay line.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `out` (signal out, audio).
///
/// **Parameters**
/// - `time_ms` (control, ms, range `[0, 10 000]`, default 250, combine `add`):
///   read in `absorb`, so the value of frame *n* sets the delay of `out[n+1]`
///   (a state-output primitive's `emit` reads no parameters, ADR 0016 §5).
///   Clamped in `absorb` to `[1, ⌈10·sr⌉]` frames.
///
/// **Feedthrough:** state-output: `out[n]` depends on `in[n−1]` and earlier
/// only, so a delay may close a feedback loop without a `std.unit_delay`
/// (ADR 0016 §2).
///
/// **Formula** (x = `in`, sr the sample rate, x\[j\] = 0 for j < 0):
///
/// ```text
/// d      = clamp(time_ms[n−1]·sr/1000, 1, ⌈10·sr⌉)        frames
/// p      = n − d,   i = ⌊p⌋,   t = p − i ∈ [0, 1)
/// y₋₁, y₀, y₁, y₂ = x[i−1], x[i], x[i+1], x[i+2]
/// c₁     = ½·(y₁ − y₋₁)
/// c₂     = y₋₁ − 2.5·y₀ + 2·y₁ − ½·y₂
/// c₃     = ½·(y₂ − y₋₁) + 1.5·(y₀ − y₁)
/// out[n] = ((c₃·t + c₂)·t + c₁)·t + y₀
/// ```
///
/// This is the 4-point, 3rd-order Hermite (Catmull-Rom) interpolator. Its
/// weights on `y₋₁ … y₂` are `(−t³ + 2t² − t)/2`, `(3t³ − 5t² + 2)/2`,
/// `(−3t³ + 4t² + t)/2` and `(t³ − t²)/2`; they sum to 1, so a constant input
/// passes exactly, and at an integer `d` (t = 0) the output is exactly
/// `x[n − d]`. `out[0] = 0`.
///
/// **Exception: delays between 1 and 2 frames** (operator decision, M2-7).
/// The formula above is exact 4-point Hermite for `d ≥ 2`. For `1 < d < 2`
/// its `y₂ = x[n]` is the current input, which a state-output primitive
/// cannot read when `out[n]` is emitted, so this causal fallback replaces it
/// with `x[n−1]`, the newest sample; every other point is the true sample.
/// At `d = 1.5` an impulse therefore comes out as `0.5, 0.5625, −0.0625`
/// instead of Hermite's `−0.0625, 0.5625, 0.5625, −0.0625`. The fallback is
/// exact at `d = 1` (`out[n] = x[n−1]`) and continuous at `d = 1` and `d = 2`,
/// where the replaced point's weight goes to 0.
///
/// **Buffer.** `prepare` allocates a circular buffer of `⌈10·sr⌉ + 4` frames,
/// zeroed; nothing else allocates. `reset` zeroes it again, which costs one
/// pass over the buffer. `absorb` and `emit` do constant work per frame. The
/// output is a weighted sum of input samples, so it needs no denormal flush
/// (`docs/dsp-conventions.md`, "Denormals").
#[derive(Clone, Debug, Default)]
pub struct Delay {
    sample_rate: f64,
    /// The input history; `buffer[newest]` is the latest absorbed sample.
    buffer: Vec<f64>,
    newest: usize,
    /// `⌈10·sr⌉`, the longest delay in frames.
    max_delay: f64,
    /// The output of the next frame, computed in `absorb`.
    next: f64,
}

impl Delay {
    /// The sample `back` frames before the newest one, `back` < the buffer length.
    fn sample(&self, back: usize) -> f64 {
        let len = self.buffer.len();
        let back = back.min(len.saturating_sub(1));
        let index = (self.newest + len - back) % len.max(1);
        self.buffer.get(index).copied().unwrap_or(0.0)
    }

    /// The input `back` frames before the newest sample, `0 ≤ back ≤ ⌈10·sr⌉ − 1`,
    /// interpolated as the formula says.
    fn read(&self, back: f64) -> f64 {
        let whole = libm::floor(back);
        let frac = back - whole;
        // `y₀` is `offset` frames before the newest sample.
        let (offset, t) = if frac == 0.0 {
            (whole as usize, 0.0)
        } else {
            (whole as usize + 1, 1.0 - frac)
        };
        let ym1 = self.sample(offset + 1);
        let y0 = self.sample(offset);
        // Points newer than the newest sample are the newest sample.
        let y1 = self.sample(offset.saturating_sub(1));
        let y2 = self.sample(offset.saturating_sub(2));
        let c1 = 0.5 * (y1 - ym1);
        let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
        ((c3 * t + c2) * t + c1) * t + y0
    }
}

impl StateOutputPrimitive for Delay {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.max_delay = libm::ceil(MAX_SECONDS * ctx.sample_rate).max(1.0);
        self.buffer = vec![0.0; self.max_delay as usize + GUARD];
        self.newest = 0;
        self.next = 0.0;
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.newest = 0;
        self.next = 0.0;
    }

    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(self.next);
    }

    fn absorb(&mut self, inputs: &Inputs<'_>) {
        let x = inputs.signal(0);
        let time_ms = inputs.parameter(0);
        let len = self.buffer.len().max(1);
        for n in 0..inputs.frames() {
            self.newest = (self.newest + 1) % len;
            if let Some(slot) = self.buffer.get_mut(self.newest) {
                *slot = at(x, n, 0.0);
            }
            // `max` before `min`: a NaN time reads as 1 frame.
            let d = (at(time_ms, n, TIME_MS) * self.sample_rate / 1000.0)
                .max(1.0)
                .min(self.max_delay);
            self.next = self.read(d - 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::context;

    /// Runs a delay per frame, as the engine does, at `sample_rate`.
    fn delay(input: &[f64], time_ms: f64, sample_rate: f64) -> Vec<f64> {
        let mut delay = Delay::default();
        delay.prepare(&context(sample_rate, 1, 0));
        input
            .iter()
            .map(|&x| {
                let mut out = [f64::NAN];
                delay.emit(&mut Outputs::new(1, &mut out, &mut []));
                delay.absorb(&Inputs::new(1, &[x], &[time_ms], &[], &[]));
                out[0]
            })
            .collect()
    }

    fn impulse(frames: usize) -> Vec<f64> {
        let mut x = vec![0.0; frames];
        x[0] = 1.0;
        x
    }

    /// The Hermite weights on `y₋₁, y₀, y₁, y₂` at `t`, written out independently.
    fn weights(t: f64) -> [f64; 4] {
        let (t2, t3) = (t * t, t * t * t);
        [
            (-t3 + 2.0 * t2 - t) / 2.0,
            (3.0 * t3 - 5.0 * t2 + 2.0) / 2.0,
            (-3.0 * t3 + 4.0 * t2 + t) / 2.0,
            (t3 - t2) / 2.0,
        ]
    }

    /// AC2a: at an integer delay an impulse comes out exactly `d` frames later,
    /// alone; 0 ms is clamped to 1 frame.
    #[test]
    fn integer_delays_shift_an_impulse_exactly() {
        // At 1000 Hz, d = time_ms frames.
        for (time_ms, d) in [(5.0, 5), (1.0, 1), (0.0, 1), (0.25, 1), (37.0, 37)] {
            let out = delay(&impulse(64), time_ms, 1000.0);
            for (n, &y) in out.iter().enumerate() {
                let expected: f64 = if n == d { 1.0 } else { 0.0 };
                assert_eq!(y.to_bits(), expected.to_bits(), "{time_ms} ms, frame {n}");
            }
        }
        // 250 ms at 48 kHz is 12 000 frames.
        let out = delay(&impulse(12_100), 250.0, 48_000.0);
        assert_eq!(out[12_000], 1.0);
        assert_eq!(out.iter().filter(|&&y| y != 0.0).count(), 1);
    }

    /// AC2a: at a fractional delay d ≥ 2 an impulse on frame 0 comes out as the
    /// four Hermite weights, on frames ⌊d⌋ − 1 … ⌊d⌋ + 2.
    #[test]
    fn fractional_delays_spread_an_impulse_over_the_hermite_weights() {
        for d in [2.25, 2.5, 7.625, 100.875] {
            let out = delay(&impulse(128), d, 1000.0);
            let t = 1.0 - (d - libm::floor(d));
            let w = weights(t);
            // out[n] reads p = n − d; frame 0 is y₂ when i + 2 = 0, …, y₋₁ when i − 1 = 0.
            let first = libm::floor(d) as usize - 1;
            for (n, &y) in out.iter().enumerate() {
                let expected = match n.checked_sub(first) {
                    Some(j @ 0..=3) => w[3 - j],
                    _ => 0.0,
                };
                approx::assert_abs_diff_eq!(y, expected, epsilon = 1e-15);
            }
            approx::assert_abs_diff_eq!(out.iter().sum::<f64>(), 1.0, epsilon = 1e-14);
        }
        // d = 2: exactly the impulse two frames later.
        let out = delay(&impulse(8), 2.0, 1000.0);
        assert_eq!(out, [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    /// The accepted exception for 1 < d < 2 (operator decision, M2-7): the
    /// unknown y₂ = x[n] is replaced by x[n−1]. Pins the fallback values, its
    /// exactness at d = 1 and its continuity at d = 1 and d = 2.
    #[test]
    fn delays_between_one_and_two_frames_use_the_causal_fallback() {
        // d = 1.5: t = 0.5, Hermite weights −0.0625, 0.5625, 0.5625, −0.0625;
        // the fallback folds y₂'s −0.0625 into y₁: 0.5, 0.5625, −0.0625.
        let out = delay(&impulse(8), 1.5, 1000.0);
        assert_eq!(out, [0.0, 0.5, 0.5625, -0.0625, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(weights(0.5), [-0.0625, 0.5625, 0.5625, -0.0625]);
        // d = 1: exactly out[n] = x[n−1].
        assert_eq!(delay(&impulse(8), 1.0, 1000.0)[..3], [0.0, 1.0, 0.0]);

        // 0.0390625 ms at 48 kHz is 1.875 frames: t = 0.125.
        let out = delay(&impulse(8), 0.0390625, 48_000.0);
        let w = weights(0.125);
        approx::assert_abs_diff_eq!(out[1], w[2] + w[3], epsilon = 1e-15);
        approx::assert_abs_diff_eq!(out[2], w[1], epsilon = 1e-15);
        approx::assert_abs_diff_eq!(out[3], w[0], epsilon = 1e-15);
        assert_eq!(out[0], 0.0);
        assert!(out[4..].iter().all(|&y| y == 0.0));

        // Continuity: just inside (1, 2) the output is within O(ε) of d = 1
        // and d = 2 on an arbitrary signal.
        let x: Vec<f64> = (0..200)
            .map(|n| libm::sin(n as f64 * 0.37) + 0.3 * libm::cos(n as f64 * 1.9))
            .collect();
        let eps = libm::ldexp(1.0, -20);
        for (inside, edge) in [(2.0 - eps, 2.0), (1.0 + eps, 1.0)] {
            let (a, b) = (delay(&x, inside, 1000.0), delay(&x, edge, 1000.0));
            for n in 0..x.len() {
                approx::assert_abs_diff_eq!(a[n], b[n], epsilon = 1e-5);
            }
        }
    }

    #[test]
    fn a_constant_passes_exactly_at_any_delay() {
        for time_ms in [0.0, 1.5, 3.3, 250.0] {
            let out = delay(&[0.7; 400], time_ms, 1000.0);
            let d = time_ms.max(1.0);
            let settled = libm::ceil(d) as usize + 2;
            assert!(out[settled..].iter().all(|&y| y == 0.7), "{time_ms} ms");
        }
    }

    /// The longest delay is 10 s, and the time is clamped to it.
    #[test]
    fn the_buffer_holds_ten_seconds() {
        let mut line = Delay::default();
        line.prepare(&context(44_100.0, 1, 0));
        assert_eq!(line.buffer.len(), 441_000 + 4);
        let out = delay(&impulse(10_010), 10_000.0, 1000.0);
        assert_eq!(out[10_000], 1.0);
        assert_eq!(out.iter().filter(|&&y| y != 0.0).count(), 1);
    }

    /// The time of frame n sets the delay of frame n + 1.
    #[test]
    fn the_time_is_read_in_absorb() {
        let mut delay = Delay::default();
        delay.prepare(&context(1000.0, 1, 0));
        let mut outputs = Vec::new();
        for (n, time_ms) in [3.0, 3.0, 3.0, 1.0, 1.0].into_iter().enumerate() {
            let mut out = [f64::NAN];
            delay.emit(&mut Outputs::new(1, &mut out, &mut []));
            delay.absorb(&Inputs::new(1, &[n as f64 + 1.0], &[time_ms], &[], &[]));
            outputs.push(out[0]);
        }
        // out[3] = x[0] (d = 3, from frame 2); out[4] = x[3] (d = 1, from frame 3).
        assert_eq!(outputs, [0.0, 0.0, 0.0, 1.0, 4.0]);
    }
}
