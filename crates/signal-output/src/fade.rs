//! The device fade (ADR 0036 §4): output policy of a device that starts or
//! stops, never a semantic of the project. A device driver asks the
//! [`Runner`](crate::Runner) to ramp its output in when its stream starts
//! ([`Runner::fade_in`](crate::Runner::fade_in)) and out before a deliberate
//! stop ([`Runner::fade_out`](crate::Runner::fade_out)), so neither edge is a
//! step. Raised-cosine gains over [`FADE_SECONDS`]. Offline renders never
//! fade, and neither does a Runner nobody asks (the null driver's): their
//! output stays bit-identical.
//!
//! [`Fade::apply`] allocates, locks and blocks nowhere.

use std::f64::consts::PI;

/// How long a fade lasts: 10 ms.
pub const FADE_SECONDS: f64 = 0.010;

/// The frames of a [`FADE_SECONDS`] fade at `sample_rate` (at least 1).
pub fn fade_frames(sample_rate: u32) -> u32 {
    ((f64::from(sample_rate) * FADE_SECONDS).round() as u32).max(1)
}

/// Where a fade is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Phase {
    /// Full level: samples pass untouched.
    #[default]
    Steady,
    /// Rising: frame `at` of `len`.
    In { at: u32, len: u32 },
    /// Falling: frame `at` of `len`.
    Out { at: u32, len: u32 },
    /// Faded out: silence.
    Silent,
}

/// A fade over interleaved frames: steady (untouched), rising, falling or
/// silent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fade {
    phase: Phase,
}

/// The raised-cosine rise at frame `at` of `len`: 0 at 0, 1 at `len`.
fn rise(at: u32, len: u32) -> f32 {
    let x = f64::from(at) / f64::from(len.max(1));
    (0.5 - 0.5 * (PI * x.min(1.0)).cos()) as f32
}

impl Fade {
    /// Ramps in over `len` frames from silence (0 frames: at once).
    pub fn start_in(&mut self, len: u32) {
        self.phase = if len == 0 {
            Phase::Steady
        } else {
            Phase::In { at: 0, len }
        };
    }

    /// Ramps out over `len` frames, from the level it is at (0 frames:
    /// silent at once). A fade already falling or silent goes on.
    pub fn start_out(&mut self, len: u32) {
        self.phase = match self.phase {
            _ if len == 0 => Phase::Silent,
            Phase::Steady => Phase::Out { at: 0, len },
            // From the level reached: rise(at, l) = rise(len − at', len)
            // with at' = len − at·len/l.
            Phase::In { at, len: l } => {
                let level = u64::from(at) * u64::from(len) / u64::from(l.max(1));
                Phase::Out {
                    at: len.saturating_sub(level as u32),
                    len,
                }
            }
            falling @ (Phase::Out { .. } | Phase::Silent) => falling,
        };
    }

    /// Whether it has faded out (silence from here on).
    pub fn is_silent(&self) -> bool {
        self.phase == Phase::Silent
    }

    /// Whether samples pass untouched.
    pub fn is_steady(&self) -> bool {
        self.phase == Phase::Steady
    }

    /// Applies the fade to `out`, frames of `channels` interleaved samples,
    /// advancing it by each frame. Steady: `out` is untouched.
    pub fn apply(&mut self, out: &mut [f32], channels: usize) {
        if self.phase == Phase::Steady || channels == 0 {
            return;
        }
        for frame in out.chunks_mut(channels) {
            let gain = match &mut self.phase {
                Phase::Steady => break,
                Phase::Silent => 0.0,
                Phase::In { at, len } => {
                    let gain = rise(*at, *len);
                    *at += 1;
                    if *at >= *len {
                        self.phase = Phase::Steady;
                    }
                    gain
                }
                Phase::Out { at, len } => {
                    let gain = rise(len.saturating_sub(*at), *len);
                    *at += 1;
                    if *at >= *len {
                        self.phase = Phase::Silent;
                    }
                    gain
                }
            };
            for sample in frame {
                *sample *= gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_milliseconds() {
        assert_eq!(fade_frames(48_000), 480);
        assert_eq!(fade_frames(44_100), 441);
        assert_eq!(fade_frames(0), 1);
    }

    #[test]
    fn a_steady_fade_touches_nothing() {
        let mut fade = Fade::default();
        let mut out = vec![0.3f32, -0.7, f32::MIN_POSITIVE, 1.0];
        let before = out.clone();
        fade.apply(&mut out, 2);
        assert_eq!(out, before);
    }

    /// In: from 0, rising, then the input as it is; out: from the level,
    /// falling, then silence.
    #[test]
    fn in_rises_from_silence_and_out_falls_to_it() {
        let mut fade = Fade::default();
        fade.start_in(4);
        let mut out = vec![1.0f32; 2 * 6];
        fade.apply(&mut out, 2);
        let gains: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        assert_eq!(gains[0], 0.0);
        assert!(gains.windows(2).all(|w| w[0] < w[1] || w[1] == 1.0));
        assert_eq!(&gains[4..], &[1.0, 1.0]);
        assert!(out.chunks(2).all(|f| f[0] == f[1]), "every channel alike");
        assert!(fade.is_steady());

        fade.start_out(4);
        let mut out = vec![1.0f32; 6];
        fade.apply(&mut out, 1);
        assert_eq!(out[0], 1.0);
        assert!(out[..4].windows(2).all(|w| w[0] > w[1]));
        assert_eq!(&out[4..], &[0.0, 0.0]);
        assert!(fade.is_silent());
    }

    /// A stop during the rise falls from the level reached, never jumps up.
    #[test]
    fn out_during_in_starts_at_the_level_reached() {
        let mut fade = Fade::default();
        fade.start_in(480);
        let mut out = vec![1.0f32; 120];
        fade.apply(&mut out, 1);
        let reached = out[119];
        fade.start_out(480);
        let mut next = vec![1.0f32; 480];
        fade.apply(&mut next, 1);
        assert!(next[0] <= reached + 0.01, "{} after {reached}", next[0]);
        assert_eq!(*next.last().unwrap(), 0.0);
        assert!(fade.is_silent());
    }

    /// The largest step a 10 ms fade at 48 kHz makes on a full-scale input
    /// is below 0.004 (a step of 1 without it).
    #[test]
    fn the_steps_are_small() {
        let mut fade = Fade::default();
        fade.start_in(fade_frames(48_000));
        let mut out = vec![1.0f32; 600];
        fade.apply(&mut out, 1);
        let step = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(out[0] == 0.0 && step < 0.004, "{step}");
    }
}
