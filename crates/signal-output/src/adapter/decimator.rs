//! Staged conversion (review 2026-11-06, finding 4): before the converter,
//! halve the source rate as often as the converter's filter would
//! otherwise need more than [`MAX_TAPS`](super::MAX_TAPS) to reach ADR
//! 0052 §4's targets. 96 → 44.1 kHz converts 48 → 44.1 after one halving,
//! 192 → 44.1 kHz after two; at most [`MAX_HALVINGS`].
//!
//! Each halving is a Kaiser-windowed sinc low-pass of odd length (linear
//! phase, a whole frame's delay) computed at every second input frame. It
//! passes the converter's passband and stops, from `R/2 − L/2` (`R` its
//! input rate, `L` the lower rate of the conversion), everything that would
//! fold into `0 … L/2`: what folds above `L/2` the later stages and the
//! converter stop. Designed for [`STAGE_ATTENUATION_DB`], a margin over
//! the converter's 105 dB for the cascade.
//!
//! It runs inside [`AdapterInput::push`](super::AdapterInput::push), on the
//! producing side: the ring, the marks and everything after them count
//! frames at the halved rate. Real-time safe: its delay lines are
//! allocated when it is made; a push's work is `taps × channels`
//! multiply-adds per stage output.

use std::f64::consts::PI;

use super::sinc::{FilterSpec, bessel_i0, kaiser_beta};

/// The most halvings: 384 kHz to 48 kHz.
pub const MAX_HALVINGS: u32 = 3;

/// A stage's stopband attenuation, in dB.
pub const STAGE_ATTENUATION_DB: f64 = 110.0;

/// The most taps a stage may use.
pub const MAX_STAGE_TAPS: usize = 511;

/// The halvings before converting `source_rate` to `dest_rate`, and the
/// rate the converter then reads; `None` when no staging meets the targets
/// (the converter's filter would still need more than its taps, or a stage
/// more than [`MAX_STAGE_TAPS`]).
pub fn staging(source_rate: u32, dest_rate: u32) -> Option<(u32, u32)> {
    let mut rate = source_rate;
    let mut halvings = 0;
    while !FilterSpec::for_rates(rate, dest_rate).reaches_design() {
        if halvings == MAX_HALVINGS || !rate.is_multiple_of(2) || rate / 2 < dest_rate {
            return None;
        }
        if stage_taps(rate, dest_rate) > MAX_STAGE_TAPS {
            return None;
        }
        rate /= 2;
        halvings += 1;
    }
    Some((halvings, rate))
}

/// The passband edge of a conversion whose lower rate is `low`, in Hz (the
/// converter's).
fn pass_hz(low: u32) -> f64 {
    let low = f64::from(low.max(1));
    20_000.0f64.min(low * 20_000.0 / 44_100.0)
}

/// A halving stage at input rate `rate` for a conversion whose lower rate
/// is `low`: (passband, stopband) in cycles per input frame.
fn stage_band(rate: u32, low: u32) -> (f64, f64) {
    let r = f64::from(rate.max(1));
    (pass_hz(low) / r, 0.5 - f64::from(low) / (2.0 * r))
}

/// A stage's taps by Kaiser's formula: odd, at least 3.
fn stage_taps(rate: u32, low: u32) -> usize {
    let (pass, stop) = stage_band(rate, low);
    let transition = 2.0 * PI * (stop - pass);
    if transition <= 0.0 {
        return usize::MAX;
    }
    let n = ((STAGE_ATTENUATION_DB - 8.0) / (2.285 * transition)).ceil() as usize + 1;
    (n | 1).max(3)
}

/// Lanes of a stage's dot product: its taps are padded with zeros to a
/// multiple, so the sum runs in independent lanes (vectorised).
const LANES: usize = 8;

/// One halving: the filter and, per channel, a mirrored delay line.
#[derive(Debug)]
struct Stage {
    /// Its taps (odd), then zeros to a multiple of [`LANES`].
    taps: Vec<f32>,
    /// Its length, the odd count.
    len: usize,
    /// `channels` lines of `len + taps.len()` samples: each sample at `at`
    /// and at `at + len`, so the last `len` (and the padding after them)
    /// are one slice.
    lines: Vec<f32>,
    at: usize,
    /// Whether the next input frame is an output's.
    emit: bool,
}

impl Stage {
    fn new(rate: u32, low: u32, channels: usize) -> Stage {
        let n = stage_taps(rate, low).min(MAX_STAGE_TAPS);
        let (pass, stop) = stage_band(rate, low);
        let cutoff = (pass + stop) / 2.0;
        let predicted = 8.0 + 2.285 * 2.0 * PI * (stop - pass) * (n as f64 - 1.0);
        let beta = kaiser_beta(predicted.min(STAGE_ATTENUATION_DB + 10.0));
        let half = (n / 2) as f64;
        let mut taps: Vec<f64> = (0..n)
            .map(|k| {
                let t = k as f64 - half;
                let x = 2.0 * cutoff * t;
                let sinc = if x == 0.0 {
                    1.0
                } else {
                    (PI * x).sin() / (PI * x)
                };
                let r = t / half;
                sinc * bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta)
            })
            .collect();
        let sum: f64 = taps.iter().sum();
        taps.iter_mut().for_each(|c| *c /= sum);
        let padded = n.div_ceil(LANES) * LANES;
        taps.resize(padded, 0.0);
        Stage {
            taps: taps.into_iter().map(|c| c as f32).collect(),
            len: n,
            lines: vec![0.0; channels * (n + padded)],
            at: 0,
            emit: false,
        }
    }

    /// Takes one input frame; writes an output frame into `out` and
    /// returns true every second one.
    fn step(&mut self, frame: &[f32], out: &mut [f32]) -> bool {
        let n = self.len;
        let padded = self.taps.len();
        let at = self.at;
        for (line, &x) in self.lines.chunks_exact_mut(n + padded).zip(frame) {
            if let Some(slot) = line.get_mut(at) {
                *slot = x;
            }
            if let Some(slot) = line.get_mut(at + n) {
                *slot = x;
            }
        }
        self.at = if at + 1 == n { 0 } else { at + 1 };
        self.emit = !self.emit;
        if !self.emit {
            return false;
        }
        // The last n samples, oldest first: from at + 1 (mod n) on; the
        // padding meets zero taps.
        let from = self.at;
        for (line, o) in self.lines.chunks_exact(n + padded).zip(out.iter_mut()) {
            let Some(window) = line.get(from..from + padded) else {
                continue;
            };
            let mut lanes = [0.0f32; LANES];
            for (x, c) in window
                .as_chunks::<LANES>()
                .0
                .iter()
                .zip(self.taps.as_chunks::<LANES>().0)
            {
                for ((l, &x), &c) in lanes.iter_mut().zip(x).zip(c) {
                    *l += x * c;
                }
            }
            *o = lanes.iter().sum();
        }
        true
    }
}

/// The halvings of a conversion, in order.
#[derive(Debug)]
pub struct Decimator {
    stages: Vec<Stage>,
    channels: usize,
    /// Each stage's output frame, `channels` per stage.
    frames: Vec<f32>,
    rate_out: u32,
    delay: f64,
}

impl Decimator {
    /// The stages [`staging`] gives for `source_rate` → `dest_rate`
    /// (none when the converter alone meets the targets); `None` when no
    /// staging does. Control thread: allocates and designs.
    pub fn for_rates(source_rate: u32, dest_rate: u32, channels: usize) -> Option<Decimator> {
        let (halvings, rate_out) = staging(source_rate, dest_rate)?;
        let channels = channels.max(1);
        let low = source_rate.min(dest_rate);
        let mut stages = Vec::new();
        let mut delay = 0.0;
        let mut rate = source_rate;
        for i in 0..halvings {
            let stage = Stage::new(rate, low, channels);
            delay += (stage.len / 2) as f64 * f64::from(1u32 << i);
            stages.push(stage);
            rate /= 2;
        }
        Some(Decimator {
            frames: vec![0.0; channels * stages.len()],
            stages,
            channels,
            rate_out,
            delay,
        })
    }

    /// The halvings.
    pub fn halvings(&self) -> u32 {
        self.stages.len() as u32
    }

    /// The rate it delivers.
    pub fn rate_out(&self) -> u32 {
        self.rate_out
    }

    /// Each stage's taps.
    pub fn taps(&self) -> Vec<usize> {
        self.stages.iter().map(|s| s.len).collect()
    }

    /// Its delay in input frames: each stage's half length at its rate.
    pub fn delay_frames(&self) -> f64 {
        self.delay
    }

    /// Multiply-adds per input frame: each stage's taps × channels, at
    /// half its input's rate.
    pub fn multiply_adds_per_frame(&self) -> f64 {
        self.stages
            .iter()
            .enumerate()
            .map(|(i, s)| (s.len * self.channels) as f64 / f64::from(2u32 << i))
            .sum()
    }

    /// Takes interleaved `input` (a partial last frame is ignored) and
    /// writes the frames it delivers into `out`, as many as fit; returns
    /// them. With no stage it copies. Real-time safe.
    pub fn process(&mut self, input: &[f32], out: &mut [f32]) -> usize {
        let channels = self.channels;
        let mut written = 0;
        let mut slots = out.chunks_exact_mut(channels);
        for frame in input.chunks_exact(channels) {
            if let Some(last) = self.feed(frame) {
                let Some(slot) = slots.next() else {
                    break;
                };
                slot.copy_from_slice(last);
                written += 1;
            }
        }
        written
    }

    /// One input frame through every stage; the last stage's frame when it
    /// delivers one.
    fn feed<'a>(&'a mut self, frame: &'a [f32]) -> Option<&'a [f32]> {
        let channels = self.channels;
        let mut carry: Option<usize> = None;
        for (i, stage) in self.stages.iter_mut().enumerate() {
            let (before, rest) = self.frames.split_at_mut(i * channels);
            let out = rest.get_mut(..channels)?;
            let input = match carry {
                None => frame,
                Some(j) => before.get(j * channels..(j + 1) * channels)?,
            };
            if !stage.step(input, out) {
                return None;
            }
            carry = Some(i);
        }
        match carry {
            None => Some(frame),
            Some(j) => self.frames.get(j * channels..(j + 1) * channels),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The halvings for the rates devices use: only where the converter
    /// alone would fall short.
    #[test]
    fn halvings_where_the_converter_falls_short() {
        for (source, dest, halvings, rate) in [
            (48_000, 48_000, 0, 48_000),
            (48_000, 44_100, 0, 48_000),
            (44_100, 192_000, 0, 44_100),
            (96_000, 48_000, 0, 96_000),
            (88_200, 48_000, 0, 88_200),
            (96_000, 44_100, 1, 48_000),
            (88_200, 44_100, 1, 44_100),
            (192_000, 48_000, 1, 96_000),
            (176_400, 48_000, 1, 88_200),
            (192_000, 44_100, 2, 48_000),
            (176_400, 44_100, 2, 44_100),
            (384_000, 48_000, 2, 96_000),
        ] {
            assert_eq!(
                staging(source, dest),
                Some((halvings, rate)),
                "{source} → {dest}"
            );
        }
        // Halving 88 kHz would go below 44.1: nothing meets the targets,
        // and the adapter refuses the route.
        assert_eq!(staging(88_000, 44_100), None);
        let config = super::super::AdapterConfig::new(2, 88_000, 44_100, 256, 128);
        assert!(matches!(
            super::super::clock_adapter(config),
            Err(super::super::AdapterError::Ratio)
        ));
    }

    /// A push split anywhere delivers what one push does.
    #[test]
    fn any_split_delivers_the_same() {
        let input: Vec<f32> = (0..3_000 * 2).map(|n| (n as f32 * 0.37).sin()).collect();
        let whole = {
            let mut d = Decimator::for_rates(192_000, 44_100, 2).unwrap();
            let mut out = vec![0.0; input.len()];
            let made = d.process(&input, &mut out);
            out.truncate(made * 2);
            out
        };
        assert_eq!(whole.len(), 750 * 2);
        let mut d = Decimator::for_rates(192_000, 44_100, 2).unwrap();
        let mut parts = Vec::new();
        let mut at = 0;
        for frames in [1, 3, 517, 2, 1_000].iter().cycle() {
            let end = (at + frames * 2).min(input.len());
            let mut out = vec![0.0; end - at + 2];
            let made = d.process(&input[at..end], &mut out);
            parts.extend_from_slice(&out[..made * 2]);
            at = end;
            if at == input.len() {
                break;
            }
        }
        assert_eq!(parts, whole);
    }

    /// DC passes at unity.
    #[test]
    fn dc_passes() {
        let mut d = Decimator::for_rates(176_400, 44_100, 1).unwrap();
        let input = vec![0.5f32; 4_000];
        let mut out = vec![0.0; 1_000];
        let made = d.process(&input, &mut out);
        assert_eq!(made, 1_000);
        assert!((out[999] - 0.5).abs() < 1e-5, "{}", out[999]);
    }
}
