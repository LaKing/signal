//! The drift loop (ADR 0052 §2): a PI controller on the fill error that
//! steers the converter's ratio, ratio = nominal × (1 + correction).
//!
//! - The error (filled frames minus the target, in source frames) is
//!   low-passed by one pole (τ, a constant per mode).
//! - correction = Kp·e + ∫Ki·e dt with Kp = 2ζω/fₛ and Ki = ω²/fₛ: the fill
//!   integrates the rate difference, so both poles sit at ω. The integral
//!   term is kept as its contribution to the correction, so a change of ω
//!   moves nothing (bumpless).
//! - **Acquire** at ω_acquire until |e| ≤ the lock window for the lock
//!   time with the fill steady over it (moving at most `lock_rate` frames
//!   per second, about 1 ppm: phase 1 added this, as a loop whose error
//!   never left the window would otherwise enter track mid-transient),
//!   both judged on the error low-passed at the track τ; then **track** at
//!   ω_track; the switch adds the dropped proportional part to the
//!   integral term, so the correction does not step.
//! - The correction is clamped (±1000 ppm) and slewed (ppm per second, a
//!   limit per mode); the integral term is clamped too and stops
//!   integrating outward while the correction is clamped (anti-windup).
//!
//! All `f64`, no allocation: [`DriftLoop::update`] runs once per callback
//! on the converting side.

use std::f64::consts::PI;

/// One part per million.
pub const PPM: f64 = 1e-6;

/// The loop's constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopParams {
    /// The fill error's low-pass time constant in track, seconds.
    pub tau: f64,
    /// The low-pass time constant in acquire, seconds.
    pub acquire_tau: f64,
    /// Damping ζ.
    pub zeta: f64,
    /// Acquire bandwidth, Hz (ω = 2π × this).
    pub acquire_hz: f64,
    /// Track bandwidth, Hz.
    pub track_hz: f64,
    /// |e| at most this many frames counts as locked.
    pub lock_frames: f64,
    /// Locked this long (seconds) switches acquire to track.
    pub lock_seconds: f64,
    /// ... if the fill moved at most this many frames per second over it
    /// (0.05 frames/s is about 1 ppm at 48 kHz).
    pub lock_rate: f64,
    /// The correction's clamp (a fraction: 1000 ppm is 1e-3).
    pub clamp: f64,
    /// The slew limit in acquire, per second (a fraction).
    pub acquire_slew: f64,
    /// The slew limit in track, per second.
    pub track_slew: f64,
}

impl Default for LoopParams {
    /// ADR 0052 §2's design, with the slew limit split by phase 1's
    /// measurements (`docs/benchmarks.md` "Clock adapter"): the ADR's
    /// 20 ppm/s in acquire locks a 100 ppm device only after about 30 s and
    /// cycles at 300 ppm, so it holds in track only, and acquire may move
    /// 400 ppm/s; and τ = 0.5 s, whose pole at 2 rad/s leaves the acquire
    /// loop (ω = 1.26 rad/s) damped at about 0.24, holds in track only, while
    /// acquire low-passes at 0.1 s (damping about 0.7).
    fn default() -> LoopParams {
        LoopParams {
            tau: 0.5,
            acquire_tau: 0.1,
            zeta: 0.7,
            acquire_hz: 0.2,
            track_hz: 0.02,
            lock_frames: 2.0,
            lock_seconds: 2.0,
            lock_rate: 0.05,
            clamp: 1000.0 * PPM,
            acquire_slew: 400.0 * PPM,
            track_slew: 20.0 * PPM,
        }
    }
}

/// Acquire or track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    /// Fast, until locked.
    Acquire,
    /// Slow, locked.
    Track,
}

/// The PI loop's state.
#[derive(Clone, Debug)]
pub struct DriftLoop {
    params: LoopParams,
    /// The source rate, frames per second (fₛ of Kp and Ki).
    rate: f64,
    mode: LoopMode,
    /// The low-passed error, frames (the mode's τ).
    filtered: f64,
    /// The error low-passed at the track τ in every mode: the lock test's.
    slow: f64,
    /// Seconds the error has stayed in the lock window.
    locked_for: f64,
    /// `slow` when the lock window's count began.
    window_start: f64,
    /// The integral term's contribution to the correction.
    integral: f64,
    /// The correction the slew limit moves towards.
    target: f64,
    /// The correction in use.
    correction: f64,
}

impl DriftLoop {
    /// A loop in acquire at correction 0, for fills in frames of a source at
    /// `rate` frames per second.
    pub fn new(params: LoopParams, rate: f64) -> DriftLoop {
        DriftLoop {
            params,
            rate: rate.max(1.0),
            mode: LoopMode::Acquire,
            filtered: 0.0,
            slow: 0.0,
            locked_for: 0.0,
            window_start: 0.0,
            integral: 0.0,
            target: 0.0,
            correction: 0.0,
        }
    }

    /// The constants.
    pub fn params(&self) -> &LoopParams {
        &self.params
    }

    /// Back to acquire, the filter at `error`, keeping the integral term
    /// (and so the correction) when `keep` is set, else all at 0.
    pub fn restart(&mut self, error: f64, keep: bool) {
        self.mode = LoopMode::Acquire;
        self.filtered = error;
        self.slow = error;
        self.locked_for = 0.0;
        self.window_start = error;
        if !keep {
            self.integral = 0.0;
            self.target = 0.0;
            self.correction = 0.0;
        }
    }

    /// Moves the filtered error by `frames` (a splice moved the fill: the
    /// loop does not see it as drift).
    pub fn shift(&mut self, frames: f64) {
        self.filtered += frames;
        self.slow += frames;
        self.window_start += frames;
    }

    /// Kp and Ki of a mode.
    fn gains(&self, mode: LoopMode) -> (f64, f64) {
        let hz = match mode {
            LoopMode::Acquire => self.params.acquire_hz,
            LoopMode::Track => self.params.track_hz,
        };
        let omega = 2.0 * PI * hz;
        (
            2.0 * self.params.zeta * omega / self.rate,
            omega * omega / self.rate,
        )
    }

    /// One step: `error` is this callback's fill minus the target (frames),
    /// `dt` the seconds since the last step. Returns the correction.
    pub fn update(&mut self, error: f64, dt: f64) -> f64 {
        let dt = dt.max(0.0);
        let p = &self.params;
        let tau = match self.mode {
            LoopMode::Acquire => p.acquire_tau,
            LoopMode::Track => p.tau,
        };
        let alpha = |tau: f64| {
            if tau > 0.0 {
                1.0 - (-dt / tau).exp()
            } else {
                1.0
            }
        };
        self.filtered += alpha(tau) * (error - self.filtered);
        self.slow += alpha(p.tau) * (error - self.slow);
        let e = self.filtered;
        let (kp, ki) = self.gains(self.mode);
        let clamp = p.clamp;
        let step = ki * e * dt;
        let slew = match self.mode {
            LoopMode::Acquire => p.acquire_slew,
            LoopMode::Track => p.track_slew,
        } * dt;
        // Anti-windup: no integration away from the correction in use while
        // the clamp or the slew limit holds it back.
        let unclamped = kp * e + self.integral + step;
        let ahead = unclamped - self.correction;
        let held = unclamped.abs() >= clamp || ahead.abs() > slew;
        if !(held && step * ahead > 0.0) {
            self.integral = (self.integral + step).clamp(-clamp, clamp);
        }
        self.target = (kp * e + self.integral).clamp(-clamp, clamp);
        self.correction += (self.target - self.correction).clamp(-slew, slew);

        if self.mode == LoopMode::Acquire {
            // Locked: |e| in the window for the lock time, and the fill
            // steady over it (the rate difference within lock_rate), so
            // track starts from a settled integral.
            if self.slow.abs() <= p.lock_frames {
                if self.locked_for == 0.0 {
                    self.window_start = self.slow;
                }
                self.locked_for += dt;
            } else {
                self.locked_for = 0.0;
            }
            if self.locked_for >= p.lock_seconds {
                let moved = (self.slow - self.window_start).abs();
                if moved <= p.lock_rate * self.locked_for {
                    // Bumpless: the integral takes what the proportional
                    // part loses, the track filter starting from `slow`.
                    let (kp_track, _) = self.gains(LoopMode::Track);
                    self.integral =
                        (self.integral + kp * e - kp_track * self.slow).clamp(-clamp, clamp);
                    self.filtered = self.slow;
                    self.mode = LoopMode::Track;
                } else {
                    self.locked_for = 0.0;
                }
            }
        }
        self.correction
    }

    /// Acquire or track.
    pub fn mode(&self) -> LoopMode {
        self.mode
    }

    /// The correction in use (a fraction; × 1e6 for ppm).
    pub fn correction(&self) -> f64 {
        self.correction
    }

    /// The low-passed fill error, frames.
    pub fn filtered_error(&self) -> f64 {
        self.filtered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A constant drift: the loop's correction converges to it and the
    /// error to 0 (the integrator), and it locks into track.
    #[test]
    fn converges_on_a_constant_drift() {
        let rate = 48_000.0;
        let mut drift = DriftLoop::new(LoopParams::default(), rate);
        let epsilon = 100.0 * PPM;
        let dt = 128.0 / rate;
        let mut fill_error = 0.0;
        for _ in 0..(120.0 / dt) as usize {
            let c = drift.update(fill_error, dt);
            fill_error += rate * (epsilon - c) * dt;
        }
        assert_eq!(drift.mode(), LoopMode::Track);
        assert!((drift.correction() - epsilon).abs() < 0.01 * PPM);
        assert!(fill_error.abs() < 0.05, "{fill_error}");
    }

    /// The clamp holds and the integral does not wind up beyond it.
    #[test]
    fn clamped_at_1000_ppm() {
        let mut drift = DriftLoop::new(LoopParams::default(), 48_000.0);
        for _ in 0..100_000 {
            drift.update(1e6, 0.01);
        }
        assert!((drift.correction() - 1000.0 * PPM).abs() < 1e-12);
        // Recovers at once when the error turns: the integral is at the
        // clamp, not beyond it.
        let before = drift.correction();
        for _ in 0..10 {
            drift.update(-1e6, 0.01);
        }
        assert!(drift.correction() < before);
    }

    /// The switch to track does not step the correction target: the
    /// target the last acquire step set is the first track target.
    #[test]
    fn bumpless_switch() {
        let mut drift = DriftLoop::new(LoopParams::default(), 48_000.0);
        let mut switched = false;
        for _ in 0..10_000 {
            let mode = drift.mode();
            drift.update(1.5, 0.01);
            if mode == LoopMode::Acquire && drift.mode() == LoopMode::Track {
                switched = true;
                break;
            }
        }
        assert!(switched);
        let before = drift.target;
        drift.update(1.5, 0.0);
        assert!(
            (drift.target - before).abs() < 1e-12,
            "{} {before}",
            drift.target
        );
    }
}
