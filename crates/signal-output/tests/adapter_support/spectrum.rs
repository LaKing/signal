//! Converter measurements: the converter at a fixed ratio, a least-squares
//! sine fit (gain, phase and residual of a tone whose output frequency is
//! known exactly) and the residual's spectrum (a flat-top windowed real FFT,
//! `realfft`).

use std::f64::consts::TAU;

use realfft::RealFftPlanner;
use signal_output::adapter::{FilterSpec, ONE, Resampler};

/// Runs the converter at a fixed `step` (source frames per output frame,
/// rounded to 32.32 as the adapter does) from source position `start`, over
/// `input`, for `frames` output frames. Returns the output and the step it
/// used, exactly.
pub fn convert(
    spec: FilterSpec,
    input: &[f32],
    step: f64,
    start: f64,
    frames: usize,
) -> (Vec<f32>, f64) {
    let mut r = Resampler::new(spec, 1, 4096);
    let step_fixed = (step * ONE as f64).round() as u64;
    let mut position = (start * ONE as f64).round() as u64;
    let mut next = 0usize;
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        let need = r.needs_until(position);
        while signal_output::adapter::frames_between(need, r.written()) > 0 {
            let x = input.get(next).copied().unwrap_or(0.0);
            r.write(&[x]);
            next += 1;
        }
        let mut y = [0.0f32];
        r.read_add(position, 1.0, &mut y);
        out.push(y[0]);
        position = position.wrapping_add(step_fixed);
    }
    (out, step_fixed as f64 / ONE as f64)
}

/// A tone of `amplitude` at `cycles` per source frame, `len` frames, as
/// `f32` from `f64`.
pub fn tone(cycles: f64, amplitude: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| (amplitude * (TAU * cycles * n as f64).sin()) as f32)
        .collect()
}

/// A least-squares fit of `a·sin(ωn) + b·cos(ωn) + c` to `y` (n from 0).
#[derive(Clone, Debug)]
pub struct Fit {
    /// The fitted sine's amplitude.
    pub amplitude: f64,
    /// `y` minus the fit.
    pub residual: Vec<f64>,
}

impl Fit {
    /// The residual's RMS over the fitted sine's RMS, in dB (THD+N, or
    /// SINAD's negative, over the whole band).
    pub fn residual_db(&self) -> f64 {
        let rms =
            (self.residual.iter().map(|r| r * r).sum::<f64>() / self.residual.len() as f64).sqrt();
        20.0 * (rms / (self.amplitude / 2f64.sqrt())).log10()
    }
}

/// Fits a sine at `omega` radians per sample to `y`.
pub fn fit(y: &[f32], omega: f64) -> Fit {
    // Normal equations for [sin, cos, 1].
    let mut m = [[0.0f64; 3]; 3];
    let mut v = [0.0f64; 3];
    for (n, &yn) in y.iter().enumerate() {
        let (s, c) = (omega * n as f64).sin_cos();
        let basis = [s, c, 1.0];
        for i in 0..3 {
            v[i] += basis[i] * f64::from(yn);
            for j in 0..3 {
                m[i][j] += basis[i] * basis[j];
            }
        }
    }
    let x = solve3(m, v);
    let residual = y
        .iter()
        .enumerate()
        .map(|(n, &yn)| {
            let (s, c) = (omega * n as f64).sin_cos();
            f64::from(yn) - (x[0] * s + x[1] * c + x[2])
        })
        .collect();
    Fit {
        amplitude: (x[0] * x[0] + x[1] * x[1]).sqrt(),
        residual,
    }
}

/// Solves a 3×3 system by Cramer's rule.
fn solve3(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    let mut x = [0.0; 3];
    for (k, xk) in x.iter_mut().enumerate() {
        let mut a = m;
        for i in 0..3 {
            a[i][k] = v[i];
        }
        *xk = det(a) / d;
    }
    x
}

/// The amplitude spectrum of `x` under a 5-term flat-top window (sidelobes
/// −93 dB, scalloping < 0.01 dB): a sine of amplitude `a` shows as `a` at its
/// bin. Returns (frequency in cycles per sample, amplitude) per bin.
pub fn spectrum(x: &[f64]) -> Vec<(f64, f64)> {
    const A: [f64; 5] = [
        0.215_578_95,
        0.416_631_58,
        0.277_263_158,
        0.083_578_947,
        0.006_947_368,
    ];
    let n = x.len();
    let window = |i: usize| {
        let p = TAU * i as f64 / n as f64;
        A[0] - A[1] * p.cos() + A[2] * (2.0 * p).cos() - A[3] * (3.0 * p).cos()
            + A[4] * (4.0 * p).cos()
    };
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut input: Vec<f64> = x.iter().enumerate().map(|(i, &v)| v * window(i)).collect();
    let mut out = fft.make_output_vec();
    fft.process(&mut input, &mut out).unwrap();
    let scale = 2.0 / (A[0] * n as f64);
    out.iter()
        .enumerate()
        .map(|(b, c)| (b as f64 / n as f64, c.norm() * scale))
        .collect()
}

/// The largest component of `x`'s spectrum: (cycles per sample, amplitude).
pub fn largest(x: &[f64]) -> (f64, f64) {
    spectrum(x).into_iter().skip(2).fold(
        (0.0, 0.0),
        |best, bin| if bin.1 > best.1 { bin } else { best },
    )
}

/// 20·log10.
pub fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}
