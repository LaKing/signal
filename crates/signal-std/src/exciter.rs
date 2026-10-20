//! `std.exciter`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext, RngStream,
};

use crate::svf::flush;
use crate::{at, event_port, parameter, round_half_up, signal_port, triggered};

/// The default `kind`: strike.
const STRIKE: f64 = 1.0;
const LEVEL: f64 = 1.0;
const DURATION_MS: f64 = 5.0;
const HARDNESS: f64 = 0.5;
const BOW_SPEED: f64 = 1.0;
/// The bow's default friction curve: `φ(Δ) = Δ·(1 + δ²)/(Δ² + δ²)`.
const DELTA: f64 = 0.25;
/// The parameters, in descriptor order.
const PARAMETERS: usize = 8;
/// The index of `bow_speed`, the first parameter added in M2.10-8.
const ADDED: usize = 4;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.exciter",
    doc: "Triggered excitation for the physical models: pluck (falling ramp), strike (raised cosine, shorter when harder), noise burst (coloured), or bow (friction against a velocity input); level, duration, hardness and the bow follow their inputs at audio rate, and a position low-pass dulls a strike nearer the middle.",
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
        parameter(
            "bow_speed",
            PortKind::Control,
            None,
            -4.0,
            4.0,
            BOW_SPEED,
            Combine::Add,
        ),
        parameter(
            "friction",
            PortKind::Control,
            None,
            0.01,
            1.0,
            DELTA,
            Combine::Add,
        ),
        parameter(
            "noise_color",
            PortKind::Control,
            None,
            -1.0,
            1.0,
            0.0,
            Combine::Add,
        ),
        parameter(
            "position",
            PortKind::Control,
            None,
            0.0,
            0.5,
            0.0,
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
///   `(1 + δ²)/(2δ)·level` for the bow (2.125·level at the default δ).
///
/// **Parameters.** `kind` is read on the trigger frame and kept for the
/// excitation; every other parameter is read on every frame.
/// - `kind` (generic, no unit, choice, range `[0, 3]`, default 1, combine
///   `replace`): `round_half_up(kind)` selects 0 pluck, 1 strike, 2 noise
///   burst, 3 bow.
/// - `level` (gain, linear, range `[0, 1]`, default 1, combine `add`): `A`,
///   applied on every frame.
/// - `duration_ms` (control, ms, range `[0, 10 000]`, default 5, combine
///   `add`): `L = max(1, round_half_up(duration_ms·sr/1000))` frames.
/// - `hardness` (control, no unit, range `[0, 1]`, default 0.5, combine
///   `add`): the strike's contact is
///   `C = max(1, round_half_up((1 − 0.9·hardness)·duration_ms·sr/1000))`
///   frames, so a harder mallet is shorter and brighter.
/// - `bow_speed` (control, range `[−4, 4]`, default 1, combine `add`): the
///   bow's speed `s`, against `velocity`.
/// - `friction` (control, `δ`, range `[0.01, 1]`, default 0.25, combine
///   `add`): the width of the bow's friction curve; smaller is grippier.
/// - `noise_color` (control, `c`, range `[−1, 1]`, default 0, combine
///   `add`): the noise burst's colour: white at 0, a one-pole low-pass
///   towards 1 (darker), a first difference towards −1 (brighter).
/// - `position` (control, fraction, range `[0, 0.5]`, default 0, combine
///   `add`): where the body is excited, from the end (0) to the middle
///   (½): a one-pole low-pass on the excitation, so nearer the middle is
///   duller.
///
/// **Feedthrough:** direct (a trigger, or `velocity`, on frame *n* changes
/// `out[n]`).
///
/// **Formula** (`i` = 0, 1, … counts the frames from the trigger frame, `A`
/// = `level[n]`; out is 0 outside the excitation and before any trigger):
///
/// ```text
/// pluck   x = A·(1 − i/L)                           0 ≤ i < L
/// strike  x = A·½·(1 − cos(2π·(i + 1)/(C + 1)))      0 ≤ i < C
/// noise   x = A·(1 − i/L)·w[n]                      0 ≤ i < L
/// bow     x = A·sin(π·i/L)·φ(s[n] − velocity[n])    0 ≤ i < L
///         φ(Δ) = Δ·(1 + δ²)/(Δ² + δ²)
/// w[n]    = 2·u[n] − 1                              c = 0
///         = w[n−1]' + (1 − c)·(2·u[n] − 1 − w[n−1]')    c > 0 (w' the coloured noise)
///         = (2·u[n] − 1 + c·(2·u[n−1] − 1))/(1 − c)      c < 0
/// out     = x                                       position = 0
///         = (1 − a)·x + a·out[n−1],  a = 1.96·position    position > 0
/// ```
///
/// - **Pluck**: a shaped impulse, the release of a string pulled to `A` and
///   let go over `L` frames. Energy `Σ out² = A²·(L + 1)(2L + 1)/(6L)`.
/// - **Strike**: one period of a raised cosine over `C + 1` frames, without
///   its first 0, so every one of its `C` frames is non-zero. Energy
///   `3A²·(C + 1)/8` for `C ≥ 2` (`A²` for `C = 1`).
/// - **Noise**: the pluck's ramp on noise. `u[n]` is one `next_f64()` from
///   the instance's stream on **every** frame, burst or not, as in
///   `std.noise`, so the kind and the triggers never shift the stream, and
///   the colour filters run on every frame too. Both filters keep `|w| ≤ 1`
///   (a unity-gain convex low-pass, a normalised difference). White,
///   expected energy `A²·(L + 1)(2L + 1)/(18L)`.
/// - **Bow**: a stroke of `L` frames whose pressure rises and falls as
///   `sin(π·i/L)`, at a bow speed `s` against `velocity`. `φ` is a smooth
///   friction curve: `φ(1) = 1`, so at the defaults with `velocity` 0 the
///   output is `A·sin(π·i/L)`, of energy `A²·L/2` for `L ≥ 2`; it peaks at
///   `(1 + δ²)/(2δ)` (2.125 at δ = ¼) at `Δ = δ` and falls beyond it, the
///   negative slope that lets a resonator fed back into `velocity` stick and
///   slip.
/// - **Position**: the low-pass has unit gain at DC, so `|out| ≤ max|x|`; it
///   rings on after the excitation, flushed to exactly 0 below 1e-9. At 0 it
///   is not applied.
///
/// **At audio rate.** `L` (`C`) is recomputed on every frame from that
/// frame's `duration_ms` and `hardness`. While it equals the excitation's
/// length the formulas above run as written, so a parameter held through an
/// excitation gives exactly the output it gave when it was read on the
/// trigger frame only. When it changes, the excitation keeps its progress:
/// with `p` the phase of the frame before (`i/L`, or `(i + 1)/(C + 1)` for the
/// strike), each frame moves it on by the step of that frame's length `L'`,
/// `s' = 1/L'` (`1/(C' + 1)`), but by no more than
/// `max((1 − p)/2, s₀)`, `s₀` the step of the length on the trigger frame;
/// it ends when the phase reaches 1, and the shapes above are evaluated at
/// the phase. So the shape never jumps: a length cut short (even to one
/// frame) runs the rest of the excitation out geometrically, halving what
/// remains on each frame, and the excitation ends at a phase within `s₀` of
/// 1, as close to the shape's end as it would have at its own length; a
/// length moving on every frame is as safe as a fixed one.
///
/// **Changed in M2.10-8:** `level`, `duration_ms` and `hardness` were read on
/// the trigger frame only and now act on every frame (ADR 0016 §4: a
/// parameter's effective value is per frame). A project that holds them
/// through each excitation (every existing golden does) renders bit for bit
/// as before; one that modulates them during an excitation now hears the
/// modulation.
///
/// **Non-finite values.** A bow friction that is not finite (an infinite or
/// NaN `velocity`, or one beyond about 1.7·10³⁰⁸ where `Δ·(1 + δ²)`
/// overflows; beyond about 1.34·10¹⁵⁴ `Δ²` overflows and `φ` is ±0, which is
/// finite) reads as 0, its limit; a NaN added parameter reads as its
/// default; an output that is not finite (a NaN `level`) reads as 0; and the
/// filter states keep only finite values. Each acts only where the exciter
/// before M2.10-8 produced a value that was not finite on that frame.
///
/// `cos` and `sin` are `libm`'s. `reset` restarts the stream and clears the
/// filters. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Exciter {
    sample_rate: f64,
    /// The stream as `prepare` received it, and the stream being drawn from.
    streams: Option<(RngStream, RngStream)>,
    kind: Kind,
    /// Whether an excitation sounds; its frame `i` and length `L` (`C`).
    active: bool,
    frame: usize,
    length: usize,
    /// The phase step of the excitation's length on its trigger frame; after
    /// a length change, the phase of the last frame emitted.
    rebased: bool,
    first_step: f64,
    last: f64,
    /// The colour filters' states: the last draw `2u − 1`, and the last
    /// coloured value.
    white: f64,
    coloured: f64,
    /// The position low-pass's last output.
    tone: f64,
}

impl Exciter {
    /// The length of an excitation of `kind` for `duration_ms` and
    /// `hardness`.
    fn length(&self, kind: Kind, duration_ms: f64, hardness: f64) -> usize {
        let mut frames = duration_ms * self.sample_rate / 1000.0;
        if kind == Kind::Strike {
            frames *= 1.0 - 0.9 * hardness;
        }
        round_half_up(frames).max(1.0) as usize
    }

    /// The phase of frame `i` of an excitation of length `length`, and the
    /// step per frame.
    fn phase(&self, i: usize, length: usize) -> (f64, f64) {
        let (i, length) = (i as f64, length as f64);
        match self.kind {
            Kind::Strike => ((i + 1.0) / (length + 1.0), 1.0 / (length + 1.0)),
            _ => (i / length, 1.0 / length),
        }
    }

    /// Starts the excitation the parameters of frame `n` describe.
    fn start(&mut self, kind: f64, duration_ms: f64, hardness: f64) {
        self.kind = match round_half_up(kind) as i64 {
            0 => Kind::Pluck,
            1 => Kind::Strike,
            2 => Kind::Noise,
            _ => Kind::Bow,
        };
        self.length = self.length(self.kind, duration_ms, hardness);
        self.first_step = self.phase(0, self.length).1;
        self.frame = 0;
        self.active = true;
        self.rebased = false;
    }

    /// The colour of this frame's draw, given `noise_color` `c`; moves the
    /// colour filters on.
    fn colour(&mut self, draw: f64, c: f64) -> f64 {
        let w = 2.0 * draw - 1.0;
        let coloured = if c == 0.0 {
            w
        } else if c > 0.0 {
            self.coloured + (1.0 - c) * (w - self.coloured)
        } else {
            (w + c * self.white) / (1.0 - c)
        };
        self.white = w;
        self.coloured = if coloured.is_finite() { coloured } else { 0.0 };
        coloured
    }

    /// The excitation of the current frame, before `level` and the position
    /// low-pass, given its coloured draw, `velocity` and the parameters `p`;
    /// moves on.
    fn advance(&mut self, coloured: f64, velocity: f64, p: &[f64; PARAMETERS]) -> f64 {
        if !self.active {
            return 0.0;
        }
        let [_, _, duration_ms, hardness, bow_speed, delta, ..] = *p;
        let length = self.length(self.kind, duration_ms, hardness);
        if !self.rebased && length != self.length && self.frame > 0 {
            self.last = self.phase(self.frame - 1, self.length).0;
            self.rebased = true;
        }
        let shape = if self.rebased {
            // The step the length asks for, but at most half the phase that
            // remains unless that is below the trigger frame's step: a
            // shortened excitation runs out geometrically and ends no more
            // abruptly than it would have at its own length.
            let wanted = self.phase(0, length).1;
            let step = wanted.min((0.5 * (1.0 - self.last)).max(self.first_step));
            let phase = self.last + step;
            if phase >= 1.0 {
                self.active = false;
                return 0.0;
            }
            self.last = phase;
            match self.kind {
                Kind::Pluck => 1.0 - phase,
                Kind::Strike => 0.5 * (1.0 - libm::cos(std::f64::consts::TAU * phase)),
                Kind::Noise => (1.0 - phase) * coloured,
                Kind::Bow => {
                    libm::sin(std::f64::consts::PI * phase) * friction(bow_speed - velocity, delta)
                }
            }
        } else {
            if self.frame >= self.length {
                self.active = false;
                return 0.0;
            }
            let (i, length) = (self.frame as f64, self.length as f64);
            match self.kind {
                Kind::Pluck => 1.0 - i / length,
                Kind::Strike => {
                    let phase = std::f64::consts::TAU * (i + 1.0) / (length + 1.0);
                    0.5 * (1.0 - libm::cos(phase))
                }
                Kind::Noise => (1.0 - i / length) * coloured,
                Kind::Bow => {
                    libm::sin(std::f64::consts::PI * i / length)
                        * friction(bow_speed - velocity, delta)
                }
            }
        };
        self.frame += 1;
        shape
    }
}

/// The bow's friction curve `Δ·(1 + δ²)/(Δ² + δ²)`, 0 where it is not finite
/// (its limit as `|Δ|` grows).
fn friction(delta: f64, width: f64) -> f64 {
    let phi = delta * (1.0 + width * width) / (delta * delta + width * width);
    if phi.is_finite() { phi } else { 0.0 }
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
        self.active = false;
        self.frame = 0;
        self.length = 0;
        self.rebased = false;
        (self.white, self.coloured, self.tone) = (0.0, 0.0, 0.0);
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let triggers = inputs.events(0);
        let velocity = inputs.signal(0);
        let lanes: [&[f64]; PARAMETERS] = std::array::from_fn(|i| inputs.parameter(i));
        let defaults = DESCRIPTOR.parameters;
        let mut next = 0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let p: [f64; PARAMETERS] = std::array::from_fn(|i| {
                let default = defaults.get(i).map_or(0.0, |d| d.default);
                let value = at(lanes.get(i).copied().unwrap_or(&[]), n, default);
                if i >= ADDED && value.is_nan() {
                    default
                } else {
                    value
                }
            });
            let [
                kind,
                level,
                duration_ms,
                hardness,
                _,
                _,
                noise_color,
                position,
            ] = p;
            if triggered(triggers, &mut next, n) {
                self.start(kind, duration_ms, hardness);
            }
            let draw = match &mut self.streams {
                Some((_, stream)) => stream.next_f64(),
                None => 0.5,
            };
            let coloured = self.colour(draw, noise_color);
            let shape = self.advance(coloured, at(velocity, n, 0.0), &p);
            let x = if self.active { level * shape } else { 0.0 };
            let y = if position == 0.0 {
                x
            } else {
                let a = 1.96 * position;
                flush((1.0 - a) * x + a * self.tone)
            };
            let y = if y.is_finite() { y } else { 0.0 };
            self.tone = y;
            *out = y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render, stream};

    const RATE: f64 = 48_000.0;
    const TRIGGER: u64 = 10;

    /// Every parameter's lane at its default, with `set` held.
    fn lanes(frames: usize, set: &[(usize, f64)]) -> Vec<Vec<f64>> {
        let mut lanes: Vec<Vec<f64>> = DESCRIPTOR
            .parameters
            .iter()
            .map(|p| vec![p.default; frames])
            .collect();
        for &(i, value) in set {
            lanes[i] = vec![value; frames];
        }
        lanes
    }

    /// Renders `lanes`, `velocity` and `triggers` through an exciter
    /// prepared with seed 3, in calls of `block`.
    fn run(lanes: &[Vec<f64>], velocity: &[f64], triggers: &[u64], block: usize) -> Vec<f64> {
        let mut exciter = Exciter::default();
        exciter.prepare(&context(RATE, 64, 3));
        let lanes: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
        render(
            &mut exciter,
            &Timeline {
                signals: &[velocity],
                parameters: &lanes,
                triggers: &[triggers],
            },
            velocity.len(),
            block,
        )
        .signals[0]
            .clone()
    }

    /// The output of one excitation triggered on frame 10.
    fn excite(kind: f64, level: f64, duration_ms: f64, hardness: f64, velocity: f64) -> Vec<f64> {
        let frames = TRIGGER as usize + (duration_ms * RATE / 1000.0) as usize + 100;
        let lanes = lanes(
            frames,
            &[(0, kind), (1, level), (2, duration_ms), (3, hardness)],
        );
        run(&lanes, &vec![velocity; frames], &[TRIGGER], 64)
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
        let lanes = lanes(frames, &[(0, 0.0), (1, 1.0), (2, 5.0), (3, 0.5)]);
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

    /// The exciter as it was before M2.10-8 (M2.8), verbatim: parameters
    /// read on the trigger frame only.
    #[derive(Default)]
    struct Legacy {
        stream: Option<RngStream>,
        kind: Kind,
        level: f64,
        frame: usize,
        length: usize,
    }

    impl Legacy {
        fn start(&mut self, kind: f64, level: f64, duration_ms: f64, hardness: f64) {
            self.kind = match round_half_up(kind) as i64 {
                0 => Kind::Pluck,
                1 => Kind::Strike,
                2 => Kind::Noise,
                _ => Kind::Bow,
            };
            self.level = level;
            let mut frames = duration_ms * RATE / 1000.0;
            if self.kind == Kind::Strike {
                frames *= 1.0 - 0.9 * hardness;
            }
            self.length = round_half_up(frames).max(1.0) as usize;
            self.frame = 0;
        }

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

        /// Renders the first four parameter lanes.
        fn run(lanes: &[Vec<f64>], velocity: &[f64], triggers: &[u64]) -> Vec<f64> {
            let mut legacy = Legacy {
                stream: Some(stream(3)),
                ..Legacy::default()
            };
            (0..velocity.len())
                .map(|n| {
                    if triggers.contains(&(n as u64)) {
                        legacy.start(lanes[0][n], lanes[1][n], lanes[2][n], lanes[3][n]);
                    }
                    let draw = legacy.stream.as_mut().unwrap().next_f64();
                    legacy.advance(draw, velocity[n])
                })
                .collect()
        }
    }

    fn uniform(s: &mut RngStream, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * s.next_f64()
    }

    /// T-E1 (AC7): with the added parameters at their defaults and the
    /// parameters held through each excitation (they may change on trigger
    /// frames), the exciter is bit-identical to the one before M2.10-8 for
    /// every kind, in calls of 1, 64 and 256 frames.
    #[test]
    fn held_parameters_are_bit_identical_to_the_exciter_before() {
        let mut s = stream(29);
        for case in 0..16 {
            let frames = 20_000;
            let triggers: Vec<u64> = (0..12).map(|j| j * 1600 + (case as u64 * 7) % 50).collect();
            let mut lanes = lanes(frames, &[]);
            let mut p = [0.0; 4];
            for n in 0..frames {
                if triggers.contains(&(n as u64)) || n == 0 {
                    p = [
                        if case < 4 {
                            case as f64
                        } else {
                            uniform(&mut s, 0.0, 3.0)
                        },
                        uniform(&mut s, 0.0, 1.0),
                        uniform(&mut s, 0.0, 30.0),
                        uniform(&mut s, 0.0, 1.0),
                    ];
                }
                for (lane, &v) in lanes.iter_mut().zip(&p) {
                    lane[n] = v;
                }
            }
            let velocity: Vec<f64> = (0..frames).map(|_| uniform(&mut s, -2.0, 2.0)).collect();
            let expected = Legacy::run(&lanes, &velocity, &triggers);
            for block in [1, 64, 256] {
                let y = run(&lanes, &velocity, &triggers, block);
                for n in 0..frames {
                    assert_eq!(
                        y[n].to_bits(),
                        expected[n].to_bits(),
                        "case {case}, block {block}, frame {n}"
                    );
                }
            }
        }
    }

    /// AC7: the duration and hardness may move on every frame. A pluck whose
    /// duration sweeps keeps its ramp continuous (no step larger than the
    /// longer of the two lengths' steps) and ends; a strike under random
    /// hardness and duration stays within `[0, level]` and ends; `level`
    /// scales each frame; a `kind` that moves during an excitation changes
    /// nothing until the next trigger.
    #[test]
    fn the_shape_follows_the_parameters_at_audio_rate() {
        let frames = 4000;
        let mut l = lanes(frames, &[(0, 0.0), (1, 1.0)]);
        for (n, d) in l[2].iter_mut().enumerate() {
            *d = 20.0 + 15.0 * libm::sin(n as f64 * 0.01);
        }
        let y = run(&l, &vec![0.0; frames], &[0], 64);
        let shortest = round_half_up(5.0 * RATE / 1000.0);
        let mut ended = false;
        for n in 1..frames {
            assert!(
                (y[n] - y[n - 1]).abs() <= 1.0 / shortest + 1e-12,
                "frame {n}"
            );
            assert!((0.0..=1.0).contains(&y[n]));
            ended |= y[n] == 0.0;
        }
        assert!(ended);

        let mut s = stream(31);
        let mut l = lanes(frames, &[(0, 1.0)]);
        l[1].fill(0.7);
        let (head, tail) = l.split_at_mut(3);
        for (duration, hardness) in head[2].iter_mut().zip(tail[0].iter_mut()) {
            (*duration, *hardness) = (uniform(&mut s, 0.0, 30.0), uniform(&mut s, 0.0, 1.0));
        }
        let y = run(&l, &vec![0.0; frames], &[0, 2000], 1);
        assert!(y.iter().all(|&y| (0.0..=0.7).contains(&y)));
        assert!(y[1990..2000].iter().all(|&y| y == 0.0), "the strike ends");

        let mut l = lanes(frames, &[(0, 0.0), (2, 20.0)]);
        let plain = run(&l, &vec![0.0; frames], &[0], 64);
        for (n, level) in l[1].iter_mut().enumerate() {
            *level = 0.5 + 0.5 * libm::sin(n as f64 * 0.05);
        }
        let scaled = run(&l, &vec![0.0; frames], &[0], 64);
        for ((y, level), p) in scaled.iter().zip(&l[1]).zip(&plain) {
            assert_eq!(*y, level * p);
        }
        let mut l = lanes(frames, &[(0, 0.0), (2, 20.0)]);
        for (n, kind) in l[0].iter_mut().enumerate().skip(1) {
            *kind = (n % 4) as f64;
        }
        assert_eq!(run(&l, &vec![0.0; frames], &[0], 64), plain);
    }

    /// Review of M2.10-8: a duration cut short mid-excitation (20 ms, 960
    /// frames, to 0 ms, one frame, from frame 300 on) does not cut the
    /// excitation off. The pluck falls by at most half of itself per frame
    /// (it halves what remains), or one trigger-length step near its end,
    /// and its last non-zero value is at most one
    /// trigger-length step, `1/960`, as at its own end; every kind ends
    /// within `log₂ 960 + 3` frames of the cut, its last non-zero value at
    /// most `π·φ_max/960` (the strike's and the bow's shape within one step
    /// of their end), where the exciter before the fix jumped from about
    /// 0.69 to 0.
    #[test]
    fn a_shortened_excitation_runs_out_without_a_cut() {
        let frames = 2000;
        let cut = 300;
        let s0 = 1.0 / 960.0;
        for kind in [0.0, 1.0, 3.0] {
            let mut l = lanes(frames, &[(0, kind), (2, 20.0), (3, 0.0)]);
            l[2][cut..].fill(0.0);
            let y = run(&l, &vec![0.0; frames], &[0], 64);
            assert!(y[cut - 1].abs() > 0.1, "kind {kind} sounds before the cut");
            let end = (cut..frames).find(|&n| y[n] == 0.0).unwrap();
            assert!(
                end - cut <= 13,
                "kind {kind}: ends {} frames after the cut",
                end - cut
            );
            let phi_max = (1.0 + 0.0625) / 0.5;
            assert!(
                y[end - 1].abs() <= std::f64::consts::PI * phi_max * s0,
                "kind {kind}: last value {}",
                y[end - 1]
            );
            assert!(y[end..].iter().all(|&v| v == 0.0));
            if kind == 0.0 {
                for n in cut..end {
                    assert!(
                        y[n] <= y[n - 1] && y[n - 1] - y[n] <= (0.5 * y[n - 1]).max(s0) + 1e-12
                    );
                }
                assert!(y[end - 1] <= s0 + 1e-12);
            }
        }
    }

    /// AC7: `noise_color` darkens (towards 1) or brightens (towards −1) the
    /// burst, keeping it within ±level; `position` dulls the strike and keeps
    /// it within its peak, and its tail is flushed to exactly 0. The
    /// brightness is the energy of the first difference over the energy.
    #[test]
    fn colour_and_position_shape_the_spectrum() {
        let frames = 20_000;
        let brightness = |y: &[f64]| {
            let e: f64 = y.iter().map(|v| v * v).sum();
            let d: f64 = y.windows(2).map(|w| (w[1] - w[0]) * (w[1] - w[0])).sum();
            d / e
        };
        let noise = |c: f64| {
            let l = lanes(frames, &[(0, 2.0), (1, 0.9), (2, 400.0), (6, c)]);
            run(&l, &vec![0.0; frames], &[0], 64)
        };
        let (white, dark, bright) = (noise(0.0), noise(0.9), noise(-0.9));
        println!(
            "noise brightness: white {:.3}, c 0.9 {:.3}, c −0.9 {:.3}",
            brightness(&white),
            brightness(&dark),
            brightness(&bright)
        );
        assert!(brightness(&dark) < 0.5 * brightness(&white));
        assert!(brightness(&bright) > 1.2 * brightness(&white));
        assert!(dark.iter().chain(&bright).all(|y| y.abs() <= 0.9));
        let strike = |position: f64| {
            let l = lanes(frames, &[(0, 1.0), (2, 2.0), (3, 1.0), (7, position)]);
            run(&l, &vec![0.0; frames], &[0], 64)
        };
        let (edge, middle) = (strike(0.0), strike(0.5));
        let peak = edge.iter().fold(0.0f64, |p, y| p.max(y.abs()));
        println!(
            "strike brightness: edge {:.4}, middle {:.4}",
            brightness(&edge),
            brightness(&middle)
        );
        assert!(brightness(&middle) < 0.5 * brightness(&edge));
        assert!(middle.iter().all(|y| y.abs() <= peak));
        assert!(middle[frames - 100..].iter().all(|&y| y == 0.0));
    }

    /// T-E2 (AC7, AC6): non-finite and overflowing values. A bow against a
    /// `velocity` of 10³⁰⁸, 1.7·10³⁰⁸, ±∞ or NaN, or 1.5·10¹⁵⁴ (where the
    /// friction is a finite ±0, as before), and a NaN on every parameter
    /// during a bow stroke and a noise burst: the output stays finite and
    /// within the bow's peak `(1 + δ²)/(2δ)·level`; with the parameters held,
    /// it equals the exciter before M2.10-8 on every frame before that one's
    /// first value that is not finite.
    #[test]
    fn non_finite_values_keep_the_exciter_finite() {
        let frames = 2000;
        let bound = |y: &[f64], peak: f64, what: &str| {
            assert!(y.iter().all(|y| y.is_finite() && y.abs() <= peak), "{what}");
        };
        for v in [
            1e308,
            1.7e308,
            1.5e154,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ] {
            let l = lanes(frames, &[(0, 3.0), (2, 20.0)]);
            let mut velocity = vec![0.3; frames];
            velocity[100] = v;
            let y = run(&l, &velocity, &[0], 64);
            bound(&y, 2.125, &format!("velocity {v}"));
            let old = Legacy::run(&l, &velocity, &[0]);
            for n in 0..frames {
                if !old[n].is_finite() {
                    break;
                }
                assert_eq!(y[n].to_bits(), old[n].to_bits(), "velocity {v}, frame {n}");
            }
            if v == 1.5e154 {
                assert!(
                    old.iter().all(|y| y.is_finite()),
                    "finite before at 1.5e154"
                );
            }
        }
        for kind in [2.0, 3.0] {
            for i in 0..PARAMETERS {
                let mut l = lanes(frames, &[(0, kind), (2, 20.0)]);
                l[i][100] = f64::NAN;
                let y = run(&l, &vec![0.3; frames], &[0], 64);
                bound(
                    &y,
                    (1.0 + 0.0625) / 0.5,
                    &format!("kind {kind}, NaN parameter {i}"),
                );
                if i >= ADDED {
                    assert_eq!(
                        y,
                        run(
                            &lanes(frames, &[(0, kind), (2, 20.0)]),
                            &vec![0.3; frames],
                            &[0],
                            64
                        )
                    );
                }
            }
        }
    }
}
