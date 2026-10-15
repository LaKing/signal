//! `std.particles`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PortShape, PrepareContext,
};

use crate::bridge::{WINDOW_ROWS, Window, at_tick, effect, mean};
use crate::svf::flush;
use crate::{VISUAL_DIVISOR, at, fract, parameter_at, port_at};

/// Particle slots.
pub(crate) const PARTICLES: usize = 64;
/// The golden ratio's fractional part: emission *k*'s direction is spread by
/// `frac(k·φ)`, a low-discrepancy sequence with no random stream.
const PHI: f64 = 0.618_033_988_749_894_9;

const D: u32 = VISUAL_DIVISOR;

const RATE: usize = 0;
const X: usize = 1;
const Y: usize = 2;
const ANGLE: usize = 3;
const SPREAD: usize = 4;
const SPEED: usize = 5;
const LIFETIME_S: usize = 6;
const GRAVITY: usize = 7;
const DRAG: usize = 8;
const SPRING_HZ: usize = 9;
const IMPULSE_SPEED: usize = 10;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.particles",
    doc: "64 particles at the visual rate (rate divisor 800) with gravity, drag and springs to their anchors; emitted and pushed through std.count windows (emit, impulse) or at a rate; outputs positions, velocities and remaining life as vectors.",
    ports: &[
        port_at(
            "emit",
            Direction::In,
            PortShape::Vector(WINDOW_ROWS as u32),
            1,
            PortKind::Generic,
            None,
            None,
        ),
        port_at(
            "impulse",
            Direction::In,
            PortShape::Vector(WINDOW_ROWS as u32),
            1,
            PortKind::Generic,
            None,
            None,
        ),
        port_at(
            "pos",
            Direction::Out,
            PortShape::Vector(2 * PARTICLES as u32),
            D,
            PortKind::Control,
            None,
            None,
        ),
        port_at(
            "vel",
            Direction::Out,
            PortShape::Vector(2 * PARTICLES as u32),
            D,
            PortKind::Control,
            None,
            None,
        ),
        port_at(
            "life",
            Direction::Out,
            PortShape::Vector(PARTICLES as u32),
            D,
            PortKind::Control,
            None,
            None,
        ),
    ],
    parameters: &[
        parameter_at(
            "rate",
            D,
            PortKind::Control,
            Some("1/s"),
            0.0,
            1000.0,
            0.0,
            Combine::Add,
        ),
        parameter_at(
            "x",
            D,
            PortKind::Control,
            None,
            -1.0,
            1.0,
            0.0,
            Combine::Add,
        ),
        parameter_at(
            "y",
            D,
            PortKind::Control,
            None,
            -1.0,
            1.0,
            -0.5,
            Combine::Add,
        ),
        parameter_at(
            "angle",
            D,
            PortKind::Control,
            Some("rad"),
            -4.0,
            4.0,
            std::f64::consts::FRAC_PI_2,
            Combine::Add,
        ),
        parameter_at(
            "spread",
            D,
            PortKind::Control,
            Some("rad"),
            0.0,
            7.0,
            1.0,
            Combine::Add,
        ),
        parameter_at(
            "speed",
            D,
            PortKind::Control,
            Some("1/s"),
            0.0,
            100.0,
            1.0,
            Combine::Multiply,
        ),
        parameter_at(
            "lifetime_s",
            D,
            PortKind::Control,
            Some("s"),
            0.05,
            60.0,
            2.0,
            Combine::Add,
        ),
        parameter_at(
            "gravity",
            D,
            PortKind::Control,
            Some("1/s²"),
            -100.0,
            100.0,
            -1.0,
            Combine::Add,
        ),
        parameter_at(
            "drag",
            D,
            PortKind::Control,
            Some("1/s"),
            0.0,
            100.0,
            0.5,
            Combine::Add,
        ),
        parameter_at(
            "spring_hz",
            D,
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            1000.0,
            0.0,
            Combine::Add,
        ),
        parameter_at(
            "impulse_speed",
            D,
            PortKind::Control,
            Some("1/s"),
            0.0,
            100.0,
            1.0,
            Combine::Multiply,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// One particle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Particle {
    p: [f64; 2],
    v: [f64; 2],
    anchor: [f64; 2],
    /// Seconds left, 0 when dead.
    left: f64,
    lifetime: f64,
}

/// `std.particles`: 64 point masses at the visual rate (divisor 800, 60 Hz at
/// 48 kHz), in the units of the canvas (the box [−1, 1]², y up).
///
/// **Events** (operator decision operator-request:007): a type above
/// divisor 1 cannot have event ports (`docs/semantics.md` §16), so `emit`
/// and `impulse` are vectors of 4 at divisor 1, each the window of one
/// `std.count` connected at scale 1 ([`crate::bridge`]). At each tick the
/// window holds the events since the previous tick: an event at frame *f*
/// acts at the first tick at or after *f*, at most one tick late, once.
///
/// **Ports**
/// - `emit` (window in): `n` emissions at the tick, at speed `speed` times
///   the tick's mean event value `S/n` (a trigger is worth 1), formed
///   without overflow ([`crate::bridge`]'s `mean`). Every one is emitted in
///   order: the emission index advances by `n`, and slot `k mod 64` takes
///   emission `k`, the oldest replaced, so only the last 64 of a tick can be
///   seen; nothing is capped or dropped.
/// - `impulse` (window in): every live particle gets `S × impulse_speed` of
///   speed away from the emitter (straight along `angle` when on it).
/// - `pos`, `vel` (vectors of 128 out): particle *k*'s x and y at elements
///   2*k* and 2*k* + 1. A dead particle keeps its last position and
///   velocity.
/// - `life` (vector of 64 out): the fraction of its lifetime left, in
///   [0, 1]; 0 is dead (every slot is dead at the start).
///
/// **Parameters** (at divisor 800): `rate` (emissions per second, `[0,
/// 1000]`, default 0: fractions carry over ticks), `x`, `y` (the emitter,
/// default (0, −½)), `angle` (rad, default π/2, up), `spread` (rad, default
/// 1), `speed` (default 1, `multiply`), `lifetime_s` (default 2), `gravity`
/// (along y, default −1), `drag` (1/s, default ½), `spring_hz` (to the
/// anchor, default 0), `impulse_speed` (default 1, `multiply`).
///
/// **Formula** (h = 800 / sample rate; per tick: emissions, the impulse,
/// then every live particle, velocity first as in `std.mass_spring`):
///
/// ```text
/// emission k: p = anchor = (x, y),  θ = angle + spread·(frac(k·φ) − ½),
///             v = s·(cos θ, sin θ),  s = speed·S/n (emit) or speed (rate)
/// a = (0, gravity) − ω²·(p − anchor),  ω = 2π·min(spring_hz, 1/(2πh))
/// v ← v + h·a;   p ← p + h·v;   v ← e^(−drag·h)·v;   life ← life − h
/// ```
///
/// Without drag and spring a particle emitted with `p₀, v₀` is, after *n*
/// ticks, exactly at `p₀ + n·h·v₀ + h²·g·n(n+1)/2` (the free fall's closed
/// form, offset from the continuous `p₀ + v₀t + gt²/2` by `g·h·t/2`).
/// Velocities and positions are flushed to 0 below 1e-9
/// (`docs/dsp-conventions.md`). Work per tick is proportional to the 64
/// slots.
#[derive(Clone, Debug)]
pub struct Particles {
    h: f64,
    particles: [Particle; PARTICLES],
    /// Emissions so far.
    emitted: u64,
    /// Fractional emissions of `rate` carried to the next tick.
    carry: f64,
    drag: f64,
    decay: f64,
}

impl Default for Particles {
    fn default() -> Particles {
        Particles {
            h: f64::from(D) / 48_000.0,
            particles: [Particle::default(); PARTICLES],
            emitted: 0,
            carry: 0.0,
            drag: f64::NAN,
            decay: 1.0,
        }
    }
}

/// The parameters of one tick.
struct Tick {
    x: f64,
    y: f64,
    angle: f64,
    spread: f64,
    speed: f64,
    lifetime: f64,
    gravity: f64,
    w2: f64,
    impulse_speed: f64,
}

impl Particles {
    /// Emits `count` particles at `speed`, in order.
    fn emit(&mut self, count: u64, speed: f64, tick: &Tick) {
        let first = self.emitted + count.saturating_sub(PARTICLES as u64);
        for k in first..self.emitted + count {
            let theta = tick.angle + tick.spread * (fract(k as f64 * PHI) - 0.5);
            let slot = (k % PARTICLES as u64) as usize;
            if let Some(particle) = self.particles.get_mut(slot) {
                *particle = Particle {
                    p: [tick.x, tick.y],
                    v: [speed * libm::cos(theta), speed * libm::sin(theta)],
                    anchor: [tick.x, tick.y],
                    left: tick.lifetime,
                    lifetime: tick.lifetime,
                };
            }
        }
        self.emitted += count;
    }

    /// The impulse `speed` away from the emitter.
    fn push(&mut self, speed: f64, tick: &Tick) {
        if speed == 0.0 {
            return;
        }
        for particle in self.particles.iter_mut().filter(|q| q.left > 0.0) {
            let d = [particle.p[0] - tick.x, particle.p[1] - tick.y];
            let length = libm::sqrt(d[0] * d[0] + d[1] * d[1]);
            let unit = if length > 0.0 {
                [d[0] / length, d[1] / length]
            } else {
                [libm::cos(tick.angle), libm::sin(tick.angle)]
            };
            particle.v[0] += speed * unit[0];
            particle.v[1] += speed * unit[1];
        }
    }

    /// One tick of every live particle.
    fn integrate(&mut self, tick: &Tick) {
        let (h, decay) = (self.h, self.decay);
        for particle in self.particles.iter_mut().filter(|q| q.left > 0.0) {
            let axes = particle.p.iter_mut().zip(particle.v.iter_mut());
            for (axis, ((p, v), anchor)) in axes.zip(particle.anchor).enumerate() {
                let g = if axis == 1 { tick.gravity } else { 0.0 };
                let a = g - tick.w2 * (*p - anchor);
                let moved = *v + h * a;
                *p = flush(*p + h * moved);
                *v = flush(decay * moved);
            }
            particle.left = (particle.left - h).max(0.0);
        }
    }

    /// One tick with its two windows.
    fn tick(&mut self, emit: Window, impulse: Window, rate: f64, tick: &Tick) {
        self.carry += rate.max(0.0) * self.h;
        let regular = libm::floor(self.carry);
        self.carry -= regular;
        self.emit(emit.n, mean(emit, tick.speed), tick);
        self.emit(regular as u64, tick.speed, tick);
        self.push(effect(impulse, tick.impulse_speed), tick);
        self.integrate(tick);
    }
}

impl DirectPrimitive for Particles {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.h = f64::from(D) / ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        let h = self.h;
        *self = Particles::default();
        self.h = h;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let ticks = inputs.frames();
        let p = |index: usize, t: usize| {
            let fallback = DESCRIPTOR.parameters.get(index).map_or(0.0, |d| d.default);
            at(inputs.parameter(index), t, fallback)
        };
        let emit = inputs.rows(0, WINDOW_ROWS);
        let impulse = inputs.rows(WINDOW_ROWS, WINDOW_ROWS);
        for t in 0..ticks {
            let drag = p(DRAG, t);
            if drag != self.drag {
                self.drag = drag;
                self.decay = libm::exp(-drag.max(0.0) * self.h);
            }
            let spring = p(SPRING_HZ, t)
                .max(0.0)
                .min(1.0 / (std::f64::consts::TAU * self.h));
            let w = std::f64::consts::TAU * spring;
            let tick = Tick {
                x: p(X, t),
                y: p(Y, t),
                angle: p(ANGLE, t),
                spread: p(SPREAD, t),
                speed: p(SPEED, t),
                lifetime: p(LIFETIME_S, t),
                gravity: p(GRAVITY, t),
                w2: w * w,
                impulse_speed: p(IMPULSE_SPEED, t),
            };
            self.tick(
                at_tick(emit, ticks, t),
                at_tick(impulse, ticks, t),
                p(RATE, t),
                &tick,
            );
            for (k, q) in self.particles.iter().enumerate() {
                for (axis, (&p, &v)) in q.p.iter().zip(&q.v).enumerate() {
                    if let Some(out) = outputs.signal(2 * k + axis).get_mut(t) {
                        *out = p;
                    }
                    if let Some(out) = outputs.signal(2 * PARTICLES + 2 * k + axis).get_mut(t) {
                        *out = v;
                    }
                }
                let life = if q.lifetime > 0.0 {
                    (q.left / q.lifetime).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                if let Some(out) = outputs.signal(4 * PARTICLES + k).get_mut(t) {
                    *out = life;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::Window;

    fn trigger(n: u64) -> Window {
        match n {
            0 => Window::EMPTY,
            _ => {
                let (m, e) = libm::frexp(n as f64);
                Window { n, m, e }
            }
        }
    }

    fn tick(gravity: f64) -> Tick {
        Tick {
            x: 0.25,
            y: -0.5,
            angle: 1.0,
            spread: 0.0,
            speed: 2.0,
            lifetime: 60.0,
            gravity,
            w2: 0.0,
            impulse_speed: 1.0,
        }
    }

    /// AC2: free fall without drag or spring is exactly the discrete closed
    /// form `p₀ + n·h·v₀ + h²·g·n(n+1)/2`, and within `g·h·t/2` of the
    /// continuous one.
    #[test]
    fn free_fall_follows_the_closed_form() {
        let mut particles = Particles::default();
        particles.prepare(&crate::testing::context(48_000.0, 256, 0));
        particles.decay = 1.0;
        particles.drag = 0.0;
        let h = particles.h;
        let (g, step) = (-9.81, tick(-9.81));
        particles.tick(trigger(1), Window::EMPTY, 0.0, &step);
        let v0 = [2.0 * libm::cos(1.0), 2.0 * libm::sin(1.0)];
        let p0 = [0.25, -0.5];
        for n in 1..=3000u32 {
            if n > 1 {
                particles.tick(Window::EMPTY, Window::EMPTY, 0.0, &step);
            }
            let q = particles.particles[0];
            let n = f64::from(n);
            let discrete = [
                p0[0] + n * h * v0[0],
                p0[1] + n * h * v0[1] + h * h * g * n * (n + 1.0) / 2.0,
            ];
            let t = n * h;
            let continuous = p0[1] + v0[1] * t + g * t * t / 2.0;
            for (axis, expected) in discrete.into_iter().enumerate() {
                let tolerance = 1e-12 * (1.0 + expected.abs());
                assert!((q.p[axis] - expected).abs() <= tolerance, "tick {n}: {q:?}");
            }
            let offset = (q.p[1] - continuous).abs();
            assert!(offset <= (g * h * t / 2.0).abs() * (1.0 + 1e-9) + 1e-12);
        }
    }

    /// Finding auditor-1-002: 65 events in one tick are 65 emissions, the
    /// last 64 of them in the slots; the index advances by 65.
    #[test]
    fn no_emission_is_capped() {
        let mut particles = Particles::default();
        particles.prepare(&crate::testing::context(48_000.0, 256, 0));
        let step = tick(0.0);
        particles.tick(trigger(65), Window::EMPTY, 0.0, &step);
        assert_eq!(particles.emitted, 65);
        assert!(particles.particles.iter().all(|q| q.left > 0.0));
        // Slot 0 holds emission 64 (the 65th), born at the same tick: with
        // a spread, its direction is emission 64's, not emission 0's.
        let mut spread = Particles::default();
        spread.prepare(&crate::testing::context(48_000.0, 256, 0));
        let step = Tick {
            spread: 1.0,
            ..tick(0.0)
        };
        spread.tick(trigger(65), Window::EMPTY, 0.0, &step);
        let q = spread.particles[0];
        let theta = 1.0 + (fract(64.0 * PHI) - 0.5);
        assert!((libm::atan2(q.v[1], q.v[0]) - theta).abs() < 1e-12);
    }

    /// The mean value per event scales the speed: Set(2.5) twice emits at
    /// 2.5 × speed; a payload whose sum binary64 cannot hold still gives
    /// its finite mean (finding auditor-1-005).
    #[test]
    fn emission_speed_is_the_mean_value() {
        let mut particles = Particles::default();
        particles.prepare(&crate::testing::context(48_000.0, 256, 0));
        let step = tick(0.0);
        let (m, e) = libm::frexp(5.0);
        particles.tick(Window { n: 2, m, e }, Window::EMPTY, 0.0, &step);
        let q = particles.particles[1];
        let speed = libm::sqrt(q.v[0] * q.v[0] + q.v[1] * q.v[1]);
        // `tick` without `process` keeps the decay at 1 (no drag).
        assert!((speed - 2.5 * 2.0).abs() < 1e-12);
        let (m1, e1) = libm::frexp(1e308);
        let w = Window {
            n: 2,
            m: m1,
            e: e1 + 1,
        };
        assert_eq!(mean(w, 1.0), 1e308);
    }
}
