//! `std.mesh2d` and `std.mesh2d_audio`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext,
};

use crate::bridge::{WINDOW_ROWS, at_tick, effect};
use crate::count::value;
use crate::svf::flush;
use crate::{VISUAL_DIVISOR, at, parameter_at, port_at};

/// Nodes per side of the grid.
pub(crate) const GRID: usize = 16;
/// Nodes.
pub(crate) const NODES: usize = GRID * GRID;
/// The largest `spring_hz` declared: the stability bound at the highest
/// sample rate the engine accepts; `process` limits it to the bound of the
/// rate it was prepared at.
const SPRING_MAX: f64 = u32::MAX as f64 / std::f64::consts::TAU;

/// Parameter indices.
const SPRING_HZ: usize = 0;
const DECAY_S: usize = 1;
const STRIKE_X: usize = 2;
const STRIKE_Y: usize = 3;
const STRENGTH: usize = 4;
const PICKUP_X: usize = 5;
const PICKUP_Y: usize = 6;

const fn parameters(d: u32, spring: f64, decay: f64) -> [signal_core::ParameterDescriptor; 7] {
    [
        parameter_at(
            "spring_hz",
            d,
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            SPRING_MAX,
            spring,
            Combine::Add,
        ),
        parameter_at(
            "decay_s",
            d,
            PortKind::Control,
            Some("s"),
            0.0,
            600.0,
            decay,
            Combine::Add,
        ),
        parameter_at(
            "strike_x",
            d,
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.5,
            Combine::Add,
        ),
        parameter_at(
            "strike_y",
            d,
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.5,
            Combine::Add,
        ),
        parameter_at(
            "strength",
            d,
            PortKind::Control,
            None,
            0.0,
            10_000.0,
            1.0,
            Combine::Multiply,
        ),
        parameter_at(
            "pickup_x",
            d,
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.3,
            Combine::Add,
        ),
        parameter_at(
            "pickup_y",
            d,
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.6,
            Combine::Add,
        ),
    ]
}

const VISUAL_PORTS: [PortDescriptor; 4] = [
    port_at(
        "strike",
        Direction::In,
        PortShape::Vector(WINDOW_ROWS as u32),
        1,
        PortKind::Generic,
        None,
        None,
    ),
    port_at(
        "drive",
        Direction::In,
        PortShape::Signal,
        1,
        PortKind::Control,
        None,
        None,
    ),
    port_at(
        "z",
        Direction::Out,
        PortShape::Vector(NODES as u32),
        VISUAL_DIVISOR,
        PortKind::Control,
        None,
        None,
    ),
    port_at(
        "out",
        Direction::Out,
        PortShape::Signal,
        VISUAL_DIVISOR,
        PortKind::Control,
        None,
        None,
    ),
];

const AUDIO_PORTS: [PortDescriptor; 4] = [
    port_at(
        "strike",
        Direction::In,
        PortShape::Events,
        1,
        PortKind::Trigger,
        None,
        None,
    ),
    port_at(
        "drive",
        Direction::In,
        PortShape::Signal,
        1,
        PortKind::Audio,
        None,
        None,
    ),
    port_at(
        "z",
        Direction::Out,
        PortShape::Vector(NODES as u32),
        1,
        PortKind::Control,
        None,
        None,
    ),
    port_at(
        "out",
        Direction::Out,
        PortShape::Signal,
        1,
        PortKind::Audio,
        None,
        None,
    ),
];

const VISUAL_PARAMETERS: [signal_core::ParameterDescriptor; 7] =
    parameters(VISUAL_DIVISOR, 2.0, 3.0);
const AUDIO_PARAMETERS: [signal_core::ParameterDescriptor; 7] = parameters(1, 150.0, 1.5);

const VISUAL_DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.mesh2d",
    doc: "A 16 × 16 mass-spring membrane between fixed walls at the visual rate (rate divisor 800, 60 Hz at 48 kHz), symplectic Euler; struck through a std.count window (strike) or pushed (drive); outputs every node's displacement (z, vector of 256) and one node's (out).",
    ports: &VISUAL_PORTS,
    parameters: &VISUAL_PARAMETERS,
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

const AUDIO_DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.mesh2d_audio",
    doc: "std.mesh2d at audio rate (rate divisor 1): strike takes events directly, out is the audio-rate pickup.",
    ports: &AUDIO_PORTS,
    parameters: &AUDIO_PARAMETERS,
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// `std.mesh2d` (`Mesh2d<false>`, at the visual divisor 800) and
/// `std.mesh2d_audio` (`Mesh2d<true>`, at divisor 1): a square membrane of
/// 16 × 16 equal masses joined by springs to their four neighbours, with
/// fixed walls around it. One implementation; a type's rate is fixed by its
/// descriptor (`docs/module-contract.md`, "Rates and vectors"), so the two
/// rates are two types.
///
/// **Ports**
/// - `strike`: an impulse at the strike node, in velocity per unit
///   `strength`. `std.mesh2d`: a vector of 4 at divisor 1, the window of one
///   `std.count` connected at scale 1 ([`crate::bridge`]): at each tick it
///   adds `S × strength`, the summed values of the events since the previous
///   tick (impulses within one tick superpose; nothing is capped); an event
///   at frame *f* acts at the first tick at or after *f*. `std.mesh2d_audio`:
///   an event input, each event acting once on its own frame with its value
///   (`Trigger` 1, `Set(v)` v, `Ramp` its target) times `strength`; several
///   on one frame add.
/// - `drive` (signal in, at divisor 1): a force at the strike node, in units
///   of the displacement it would hold against one spring; read at the
///   ticks; unconnected 0.
/// - `z` (vector of 256 out): the displacement of node (*i*, *j*) at element
///   `j·16 + i`.
/// - `out` (signal out): the displacement at the pickup node (`audio` in
///   `std.mesh2d_audio`).
///
/// **Parameters** (at the type's rate)
/// - `spring_hz` (Hz, `[0, (2³² − 1)/2π]`, default 2 visual, 150 audio,
///   `add`): `ω_s/2π` of one spring on one mass, limited to the stability
///   bound `r/(2π√2)`, `r` the tick rate (sample rate / divisor).
/// - `decay_s` (s, `[0, 600]`, default 3 visual, 1.5 audio, `add`): the T60
///   of every mode; **0 is undamped**.
/// - `strike_x`, `strike_y`, `pickup_x`, `pickup_y` (`[0, 1]`, defaults ½,
///   ½, 0.3, 0.6, `add`): node `round_half_up(x·15)`.
/// - `strength` (`[0, 10 000]`, default 1, `multiply`).
///
/// **Formula** (h = divisor / sample rate; per tick, velocity first):
///
/// ```text
/// ω_s = 2π·min(spring_hz, 1/(2π√2·h))     so h·ω_s ≤ 1/√2
/// v_s ← v_s + effect(strike)                the strike node
/// a   = −ω_s²·(L·z) + ω_s²·drive·[node = s],  (L·z)_ij = 4z_ij − Σ neighbours (walls 0)
/// v ← v + h·a                               every node, from the old z
/// z ← z + h·v                               every node, from the new v
/// z ← g·z, v ← g·v,  g = e^(−3 ln 10·h/decay_s) (1 when decay_s = 0)
/// ```
///
/// Symplectic (semi-implicit) Euler, the integrator of `std.mass_spring` and
/// of `std.particles` and `std.body2d`. Undamped with fixed parameters it
/// keeps exactly the quadratic form `E = |v|² + zᵀK·z − h·vᵀK·z` (`K =
/// ω_s²L`), which is positive definite while `h·ω < 2` for every mode; the
/// largest mode of the 16 × 16 grid has `ω² = ω_s²(4 + 4 cos(π/17)) < 8ω_s²`,
/// so the clamp keeps it stable. The plain energy `|v|² + zᵀK·z` then stays
/// within a factor `1 ± h·ω_max/2` of `E`: bounded, without drift.
///
/// **Changing parameters.** On a tick where `ω_s` differs from the tick
/// before, the state is scaled so that `E` does not grow (as
/// `std.mass_spring`), so a modulated membrane stays bounded.
///
/// When damped, each `z` and `v` is flushed to exactly 0 below 1e-9
/// (`docs/dsp-conventions.md`, "Denormals"); undamped nothing decays, and
/// a flush would only take energy out. Work per tick is proportional
/// to the 256 nodes.
#[derive(Clone, Debug)]
pub struct Mesh2d<const AUDIO: bool> {
    tick_rate: f64,
    z: [f64; NODES],
    v: [f64; NODES],
    decay_s: f64,
    g: f64,
    /// `ω_s²` of the last tick (`None`: none yet).
    w2: Option<f64>,
}

impl<const AUDIO: bool> Default for Mesh2d<AUDIO> {
    fn default() -> Self {
        Mesh2d {
            tick_rate: 48_000.0,
            z: [0.0; NODES],
            v: [0.0; NODES],
            decay_s: f64::NAN,
            g: 1.0,
            w2: None,
        }
    }
}

/// The node of grid position `x` in `[0, 1]`.
fn cell(x: f64) -> usize {
    (crate::round_half_up(x.clamp(0.0, 1.0) * (GRID - 1) as f64) as usize).min(GRID - 1)
}

impl<const AUDIO: bool> Mesh2d<AUDIO> {
    /// `(L·z)` at node `k`, walls 0.
    fn laplacian(&self, k: usize) -> f64 {
        let (i, j) = (k % GRID, k / GRID);
        let z = |i: usize, j: usize| self.z.get(j * GRID + i).copied().unwrap_or(0.0);
        let here = z(i, j);
        let mut sum = 4.0 * here;
        if i > 0 {
            sum -= z(i - 1, j);
        }
        if i + 1 < GRID {
            sum -= z(i + 1, j);
        }
        if j > 0 {
            sum -= z(i, j - 1);
        }
        if j + 1 < GRID {
            sum -= z(i, j + 1);
        }
        sum
    }

    /// `E = |v|² + zᵀK·z − h·vᵀK·z` for `K = w2·L`.
    pub(crate) fn energy(&self, w2: f64, h: f64) -> f64 {
        (0..NODES)
            .map(|k| {
                let lz = w2 * self.laplacian(k);
                let (z, v) = (self.z.get(k), self.v.get(k));
                let (z, v) = (z.copied().unwrap_or(0.0), v.copied().unwrap_or(0.0));
                v * v + z * lz - h * v * lz
            })
            .sum()
    }

    /// One tick, the strike's velocity `impulse` and `force` at node `s`.
    fn step(&mut self, w2: f64, h: f64, s: usize, impulse: f64, force: f64) {
        if let Some(old) = self.w2
            && old != w2
        {
            let before = self.energy(old, h);
            let after = self.energy(w2, h);
            if after > before {
                let scale = libm::sqrt(before.max(0.0) / after);
                for (z, v) in self.z.iter_mut().zip(self.v.iter_mut()) {
                    *z *= scale;
                    *v *= scale;
                }
            }
        }
        self.w2 = Some(w2);
        if let Some(v) = self.v.get_mut(s) {
            *v += impulse;
        }
        for k in 0..NODES {
            let mut a = -w2 * self.laplacian(k);
            if k == s {
                a += w2 * force;
            }
            if let Some(v) = self.v.get_mut(k) {
                *v += h * a;
            }
        }
        let g = self.g;
        for (z, v) in self.z.iter_mut().zip(self.v.iter_mut()) {
            *z += h * *v;
            // Only decaying state needs the flush; undamped, it would
            // take energy out.
            if g < 1.0 {
                *z = flush(g * *z);
                *v = flush(g * *v);
            }
        }
    }
}

impl<const AUDIO: bool> DirectPrimitive for Mesh2d<AUDIO> {
    fn describe() -> ModuleDescriptor {
        if AUDIO {
            AUDIO_DESCRIPTOR
        } else {
            VISUAL_DESCRIPTOR
        }
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        let divisor = if AUDIO {
            1.0
        } else {
            f64::from(VISUAL_DIVISOR)
        };
        self.tick_rate = ctx.sample_rate / divisor;
        self.reset();
    }

    fn reset(&mut self) {
        let tick_rate = self.tick_rate;
        *self = Mesh2d::default();
        self.tick_rate = tick_rate;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let ticks = inputs.frames();
        let h = 1.0 / self.tick_rate;
        let defaults = Self::describe().parameters;
        let p = |index: usize, t: usize| {
            let fallback = defaults.get(index).map_or(0.0, |d| d.default);
            at(inputs.parameter(index), t, fallback)
        };
        // The bridge's rows (std.mesh2d) or the events (std.mesh2d_audio),
        // then the drive row.
        let (window, drive) = if AUDIO {
            (&[][..], inputs.signal(0))
        } else {
            (inputs.rows(0, WINDOW_ROWS), inputs.signal(WINDOW_ROWS))
        };
        let events = inputs.events(0);
        let mut next = 0;
        for t in 0..ticks {
            let spring = p(SPRING_HZ, t)
                .max(0.0)
                .min(self.tick_rate / (std::f64::consts::TAU * std::f64::consts::SQRT_2));
            let decay_s = p(DECAY_S, t);
            if decay_s != self.decay_s {
                self.decay_s = decay_s;
                self.g = if decay_s > 0.0 {
                    libm::exp(-3.0 * std::f64::consts::LN_10 / decay_s * h)
                } else {
                    1.0
                };
            }
            let s = cell(p(STRIKE_Y, t)) * GRID + cell(p(STRIKE_X, t));
            let strength = p(STRENGTH, t);
            let impulse = if AUDIO {
                let mut sum = 0.0;
                while let Some(event) = events.get(next) {
                    if event.offset as usize > t {
                        break;
                    }
                    if event.offset as usize == t {
                        sum += value(event.payload) * strength;
                    }
                    next += 1;
                }
                sum
            } else {
                effect(at_tick(window, ticks, t), strength)
            };
            let w = std::f64::consts::TAU * spring;
            self.step(w * w, h, s, impulse, at(drive, t, 0.0));
            let pickup = cell(p(PICKUP_Y, t)) * GRID + cell(p(PICKUP_X, t));
            let z = outputs.rows(0, NODES);
            for (k, x) in self.z.iter().enumerate() {
                if let Some(out) = z.get_mut(k * ticks + t) {
                    *out = *x;
                }
            }
            if let Some(out) = outputs.signal(NODES).get_mut(t) {
                *out = self.z.get(pickup).copied().unwrap_or(0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signal_core::{Event, EventPayload};

    fn ready<const A: bool>(sample_rate: f64) -> Mesh2d<A> {
        let mut mesh = Mesh2d::<A>::default();
        mesh.prepare(&crate::testing::context(sample_rate, 256, 0));
        mesh
    }

    /// Finding auditor-2-energy-test-cost: driven directly, 200 000 ticks of
    /// an undamped struck membrane at the clamp: `E` drifts by at most 1e-9
    /// relative, and the plain energy stays within its bound without growth.
    #[test]
    fn the_undamped_mesh_keeps_its_energy() {
        let mut mesh = ready::<false>(48_000.0);
        let h = 1.0 / mesh.tick_rate;
        mesh.g = 1.0;
        mesh.decay_s = 0.0;
        // Half the clamp, so that the plain-energy bound says something;
        // `the_mesh_is_stable_at_the_clamp` takes the clamp itself.
        let w = 0.5 / (h * std::f64::consts::SQRT_2);
        let w2 = w * w;
        mesh.step(w2, h, 3 * GRID + 5, 1.0, 0.0);
        let e0 = mesh.energy(w2, h);
        let omega_max = w * libm::sqrt(4.0 + 4.0 * libm::cos(std::f64::consts::PI / 17.0));
        let bound = h * omega_max / 2.0;
        let (mut worst_e, mut lo, mut hi) = (0.0f64, f64::MAX, 0.0f64);
        for _ in 0..200_000 {
            mesh.step(w2, h, 0, 0.0, 0.0);
            let e = mesh.energy(w2, h);
            worst_e = worst_e.max((e / e0 - 1.0).abs());
            let plain: f64 = (0..NODES)
                .map(|k| mesh.v[k] * mesh.v[k] + mesh.z[k] * w2 * mesh.laplacian(k))
                .sum();
            lo = lo.min(plain / e0);
            hi = hi.max(plain / e0);
        }
        println!("E drift {worst_e:e}; plain/E in [{lo}, {hi}], bound 1 ± {bound}");
        assert!(worst_e <= 1e-9);
        assert!(lo >= 1.0 / (1.0 + bound) - 1e-9 && hi <= 1.0 / (1.0 - bound) + 1e-9);
    }

    /// At the clamp `h·ω_s = 1/√2` the undamped membrane stays bounded: E
    /// is kept and every displacement stays finite and small.
    #[test]
    fn the_mesh_is_stable_at_the_clamp() {
        let mut mesh = ready::<false>(192_000.0);
        let h = 1.0 / mesh.tick_rate;
        mesh.g = 1.0;
        let w = 1.0 / (h * std::f64::consts::SQRT_2);
        mesh.step(w * w, h, 0, 1.0, 0.0);
        let e0 = mesh.energy(w * w, h);
        for _ in 0..100_000 {
            mesh.step(w * w, h, 0, 0.0, 0.0);
        }
        let e = mesh.energy(w * w, h);
        println!("E at the clamp: {e0} -> {e}");
        assert!((e / e0 - 1.0).abs() <= 1e-6);
        assert!(mesh.z.iter().all(|z| z.is_finite() && z.abs() < 1.0));
    }

    /// A modulated spring adds no energy: alternating between 0 and the
    /// clamp every tick keeps E from growing.
    #[test]
    fn a_modulated_spring_adds_no_energy() {
        let mut mesh = ready::<false>(44_100.0);
        let h = 1.0 / mesh.tick_rate;
        let w2 = 1.0 / (2.0 * h * h);
        mesh.step(w2, h, 40, 1.0, 0.0);
        let e0 = mesh.energy(w2, h);
        for k in 0..10_000 {
            mesh.step(if k % 2 == 0 { 0.0 } else { w2 }, h, 0, 0.0, 0.0);
        }
        mesh.step(w2, h, 0, 0.0, 0.0);
        assert!(mesh.energy(w2, h) <= e0 * (1.0 + 1e-9));
    }

    /// Finding auditor-2-window-length-must-match-consumer-divisor: one
    /// strike into std.mesh2d_audio acts exactly once, on its own frame:
    /// the strike node's velocity rises by exactly `strength` there, and the
    /// undamped membrane's E is constant afterwards; Set(2.5) acts as 2.5,
    /// and two events on one frame add.
    #[test]
    fn one_strike_acts_exactly_once() {
        let run = |events: &[Event], strength: f64| {
            let mut mesh = ready::<true>(48_000.0);
            let frames = 2000;
            let mut params: Vec<Vec<f64>> = AUDIO_PARAMETERS
                .iter()
                .map(|p| vec![p.default; frames])
                .collect();
            params[DECAY_S] = vec![0.0; frames];
            params[STRENGTH] = vec![strength; frames];
            let flat: Vec<f64> = params.concat();
            let drive = vec![0.0; frames];
            let mut out = vec![0.0; (NODES + 1) * frames];
            mesh.process(
                &Inputs::new(frames, &drive, &flat, events, &[events.len()]),
                &mut Outputs::new(frames, &mut out, &mut []),
            );
            mesh
        };
        let strike = |offset, payload| Event { offset, payload };
        let one = [strike(10, EventPayload::Trigger)];
        let mesh = run(&one, 0.75);
        let h = 1.0 / mesh.tick_rate;
        let w = std::f64::consts::TAU * 150.0;
        // The same impulse applied once by hand, then the same ticks.
        let mut by_hand = ready::<true>(48_000.0);
        by_hand.g = 1.0;
        by_hand.decay_s = 0.0;
        let s = cell(0.5) * GRID + cell(0.5);
        for t in 0..2000 {
            by_hand.step(w * w, h, s, if t == 10 { 0.75 } else { 0.0 }, 0.0);
        }
        assert_eq!(mesh.z, by_hand.z);
        assert_eq!(mesh.v, by_hand.v);
        let e = mesh.energy(w * w, h);
        assert!(e > 0.0);
        let mut later = mesh.clone();
        for _ in 0..8000 {
            later.step(w * w, h, s, 0.0, 0.0);
        }
        assert!((later.energy(w * w, h) / e - 1.0).abs() <= 1e-9);
        let set = run(&[strike(10, EventPayload::Set(2.5))], 1.0);
        let two = run(
            &[
                strike(10, EventPayload::Trigger),
                strike(10, EventPayload::Set(1.5)),
            ],
            1.0,
        );
        assert_eq!(set.z, two.z);
    }
}
