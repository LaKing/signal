//! `std.mass_spring`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::svf::flush;
use crate::{at, parameter, round_half_up, signal_port};

/// The most masses a chain has.
pub(crate) const MAX_MASSES: usize = 32;
/// The ends `boundary` selects (ADR 0043: its value set).
const BOUNDARY_VALUES: &[signal_core::ValueLabel] = &[
    crate::named(0.0, "fixed–fixed"),
    crate::named(1.0, "fixed–free"),
];

const MASSES: f64 = 8.0;
const SPRING_HZ: f64 = 100.0;
/// The upper end of `spring_hz`'s declared range: the stability bound
/// `sr/2π` at the highest sample rate the engine accepts.
/// ADR 0043: 100 kHz; `process` limits it to the stability bound.
const SPRING_MAX: f64 = 100_000.0;
const DECAY_S: f64 = 2.0;
const DRIVE_NODE: f64 = 1.0;
const PICKUP_NODE: f64 = 2.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.mass_spring",
    doc: "1D chain of up to 32 equal masses joined by springs, fixed-fixed or fixed-free, integrated with symplectic Euler; driven at one node and heard at another. spring_hz is clamped to the stability bound sr/2π.",
    ports: &[
        signal_port("drive", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "masses",
            PortKind::Generic,
            None,
            1.0,
            MAX_MASSES as f64,
            MASSES,
            Combine::Add,
        ),
        parameter(
            "spring_hz",
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            SPRING_MAX,
            SPRING_HZ,
            Combine::Add,
        ),
        parameter(
            "decay_s",
            PortKind::Control,
            Some("s"),
            0.01,
            60.0,
            DECAY_S,
            Combine::Add,
        ),
        crate::choice(
            parameter(
                "boundary",
                PortKind::Generic,
                None,
                0.0,
                1.0,
                0.0,
                Combine::Replace,
            ),
            BOUNDARY_VALUES,
        ),
        parameter(
            "drive_node",
            PortKind::Generic,
            None,
            1.0,
            MAX_MASSES as f64,
            DRIVE_NODE,
            Combine::Add,
        ),
        parameter(
            "pickup_node",
            PortKind::Generic,
            None,
            1.0,
            MAX_MASSES as f64,
            PICKUP_NODE,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::PHYSICAL),
};

/// `std.mass_spring`: a one-dimensional chain of equal masses and springs.
///
/// **Ports**
/// - `drive` (signal in, audio, unbounded): a force on the drive node, in
///   units of the displacement it would hold against one spring; unconnected
///   it reads 0.
/// - `out` (signal out, audio): the displacement of the pickup node.
///
/// **Parameters**
/// - `masses` (generic, integer, range `[1, 32]`, default 8, combine `add`):
///   `M = round_half_up(masses)`.
/// - `spring_hz` (frequency, Hz, range `[0, sr/2π]`, default 100, combine
///   `add`): `ω_s/2π`, where `ω_s² = k/m` is one spring on one mass. Declared
///   `[0, 100 000]` (ADR 0043); `process` limits it to the stability bound `sr/2π`
///   of the rate it was prepared at.
/// - `decay_s` (control, seconds, range `[0.01, 60]`, default 2, combine
///   `add`): the T60 of every mode.
/// - `boundary` (generic, no unit, choice, range `[0, 1]`, default 0,
///   combine `replace`): `round_half_up(boundary)` selects 0 fixed–fixed
///   (a spring to a wall at each end) or 1 fixed–free (a wall before mass 1
///   only).
/// - `drive_node`, `pickup_node` (generic, integer, range `[1, 32]`, defaults
///   1 and 2, combine `add`): rounded with `round_half_up` and limited to
///   `M`.
///
/// All are read on every frame.
///
/// **Feedthrough:** direct (`drive[n]` moves the masses on frame *n*).
///
/// **Formula** (h = 1/sr; masses j = 1 … M with displacement x_j and
/// velocity v_j, all 0 at the start; per frame):
///
/// ```text
/// ω_s  = 2π·min(max(spring_hz, 0), sr/2π)          so h·ω_s ≤ 1
/// γ    = 6·ln 10 / decay_s,   g = e^(−γ·h/2)
/// x₀   = 0;  x_{M+1} = 0 (fixed–fixed) or x_M (fixed–free)
/// a_j  = ω_s²·(x_{j−1} − 2x_j + x_{j+1}) + ω_s²·drive[n]·[j = drive_node]
/// v_j ← v_j + h·a_j                                every j, from the old x
/// x_j ← x_j + h·v_j                                every j, from the new v
/// x_j ← g·x_j,   v_j ← g·v_j                       the damping
/// out[n] = x_pickup
/// ```
///
/// This is symplectic (semi-implicit) Euler, with the damping applied as an
/// exact decay of the whole state. With `K = ω_s²·L` (`L` the chain's
/// stiffness matrix, `a = −K·x + drive`) the undamped step keeps exactly the
/// quadratic form
///
/// ```text
/// E = |v|² + xᵀK·x − h·vᵀK·x
/// ```
///
/// which is positive semi-definite while `h·ω ≤ 2` for every mode. Each mode
/// of angular frequency ω therefore rotates on an ellipse of `E`, decays by
/// exactly `g` per frame (T60 = `decay_s`) and rings at
///
/// ```text
/// Ω = acos(1 − h²ω²/2) · sr                              rad/s
/// ω_n = 2ω_s·sin(n·π / (2(M + 1)))                       fixed–fixed, n = 1 … M
/// ω_n = 2ω_s·sin((2n − 1)·π / (2(2M + 1)))               fixed–free, n = 1 … M
/// ```
///
/// the chain's normal modes, warped by the integrator (`Ω → ω` as
/// `h·ω → 0`). A constant drive `u` on a single fixed–fixed mass settles at
/// `x = ω_s²·u / (2ω_s² + (1 − g)²/(g·h²))`, `u/2` as the damping vanishes.
///
/// **Stability bound.** The step is stable while `|1 − h²ω²/2| < 1`, that is
/// `h·ω < 2`. Every mode has `ω < 2ω_s`, so `h·ω_s ≤ 1` (`spring_hz ≤ sr/2π`)
/// keeps every chain stable, and `process` clamps `spring_hz` to it.
///
/// **Changing parameters.** A stiffness that changes while the chain moves
/// can pump energy into it (parametric excitation), without bound when it
/// alternates every frame, so on a frame where `ω_s`, `boundary` or `M`
/// differs from the frame before, before the step: the masses beyond a
/// smaller `M` are set to rest, and if `E` under the new values exceeds `E`
/// under the old ones, the whole state is scaled by `√(E_old/E_new)`. A
/// change of parameters therefore never adds energy, every step under fixed
/// ones keeps `E` and the damping shrinks it, so only the drive adds energy
/// and the chain stays bounded under any modulation. With constant
/// parameters nothing is scaled.
///
/// Each `x_j` and `v_j` is flushed to exactly 0 below 1e-9
/// (`docs/dsp-conventions.md`, "Denormals"). `exp` and `sqrt` are `libm`'s;
/// `exp` is computed only when `decay_s` changes. Work per frame is
/// proportional to `M`, at most 32, twice that on a frame where the chain
/// changes.
#[derive(Clone, Debug, Default)]
pub struct MassSpring {
    sample_rate: f64,
    x: [f64; MAX_MASSES],
    v: [f64; MAX_MASSES],
    /// The `decay_s` that `g` was computed for (NaN: none yet).
    decay_s: f64,
    g: f64,
    /// The chain of the last frame: `ω_s²`, fixed–free, `M` (`None`: none yet).
    chain: Option<(f64, bool, usize)>,
}

/// `round_half_up(value)` limited to `[1, max]`, as an index from 0.
fn node(value: f64, max: usize) -> usize {
    (round_half_up(value).max(1.0) as usize).min(max) - 1
}

impl MassSpring {
    /// `(L·x)_j` of the first `masses` masses.
    fn stiffness(&self, j: usize, masses: usize, free: bool) -> f64 {
        let x = &self.x;
        let left = j.checked_sub(1).and_then(|i| x.get(i)).copied();
        let here = x.get(j).copied().unwrap_or(0.0);
        let right = match x.get(j + 1) {
            Some(&r) if j + 1 < masses => r,
            _ if free => here,
            _ => 0.0,
        };
        2.0 * here - left.unwrap_or(0.0) - right
    }

    /// `E = |v|² + xᵀK·x − h·vᵀK·x` of the first `masses` masses.
    fn energy(&self, (w2, free, masses): (f64, bool, usize), h: f64) -> f64 {
        let mut energy = 0.0;
        for (j, (&x, &v)) in self.x.iter().zip(&self.v).enumerate().take(masses) {
            let lx = w2 * self.stiffness(j, masses, free);
            energy += v * v + x * lx - h * v * lx;
        }
        energy
    }

    /// Makes a change of the chain from `old` to `new` add no energy.
    fn change(&mut self, old: (f64, bool, usize), new: (f64, bool, usize), h: f64) {
        let before = self.energy(old, h);
        let rest = self.x.iter_mut().zip(self.v.iter_mut()).skip(new.2);
        for (x, v) in rest {
            (*x, *v) = (0.0, 0.0);
        }
        let after = self.energy(new, h);
        if after > before {
            let scale = libm::sqrt(before.max(0.0) / after);
            for (x, v) in self.x.iter_mut().zip(self.v.iter_mut()) {
                *x *= scale;
                *v *= scale;
            }
        }
    }
}

impl DirectPrimitive for MassSpring {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.x = [0.0; MAX_MASSES];
        self.v = [0.0; MAX_MASSES];
        self.decay_s = f64::NAN;
        self.chain = None;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let drive = inputs.signal(0);
        let h = 1.0 / self.sample_rate;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let masses = node(at(inputs.parameter(0), n, MASSES), MAX_MASSES) + 1;
            let spring = at(inputs.parameter(1), n, SPRING_HZ)
                .max(0.0)
                .min(self.sample_rate / std::f64::consts::TAU);
            let decay_s = at(inputs.parameter(2), n, DECAY_S);
            let free = crate::chosen(
                BOUNDARY_VALUES,
                signal_core::Snap::Nearest,
                at(inputs.parameter(3), n, 0.0),
            ) >= 1.0;
            let driven = node(at(inputs.parameter(4), n, DRIVE_NODE), masses);
            let pickup = node(at(inputs.parameter(5), n, PICKUP_NODE), masses);
            if decay_s != self.decay_s {
                self.decay_s = decay_s;
                self.g = libm::exp(-3.0 * std::f64::consts::LN_10 / decay_s * h);
            }
            let w = std::f64::consts::TAU * spring;
            let w2 = w * w;
            let chain = (w2, free, masses);
            match self.chain {
                Some(old) if old != chain => self.change(old, chain, h),
                _ => {}
            }
            self.chain = Some(chain);
            let force = at(drive, n, 0.0);
            for j in 0..masses {
                let mut a = -w2 * self.stiffness(j, masses, free);
                if j == driven {
                    a += w2 * force;
                }
                if let Some(v) = self.v.get_mut(j) {
                    *v += h * a;
                }
            }
            let g = self.g;
            for (x, v) in self.x.iter_mut().zip(self.v.iter_mut()).take(masses) {
                *x = flush(g * (*x + h * *v));
                *v = flush(g * *v);
            }
            *out = self.x.get(pickup).copied().unwrap_or(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::string::measure::partial;
    use crate::testing::{Timeline, context, render};

    /// The pickup's displacement after a unit drive impulse on frame 0.
    fn ring(params: [f64; 6], drive: &[f64], sample_rate: f64) -> Vec<f64> {
        let frames = drive.len();
        let mut chain = MassSpring::default();
        chain.prepare(&context(sample_rate, 256, 0));
        let lanes: Vec<Vec<f64>> = params.iter().map(|&p| vec![p; frames]).collect();
        let lanes: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
        render(
            &mut chain,
            &Timeline {
                signals: &[drive],
                parameters: &lanes,
                triggers: &[],
            },
            frames,
            256,
        )
        .signals[0]
            .clone()
    }

    fn impulse(frames: usize) -> Vec<f64> {
        let mut x = vec![0.0; frames];
        x[0] = 1.0;
        x
    }

    /// AC2: for small M, each normal mode rings at the closed form, warped
    /// by the integrator as documented (within 1e-5), which is within 0.1 %
    /// of the continuous chain's `ω_n` at these rates; every mode's T60 is
    /// `decay_s`.
    #[test]
    fn normal_modes_match_the_closed_form() {
        let (sample_rate, spring, decay_s) = (48_000.0, 400.0, 4.0);
        let pi = std::f64::consts::PI;
        for (boundary, masses) in [(0.0, 2), (0.0, 3), (1.0, 3), (1.0, 4)] {
            let y = ring(
                [masses as f64, spring, decay_s, boundary, 1.0, 1.0],
                &impulse(96_000),
                sample_rate,
            );
            let h = 1.0 / sample_rate;
            for n in 1..=masses {
                let m = masses as f64;
                let angle = if boundary == 0.0 {
                    n as f64 * pi / (2.0 * (m + 1.0))
                } else {
                    (2.0 * n as f64 - 1.0) * pi / (2.0 * (2.0 * m + 1.0))
                };
                let omega = 2.0 * std::f64::consts::TAU * spring * libm::sin(angle);
                let cos = 1.0 - h * h * omega * omega / 2.0;
                let discrete = libm::acos(cos) * sample_rate / std::f64::consts::TAU;
                let continuous = omega / std::f64::consts::TAU;
                let (f, t60) = partial(&y, discrete, 4800, 52_800, 43_200, sample_rate);
                println!(
                    "boundary {boundary}, M {masses}, mode {n}: {f:.5} Hz, closed form \
                     {discrete:.5} Hz (continuous {continuous:.5}), T60 {t60:.4} s"
                );
                assert!((f / discrete - 1.0).abs() < 1e-5);
                assert!((f / continuous - 1.0).abs() < 1e-3);
                assert!((t60 / decay_s - 1.0).abs() < 0.01);
            }
        }
    }

    /// AC2: at the documented bound, and above it (clamped to it), the
    /// longest chain of each boundary with the slowest decay stays bounded:
    /// after a burst of full-scale noise the peak of every later second is
    /// no higher than the first second's.
    #[test]
    fn the_chain_is_stable_at_the_bound() {
        for sample_rate in [44_100.0, 192_000.0] {
            for spring in [sample_rate / std::f64::consts::TAU, 1e9] {
                for boundary in [0.0, 1.0] {
                    let frames = 5 * sample_rate as usize;
                    let second = sample_rate as usize;
                    let drive: Vec<f64> = (0..frames)
                        .map(|n| match n < 1000 {
                            true => libm::sin(n as f64 * 2.1) * libm::cos(n as f64 * 0.7),
                            false => 0.0,
                        })
                        .collect();
                    let y = ring(
                        [32.0, spring, 60.0, boundary, 7.0, 32.0],
                        &drive,
                        sample_rate,
                    );
                    let peak = |s: &[f64]| {
                        s.iter().fold(0.0f64, |p, &x| {
                            assert!(x.is_finite());
                            p.max(x.abs())
                        })
                    };
                    let first = peak(&y[..second]);
                    let peaks: Vec<f64> = y.chunks(second).map(peak).collect();
                    println!("{sample_rate} Hz, spring {spring}, boundary {boundary}: {peaks:?}");
                    assert!(first > 0.0);
                    assert!(peaks.iter().all(|&p| p <= first));
                }
            }
        }
    }

    /// A static drive settles at the static deflection: a single fixed–fixed
    /// mass between two springs holds `drive/2`.
    #[test]
    fn a_static_drive_settles_at_the_static_deflection() {
        let (sample_rate, decay_s) = (48_000.0, 0.05);
        let y = ring(
            [1.0, 50.0, decay_s, 0.0, 1.0, 1.0],
            &[1.0; 48_000],
            sample_rate,
        );
        let h = 1.0 / sample_rate;
        let g = libm::exp(-3.0 * std::f64::consts::LN_10 / decay_s * h);
        let w2 = (std::f64::consts::TAU * 50.0) * (std::f64::consts::TAU * 50.0);
        let expected = w2 / (2.0 * w2 + (1.0 - g) * (1.0 - g) / (g * h * h));
        println!(
            "static deflection {} (expected {expected}, undamped 0.5)",
            y[47_999]
        );
        approx::assert_abs_diff_eq!(y[47_999], expected, epsilon = 1e-9);
    }
}
