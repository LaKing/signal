//! `std.exciter`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, RngStream,
};

use crate::{at, event_port, parameter, round_half_up, signal_port, triggered};

/// The default `kind`: strike.
const STRIKE: f64 = 1.0;
const LEVEL: f64 = 1.0;
const DURATION_MS: f64 = 5.0;
const HARDNESS: f64 = 0.5;
/// The bow's friction curve: `φ(Δ) = Δ·(1 + δ²)/(Δ² + δ²)`.
const DELTA: f64 = 0.25;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.exciter",
    doc: "Triggered excitation for the physical models: pluck (falling ramp), strike (raised cosine, shorter when harder), noise burst, or bow (friction against a velocity input).",
    ports: &[
        event_port("trigger", Direction::In),
        signal_port("velocity", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "kind",
            PortKind::Generic,
            None,
            0.0,
            3.0,
            STRIKE,
            Combine::Replace,
        ),
        parameter("level", PortKind::Gain, None, 0.0, 1.0, LEVEL, Combine::Add),
        parameter(
            "duration_ms",
            PortKind::Control,
            Some("ms"),
            0.0,
            10_000.0,
            DURATION_MS,
            Combine::Add,
        ),
        parameter(
            "hardness",
            PortKind::Control,
            None,
            0.0,
            1.0,
            HARDNESS,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::PHYSICAL),
};

/// The kind of excitation sounding.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Kind {
    #[default]
    Pluck,
    Strike,
    Noise,
    Bow,
}

/// `std.exciter`: event-triggered excitations for `std.string`,
/// `std.modal` and `std.mass_spring`.
///
/// **Ports**
/// - `trigger` (event in, trigger): each frame with at least one `Trigger`
///   starts an excitation; several on one frame count once. A trigger during
///   an excitation restarts it. `Set` and `Ramp` events are ignored.
/// - `velocity` (signal in, audio, unbounded): the velocity of what is bowed,
///   read by the bow only; unconnected it reads 0.
/// - `out` (signal out, audio): the excitation; `[−level, level]`, and up to
///   `2.125·level` for the bow.
///
/// **Parameters**, all read on the trigger frame:
/// - `kind` (generic, no unit, choice, range `[0, 3]`, default 1, combine
///   `replace`): `round_half_up(kind)` selects 0 pluck, 1 strike, 2 noise
///   burst, 3 bow.
/// - `level` (gain, linear, range `[0, 1]`, default 1, combine `add`).
/// - `duration_ms` (control, ms, range `[0, 10 000]`, default 5, combine
///   `add`): `L = max(1, round_half_up(duration_ms·sr/1000))` frames.
/// - `hardness` (control, no unit, range `[0, 1]`, default 0.5, combine
///   `add`): the strike's contact is
///   `C = max(1, round_half_up((1 − 0.9·hardness)·duration_ms·sr/1000))`
///   frames, so a harder mallet is shorter and brighter.
///
/// **Feedthrough:** direct (a trigger, or `velocity`, on frame *n* changes
/// `out[n]`).
///
/// **Formula** (`i` = 0, 1, … counts the frames from the trigger frame, `A`
/// = `level`; out is 0 outside the excitation and before any trigger):
///
/// ```text
/// pluck   out = A·(1 − i/L)                           0 ≤ i < L
/// strike  out = A·½·(1 − cos(2π·(i + 1)/(C + 1)))      0 ≤ i < C
/// noise   out = A·(1 − i/L)·(2·u[n] − 1)              0 ≤ i < L
/// bow     out = A·sin(π·i/L)·φ(1 − velocity[n])       0 ≤ i < L
///         φ(Δ) = Δ·(1 + δ²)/(Δ² + δ²),   δ = ¼
/// ```
///
/// - **Pluck**: a shaped impulse, the release of a string pulled to `A` and
///   let go over `L` frames. Energy `Σ out² = A²·(L + 1)(2L + 1)/(6L)`.
/// - **Strike**: one period of a raised cosine over `C + 1` frames, without
///   its first 0, so every one of its `C` frames is non-zero. Energy
///   `3A²·(C + 1)/8` for `C ≥ 2` (`A²` for `C = 1`).
/// - **Noise**: the pluck's ramp on white noise. `u[n]` is one
///   `next_f64()` from the instance's stream on **every** frame, burst or
///   not, as in `std.noise`, so the kind and the triggers never shift the
///   stream. Expected energy `A²·(L + 1)(2L + 1)/(18L)`.
/// - **Bow**: a stroke of `L` frames whose pressure rises and falls as
///   `sin(π·i/L)`, at a bow speed of 1 against `velocity`. `φ` is a smooth
///   friction curve: `φ(1) = 1`, so with `velocity` 0 the output is
///   `A·sin(π·i/L)`, of energy `A²·L/2` for `L ≥ 2`; it peaks at
///   `(1 + δ²)/(2δ) = 2.125` at `Δ = δ` and falls beyond it, the negative
///   slope that lets a resonator fed back into `velocity` stick and slip.
///
/// The duration is `L` frames (`C` for the strike). `cos` and `sin` are
/// `libm`'s. `reset` restarts the stream. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Exciter {
    sample_rate: f64,
    /// The stream as `prepare` received it, and the stream being drawn from.
    streams: Option<(RngStream, RngStream)>,
    kind: Kind,
    level: f64,
    /// The frame of the excitation, and its length (0: none sounding).
    frame: usize,
    length: usize,
}

impl Exciter {
    /// Starts the excitation the parameters of frame `n` describe.
    fn start(&mut self, kind: f64, level: f64, duration_ms: f64, hardness: f64) {
        self.kind = match round_half_up(kind) as i64 {
            0 => Kind::Pluck,
            1 => Kind::Strike,
            2 => Kind::Noise,
            _ => Kind::Bow,
        };
        self.level = level;
        let mut frames = duration_ms * self.sample_rate / 1000.0;
        if self.kind == Kind::Strike {
            frames *= 1.0 - 0.9 * hardness;
        }
        self.length = round_half_up(frames).max(1.0) as usize;
        self.frame = 0;
    }

    /// The output of the current frame, given its draw and `velocity`; moves on.
    fn advance(&mut self, draw: f64, velocity: f64) -> f64 {
        if self.frame >= self.length {
            return 0.0;
        }
        let (i, length) = (self.frame as f64, self.length as f64);
        self.frame += 1;
        let shape = match self.kind {
            Kind::Pluck => 1.0 - i / length,
            Kind::Strike => {
                let phase = std::f64::consts::TAU * (i + 1.0) / (length + 1.0);
                0.5 * (1.0 - libm::cos(phase))
            }
            Kind::Noise => (1.0 - i / length) * (2.0 * draw - 1.0),
            Kind::Bow => {
                let delta = 1.0 - velocity;
                let friction = delta * (1.0 + DELTA * DELTA) / (delta * delta + DELTA * DELTA);
                libm::sin(std::f64::consts::PI * i / length) * friction
            }
        };
        self.level * shape
    }
}

impl DirectPrimitive for Exciter {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.streams = Some((ctx.rng.clone(), ctx.rng.clone()));
        self.reset();
    }

    fn reset(&mut self) {
        if let Some((initial, stream)) = &mut self.streams {
            stream.clone_from(initial);
        }
        self.kind = Kind::Pluck;
        self.level = 0.0;
        self.frame = 0;
        self.length = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let triggers = inputs.events(0);
        let velocity = inputs.signal(0);
        let (kind, level) = (inputs.parameter(0), inputs.parameter(1));
        let (duration_ms, hardness) = (inputs.parameter(2), inputs.parameter(3));
        let mut next = 0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            if triggered(triggers, &mut next, n) {
                self.start(
                    at(kind, n, STRIKE),
                    at(level, n, LEVEL),
                    at(duration_ms, n, DURATION_MS),
                    at(hardness, n, HARDNESS),
                );
            }
            let draw = match &mut self.streams {
                Some((_, stream)) => stream.next_f64(),
                None => 0.5,
            };
            *out = self.advance(draw, at(velocity, n, 0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render, stream};

    const RATE: f64 = 48_000.0;
    const TRIGGER: u64 = 10;

    /// The output of one excitation triggered on frame 10.
    fn excite(kind: f64, level: f64, duration_ms: f64, hardness: f64, velocity: f64) -> Vec<f64> {
        let frames = TRIGGER as usize + (duration_ms * RATE / 1000.0) as usize + 100;
        let mut exciter = Exciter::default();
        exciter.prepare(&context(RATE, 64, 3));
        let lanes = [
            vec![kind; frames],
            vec![level; frames],
            vec![duration_ms; frames],
            vec![hardness; frames],
        ];
        let lanes: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
        let v = vec![velocity; frames];
        render(
            &mut exciter,
            &Timeline {
                signals: &[&v],
                parameters: &lanes,
                triggers: &[&[TRIGGER]],
            },
            frames,
            64,
        )
        .signals[0]
            .clone()
    }

    /// The energy and the frames from the trigger to the last non-zero sample.
    fn energy_and_duration(out: &[f64]) -> (f64, usize) {
        assert!(out[..TRIGGER as usize].iter().all(|&y| y == 0.0));
        let energy = out.iter().map(|y| y * y).sum();
        let last = out.iter().rposition(|&y| y != 0.0).unwrap();
        (energy, last + 1 - TRIGGER as usize)
    }

    /// AC2: each exciter's energy and duration match its definition.
    #[test]
    fn energy_and_duration_match_the_definitions() {
        let a = 0.8;
        for duration_ms in [5.0, 20.0, 1000.0] {
            let l = round_half_up(duration_ms * RATE / 1000.0);

            let (energy, frames) = energy_and_duration(&excite(0.0, a, duration_ms, 0.5, 0.0));
            let expected = a * a * (l + 1.0) * (2.0 * l + 1.0) / (6.0 * l);
            println!(
                "pluck {duration_ms} ms: E {energy:.6} (expected {expected:.6}), {frames} frames"
            );
            approx::assert_relative_eq!(energy, expected, max_relative = 1e-12);
            assert_eq!(frames as f64, l);

            for hardness in [0.0, 0.5, 1.0] {
                let out = excite(1.0, a, duration_ms, hardness, 0.0);
                let (energy, frames) = energy_and_duration(&out);
                let c = round_half_up((1.0 - 0.9 * hardness) * duration_ms * RATE / 1000.0);
                let expected = 3.0 * a * a * (c + 1.0) / 8.0;
                println!(
                    "strike {duration_ms} ms, hardness {hardness}: E {energy:.6} \
                     (expected {expected:.6}), {frames} frames (C {c})"
                );
                approx::assert_relative_eq!(energy, expected, max_relative = 1e-12);
                assert_eq!(frames as f64, c);
            }

            let out = excite(2.0, a, duration_ms, 0.5, 0.0);
            let (energy, frames) = energy_and_duration(&out);
            // Exactly the formula on the replayed stream: one draw per frame.
            let mut draws = stream(3);
            let mut exact = 0.0;
            for (n, &sample) in out.iter().enumerate() {
                let u = draws.next_f64();
                if let Some(i) = n.checked_sub(TRIGGER as usize).filter(|&i| (i as f64) < l) {
                    let y = a * ((1.0 - i as f64 / l) * (2.0 * u - 1.0));
                    assert_eq!(sample, y);
                    exact += y * y;
                }
            }
            let expected = a * a * (l + 1.0) * (2.0 * l + 1.0) / (18.0 * l);
            println!(
                "noise {duration_ms} ms: E {energy:.6} (expected {expected:.6}), {frames} frames"
            );
            assert_eq!(energy, exact);
            // The expected value, within the spread of L draws.
            let tolerance = if l > 10_000.0 { 0.03 } else { 0.25 };
            approx::assert_relative_eq!(energy, expected, max_relative = tolerance);
            assert!(frames as f64 <= l && frames as f64 > l - 3.0);

            let (energy, frames) = energy_and_duration(&excite(3.0, a, duration_ms, 0.5, 0.0));
            let expected = a * a * l / 2.0;
            println!(
                "bow {duration_ms} ms: E {energy:.6} (expected {expected:.6}), {frames} frames"
            );
            approx::assert_relative_eq!(energy, expected, max_relative = 1e-12);
            // sin(π·i/L) is 0 at i = 0 only; the last non-zero frame is L − 1.
            assert_eq!(frames as f64, l);
        }
    }

    /// The bow's friction curve: at the stick point Δ = δ the force is
    /// 2.125 times the sliding force at Δ = 1, and it changes sign with Δ.
    #[test]
    fn the_bow_follows_the_friction_curve() {
        let peak = |velocity| {
            excite(3.0, 1.0, 10.0, 0.5, velocity)
                .iter()
                .fold(0.0f64, |p, &y| if y.abs() > p.abs() { y } else { p })
        };
        approx::assert_relative_eq!(peak(0.0), 1.0, max_relative = 1e-6);
        approx::assert_relative_eq!(peak(1.0 - DELTA), 2.125, max_relative = 1e-6);
        assert_eq!(peak(1.0), 0.0);
        assert!(peak(3.0) < 0.0);
    }

    /// A retrigger restarts the excitation, and `reset` restarts everything.
    #[test]
    fn a_retrigger_restarts_and_reset_restores() {
        let frames = 600;
        let mut exciter = Exciter::default();
        exciter.prepare(&context(RATE, 64, 3));
        let lanes = [
            vec![0.0; frames],
            vec![1.0; frames],
            vec![5.0; frames],
            vec![0.5; frames],
        ];
        let lanes: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
        let timeline = Timeline {
            signals: &[&[0.0; 600]],
            parameters: &lanes,
            triggers: &[&[0, 100]],
        };
        let a = render(&mut exciter, &timeline, frames, 64).signals[0].clone();
        assert_eq!((a[0], a[100]), (1.0, 1.0));
        assert_eq!(a[339], 1.0 - 239.0 / 240.0);
        assert_eq!(a[340], 0.0);
        exciter.reset();
        let b = render(&mut exciter, &timeline, frames, 1).signals[0].clone();
        assert_eq!(a, b);
    }
}
