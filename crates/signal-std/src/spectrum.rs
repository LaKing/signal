//! `std.spectrum512` and `std.spectrum2048` (ADR 0055 §1): a windowed
//! magnitude spectrum of the input, as a vector of *N*/2 bins, with its
//! strongest partial.
//!
//! **Ports**
//! - `in` (signal in, audio).
//! - `mag` (vector out, *N*/2 bins, linear): bin *b* is centred on
//!   *b*·*f*s/*N*; a sine of amplitude *A* centred on a bin reads *A* there,
//!   bin 0 a constant's value.
//! - `peak_hz` (signal out, Hz) and `peak` (signal out, linear): the largest
//!   bin among 1 … *N*/2 − 2, refined by Grandke's interpolation for a Hann
//!   window (exact for one sine); both 0 while every bin is below 1e−9.
//!
//! **Timing:** at divisor 1 (a spectrum needs every sample). Every *N*/4
//! frames counted from its reset it analyses the last *N* inputs (zeros
//! before the first), on the frame that completes them; between analyses the
//! outputs hold. Frames are counted, never timed, so the output does not
//! depend on how they are split into calls.
//!
//! **Arithmetic:** a periodic Hann window and a radix-2 complex FFT of our
//! own in f64, its window, twiddles and bit reversal made in `prepare` with
//! `libm`: bit-exact on any machine (a run-time SIMD choice, as `realfft`'s,
//! would not be). `process` allocates nothing.

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PortShape, PrepareContext,
};

use crate::{port_at, signal_port};

/// Every bin below this reads as silence: no peak.
const SILENCE: f64 = 1e-9;

/// The analysis state of a spectrum of `N` frames.
#[derive(Clone, Debug, Default)]
pub struct Spectrum<const N: usize> {
    sample_rate: f64,
    /// The last `N` inputs, `next` the oldest.
    history: Vec<f64>,
    next: usize,
    /// Frames since reset, modulo the hop.
    count: usize,
    window: Vec<f64>,
    window_sum: f64,
    /// `(cos, −sin)` of 2π·k/N, k < N/2.
    twiddles: Vec<(f64, f64)>,
    /// The bit-reversed index of each index.
    reversed: Vec<usize>,
    /// The FFT's working buffer, `(re, im)`.
    work: Vec<(f64, f64)>,
    /// The held outputs: `N/2` bins, then `peak_hz`, then `peak`.
    held: Vec<f64>,
}

impl<const N: usize> Spectrum<N> {
    const HOP: usize = N / 4;
    const BINS: usize = N / 2;

    /// Analyses the history into `held`.
    fn analyse(&mut self) {
        let (history, next) = (&self.history, self.next);
        let ordered = history.iter().skip(next).chain(history.iter().take(next));
        for ((slot, &x), &w) in self.work.iter_mut().zip(ordered).zip(&self.window) {
            *slot = (x * w, 0.0);
        }
        fft(&mut self.work, &self.reversed, &self.twiddles);
        let scale = if self.window_sum > 0.0 {
            1.0 / self.window_sum
        } else {
            0.0
        };
        for (b, (bin, &(re, im))) in self.held.iter_mut().zip(&self.work).enumerate() {
            let magnitude = re.hypot(im) * scale;
            *bin = if b == 0 { magnitude } else { 2.0 * magnitude };
        }
        let bins = self.held.get(..Self::BINS).unwrap_or_default();
        let (peak_hz, peak) = peak(bins, self.sample_rate / N as f64);
        if let Some(out) = self.held.get_mut(Self::BINS) {
            *out = peak_hz;
        }
        if let Some(out) = self.held.get_mut(Self::BINS + 1) {
            *out = peak;
        }
    }
}

/// The strongest partial of `bins` (spaced `spacing` Hz): the largest among
/// 1 … len − 2, refined by Grandke's interpolation for a Hann window (the
/// offset from the larger neighbour's ratio, the height divided by the
/// window's response there); `(0, 0)` when every bin is below [`SILENCE`].
fn peak(bins: &[f64], spacing: f64) -> (f64, f64) {
    let inner = bins.len().saturating_sub(1);
    let Some((m, &top)) = bins.iter().enumerate().take(inner).skip(1).fold(
        None,
        |best: Option<(usize, &f64)>, (b, v)| match best {
            Some((_, w)) if *w >= *v => best,
            _ => Some((b, v)),
        },
    ) else {
        return (0.0, 0.0);
    };
    if top < SILENCE {
        return (0.0, 0.0);
    }
    // Grandke's interpolation for a Hann window, exact for one sine: the
    // larger neighbour sets the side, its ratio to the peak the offset.
    let bin = |b: usize| bins.get(b).copied().unwrap_or(0.0);
    let (left, right) = (bin(m - 1), bin(m + 1));
    let offset = if right > left {
        (2.0 * right - top) / (top + right)
    } else {
        -(2.0 * left - top) / (top + left)
    }
    .clamp(-0.5, 0.5);
    // The window's response at that offset, relative to its centre:
    // sinc(δ) / (1 − δ²).
    let response = if offset == 0.0 {
        1.0
    } else {
        let x = core::f64::consts::PI * offset;
        libm::sin(x) / x / (1.0 - offset * offset)
    };
    ((m as f64 + offset) * spacing, top / response)
}

/// An in-place radix-2 complex FFT of `data` (its length a power of two,
/// `reversed` its bit reversal, `twiddles` `(cos, −sin)` of 2π·k/len).
fn fft(data: &mut [(f64, f64)], reversed: &[usize], twiddles: &[(f64, f64)]) {
    let n = data.len();
    for (i, &j) in reversed.iter().enumerate() {
        if i < j && j < n {
            data.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let step = n / len;
        for chunk in data.chunks_exact_mut(len) {
            let (low, high) = chunk.split_at_mut(len / 2);
            for (k, (a, b)) in low.iter_mut().zip(high.iter_mut()).enumerate() {
                let (wr, wi) = twiddles.get(k * step).copied().unwrap_or((1.0, 0.0));
                let t = (b.0 * wr - b.1 * wi, b.0 * wi + b.1 * wr);
                *b = (a.0 - t.0, a.1 - t.1);
                *a = (a.0 + t.0, a.1 + t.1);
            }
        }
        len *= 2;
    }
}

impl<const N: usize> Spectrum<N> {
    fn descriptor() -> ModuleDescriptor {
        match N {
            512 => DESCRIPTOR_512,
            _ => DESCRIPTOR_2048,
        }
    }
}

impl<const N: usize> DirectPrimitive for Spectrum<N> {
    fn describe() -> ModuleDescriptor {
        Self::descriptor()
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.history = vec![0.0; N];
        self.work = vec![(0.0, 0.0); N];
        self.held = vec![0.0; Self::BINS + 2];
        let tau = 2.0 * core::f64::consts::PI;
        self.window = (0..N)
            .map(|i| 0.5 - 0.5 * libm::cos(tau * i as f64 / N as f64))
            .collect();
        self.window_sum = self.window.iter().sum();
        self.twiddles = (0..N / 2)
            .map(|k| {
                let angle = tau * k as f64 / N as f64;
                (libm::cos(angle), -libm::sin(angle))
            })
            .collect();
        let bits = N.trailing_zeros();
        self.reversed = (0..N)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        self.reset();
    }

    fn reset(&mut self) {
        self.history.iter_mut().for_each(|x| *x = 0.0);
        self.held.iter_mut().for_each(|x| *x = 0.0);
        self.next = 0;
        self.count = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let frames = inputs.frames();
        let input = inputs.signal(0);
        let mut from = 0;
        for n in 0..frames {
            if let Some(slot) = self.history.get_mut(self.next) {
                *slot = input.get(n).copied().unwrap_or(0.0);
            }
            self.next = (self.next + 1) % N;
            self.count = (self.count + 1) % Self::HOP;
            if self.count == 0 {
                write_held(outputs, &self.held, from, n);
                self.analyse();
                from = n;
            }
        }
        write_held(outputs, &self.held, from, frames);
    }
}

/// Writes `held` (one value per output row) to frames `from .. to` of every
/// output row.
fn write_held(outputs: &mut Outputs<'_>, held: &[f64], from: usize, to: usize) {
    for (row, &value) in outputs.signals().zip(held) {
        if let Some(span) = row.get_mut(from..to) {
            span.fill(value);
        }
    }
}

macro_rules! spectrum_descriptor {
    ($name:ident, $type_name:literal, $n:literal) => {
        const $name: ModuleDescriptor = ModuleDescriptor {
            type_name: $type_name,
            doc: concat!(
                "Windowed magnitude spectrum of ",
                stringify!($n),
                " frames (Hann, every N/4 frames): mag[b] the amplitude at b·fs/N (a bin-centred sine of amplitude A reads A), peak_hz and peak its strongest partial (ADR 0055)."
            ),
            ports: &[
                signal_port("in", Direction::In, PortKind::Audio, None, None),
                port_at(
                    "mag",
                    Direction::Out,
                    PortShape::Vector($n / 2),
                    1,
                    PortKind::Generic,
                    None,
                    None,
                ),
                signal_port("peak_hz", Direction::Out, PortKind::Generic, Some("Hz"), None),
                signal_port("peak", Direction::Out, PortKind::Generic, None, None),
            ],
            parameters: &[],
            feedthrough: Feedthrough::Direct,
            sink: None,
            source: None,
            latency: None,
            reproducibility: signal_core::Reproducibility::BitExact,
            category: Some(crate::category::ANALYSIS),
        };
    };
}

spectrum_descriptor!(DESCRIPTOR_512, "std.spectrum512", 512);
spectrum_descriptor!(DESCRIPTOR_2048, "std.spectrum2048", 2048);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::context;
    use signal_core::EventSink;

    /// Renders `signal` through a spectrum of `N` at 48 kHz in calls of
    /// `block` frames: the outputs' rows (`N/2` bins, `peak_hz`, `peak`).
    fn run<const N: usize>(signal: &[f64], block: usize) -> Vec<Vec<f64>> {
        let mut spectrum = Spectrum::<N>::default();
        spectrum.prepare(&context(48_000.0, block, 0));
        let rows = N / 2 + 2;
        let mut out_rows = vec![Vec::with_capacity(signal.len()); rows];
        for chunk in signal.chunks(block) {
            let k = chunk.len();
            let mut out = vec![f64::NAN; rows * k];
            spectrum.process(
                &Inputs::new(k, chunk, &[], &[], &[]),
                &mut Outputs::new(k, &mut out, &mut Vec::<EventSink>::new()),
            );
            for (row, values) in out_rows.iter_mut().zip(out.chunks(k)) {
                row.extend_from_slice(values);
            }
        }
        out_rows
    }

    fn sine(frames: usize, hz: f64, amplitude: f64) -> Vec<f64> {
        (0..frames)
            .map(|n| amplitude * (2.0 * std::f64::consts::PI * hz * n as f64 / 48_000.0).sin())
            .collect()
    }

    /// The last frame's value of each row.
    fn last(rows: &[Vec<f64>]) -> Vec<f64> {
        rows.iter().map(|r| *r.last().unwrap()).collect()
    }

    /// ADR 0055 verification 1: a bin-centred sine reads its amplitude in its
    /// bin and its frequency as the peak.
    #[test]
    fn a_bin_centred_sine_reads_its_amplitude_and_frequency() {
        let hz = 40.0 * 48_000.0 / 2048.0;
        let out = last(&run::<2048>(&sine(8192, hz, 0.25), 256));
        assert!((out[40] - 0.25).abs() < 1e-9, "{}", out[40]);
        assert!(
            out[38].abs() < 1e-9 && out[42].abs() < 1e-9,
            "Hann: two bins off read 0"
        );
        assert!((out[1024] - hz).abs() < 1e-6, "peak_hz {}", out[1024]);
        assert!((out[1025] - 0.25).abs() < 1e-9, "peak {}", out[1025]);
    }

    #[test]
    fn an_off_bin_sine_reads_close_to_its_frequency_and_amplitude() {
        let spacing = 48_000.0 / 2048.0;
        let hz = 40.3 * spacing;
        let out = last(&run::<2048>(&sine(8192, hz, 0.5), 64));
        assert!(
            (out[1024] - hz).abs() < 0.005 * spacing,
            "peak_hz {} for {hz}",
            out[1024]
        );
        assert!((out[1025] - 0.5).abs() < 0.005 * 0.5, "peak {}", out[1025]);
    }

    #[test]
    fn two_sines_each_read_in_their_bin_and_a_constant_in_bin_0() {
        let spacing = 48_000.0 / 512.0;
        let signal: Vec<f64> = sine(4096, 10.0 * spacing, 0.3)
            .iter()
            .zip(sine(4096, 50.0 * spacing, 0.1))
            .map(|(a, b)| a + b + 0.2)
            .collect();
        let out = last(&run::<512>(&signal, 100));
        assert!((out[10] - 0.3).abs() < 1e-9 && (out[50] - 0.1).abs() < 1e-9);
        assert!((out[0] - 0.2).abs() < 1e-9, "bin 0 {}", out[0]);
        assert!((out[256] - 10.0 * spacing).abs() < 1e-6);
    }

    #[test]
    fn silence_reads_zero_and_the_split_never_matters() {
        let out = last(&run::<512>(&vec![0.0; 2048], 64));
        assert!(out.iter().all(|&v| v == 0.0));
        let signal = sine(5000, 1234.5, 0.4);
        assert_eq!(run::<512>(&signal, 1), run::<512>(&signal, 333));
        assert_eq!(run::<2048>(&signal, 7), run::<2048>(&signal, 512));
    }

    /// Analyses fall every N/4 frames from reset, the outputs held between.
    #[test]
    fn it_analyses_every_quarter_and_holds_between() {
        let rows = run::<512>(&sine(1024, 9375.0, 0.5), 1024);
        let peak = &rows[257];
        assert_eq!(peak[126], 0.0, "nothing before the first hop");
        assert!(peak[127] > 0.0, "the hop's last frame analyses");
        assert_eq!(peak[127], peak[253], "held until the next");
    }
}
