//! `std.tanh_shaper_os8`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::oversample::Oversampled;
use crate::{at, parameter, signal_port};

/// ×2 stages: 8× oversampling.
const STAGES: usize = 3;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.tanh_shaper_os8",
    doc: "tanh saturation of drive·in at 8× the rate with first-order ADAA, linear-phase FIR resampling (73.8125 frames of delay; a clipped high tone overshoots to about ±1.27).",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        // No range: a band-limited clipped wave overshoots 1 (up to ≈ 1.27).
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[parameter(
        "drive",
        PortKind::Gain,
        None,
        0.1,
        20.0,
        1.0,
        Combine::Add,
    )],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::FILTER),
};

/// `std.tanh_shaper_os8`: `tanh` saturation at 8× the frame rate, with
/// first-order ADAA there, for uses where aliasing matters and a delay of
/// 73.8125 frames does not (a master limiter, a hard-driven voice).
/// Promoted from `lab.tanh_os8_adaa1` (M3 "ADAA versus oversampling",
/// `experiments/m3-tanh-aliasing/`, verdict 2026-10-23); `std.tanh_shaper`
/// stays as it is beside it.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `out` (signal out, audio, **no declared range**: see "Overshoot").
///
/// **Parameters**
/// - `drive` (gain, linear, no unit, range `[0.1, 20]`, default 1, combine
///   `add`), as `std.tanh_shaper`'s: a project switches between the two by
///   the type name alone.
///
/// **Feedthrough:** direct.
///
/// **Formula.** `u[n] = drive[n]·in[n]` is raised to 8·sr by three cascaded
/// ×2 linear-phase FIR stages (Kaiser-windowed sinc, β = 9, about 90 dB of
/// stopband; polyphase), shaped there by `std.tanh_shaper`'s first-order
/// ADAA (`docs/dsp-conventions.md`), and brought back by the same stages
/// ([`oversample`](crate::oversample)). Stage 1 (131 taps) passes 0 … 0.40·sr
/// and stops from 0.50·sr; stages 2 and 3 are half-band filters of 27 and 19
/// taps. The coefficients are designed in `prepare`, in normalized
/// frequency, so they are the same at every rate.
///
/// **Delay: 73.8125 frames** at every rate (0.77 ms at 96 kHz, 1.54 ms at
/// 48 kHz): (131 − 1)/2 samples at 2·sr, (27 − 1)/2 at 4·sr and (19 − 1)/2
/// at 8·sr, each way, plus ADAA's half sample at 8·sr
/// ([`DELAY_FRAMES`](Self::DELAY_FRAMES)). The filters are linear phase, so
/// the delay is the same at every passband frequency. A direct type cannot
/// declare a latency and the engine compensates no path, so a parallel dry
/// path that must stay aligned needs a `std.delay` of the same length; in a
/// feedback loop the delay adds to every pass.
///
/// **Overshoot.** The output is band-limited, so a hard-clipped (square-like)
/// wave overshoots 1: at drive 20 and 48 kHz, 1.10 at 1009 Hz and 1.27 at
/// 8009 Hz, where only the fundamental lies in the passband (a lone
/// fundamental of a unit square peaks at 4/π ≈ 1.27). The onset of such a
/// tone from silence rings once a little higher (1.33 measured on the first
/// frames of that 8009 Hz tone). Slow and low-drive signals stay within
/// `[−1, 1]`.
///
/// **Measured** (`experiments/m3-tanh-aliasing/README.md`): V6's inharmonic
/// ratio at drive 20 and 96 kHz is 141.2 / 130.9 / 119.0 / 101.6 dB at 1009 /
/// 2003 / 4001 / 8009 Hz (48 kHz: worst 79.5 dB at 8009 Hz), against
/// `std.tanh_shaper`'s 49.0 / 33.5 / 26.1 / 22.0 dB; −0.03 dB at 10 kHz and
/// 48 kHz; about 250 ns per frame (`std.tanh_shaper`: 55 ns).
///
/// **Real time.** `prepare` designs the filters and allocates once; `process`
/// does fixed work per frame (three FIR cascades up and down, eight ADAA
/// evaluations) and never allocates. Before `prepare` the output is 0.
#[derive(Debug, Default)]
pub struct TanhShaperOs8 {
    shaper: Option<Box<Oversampled>>,
}

impl TanhShaperOs8 {
    /// The delay in frames, at every sample rate.
    pub const DELAY_FRAMES: f64 = 73.8125;
}

impl DirectPrimitive for TanhShaperOs8 {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        // Designs the filters; the only allocation.
        self.shaper = Some(Box::new(Oversampled::new(STAGES, true)));
    }

    fn reset(&mut self) {
        if let Some(shaper) = &mut self.shaper {
            shaper.clear();
        }
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let drive = inputs.parameter(0);
        let out = outputs.signal(0);
        match &mut self.shaper {
            Some(shaper) => {
                for (n, out) in out.iter_mut().enumerate() {
                    *out = shaper.tick(at(drive, n, 0.0) * at(x, n, 0.0));
                }
            }
            None => out.fill(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oversample::latency_frames;
    use crate::testing::{Timeline, context, render};

    fn shape(input: &[f64], drive: f64, block: usize) -> Vec<f64> {
        let mut shaper = TanhShaperOs8::default();
        shaper.prepare(&context(48_000.0, 256, 0));
        let drive = vec![drive; input.len()];
        let run = render(
            &mut shaper,
            &Timeline {
                signals: &[input],
                parameters: &[&drive],
                triggers: &[],
            },
            input.len(),
            block,
        );
        run.signals.into_iter().next().unwrap()
    }

    #[test]
    fn the_documented_delay_is_the_filters() {
        assert_eq!(TanhShaperOs8::DELAY_FRAMES, latency_frames(STAGES, true));
        assert!(DESCRIPTOR.doc.contains("73.8125 frames of delay"));
        assert!(DESCRIPTOR.doc.contains("1.27"));
        assert_eq!(DESCRIPTOR.ports[1].range, None);
    }

    /// The same ports and parameter as `std.tanh_shaper`, except that `out`
    /// declares no range: a project swaps one for the other by its type name.
    #[test]
    fn it_has_the_ports_and_parameter_of_std_tanh_shaper() {
        let shaper = crate::TanhShaper::describe();
        assert_eq!(DESCRIPTOR.parameters, shaper.parameters);
        assert_eq!(DESCRIPTOR.ports[0], shaper.ports[0]);
        let out = signal_core::PortDescriptor {
            range: None,
            ..shaper.ports[1]
        };
        assert_eq!(DESCRIPTOR.ports[1], out);
        assert_eq!(DESCRIPTOR.ports.len(), 2);
        assert_eq!(DESCRIPTOR.feedthrough, shaper.feedthrough);
        assert_eq!(DESCRIPTOR.category, shaper.category);
    }

    /// The type is the oversampled shaper at 8× with ADAA, frame for frame.
    #[test]
    fn it_is_the_oversampled_shaper_at_8x_with_adaa() {
        let input: Vec<f64> = (0..2_000)
            .map(|n| libm::sin(std::f64::consts::TAU * 2003.0 * n as f64 / 48_000.0))
            .collect();
        let out = shape(&input, 20.0, 64);
        let mut reference = Oversampled::new(3, true);
        for (n, (&x, &y)) in input.iter().zip(&out).enumerate() {
            assert_eq!(y.to_bits(), reference.tick(20.0 * x).to_bits(), "{n}");
        }
    }

    /// A slow sine at low drive comes out as `tanh` of the input delayed by
    /// 73.8125 frames.
    #[test]
    fn a_slow_sine_comes_out_delayed() {
        let period = 480.0;
        let input: Vec<f64> = (0..4_000)
            .map(|n| 0.5 * libm::sin(std::f64::consts::TAU * n as f64 / period))
            .collect();
        let out = shape(&input, 0.1, 256);
        for (n, &y) in out.iter().enumerate().skip(500) {
            let t = n as f64 - TanhShaperOs8::DELAY_FRAMES;
            let expected = libm::tanh(0.05 * libm::sin(std::f64::consts::TAU * t / period));
            assert!((y - expected).abs() < 1e-6, "{n}: {y} vs {expected}");
        }
    }

    /// Hard clipping overshoots 1, by at most about 4/π once the start-up
    /// transient (the step from silence, which reads about 1.33) has passed:
    /// the experiment's peak table skips 1000 frames, as here.
    #[test]
    fn a_clipped_high_tone_overshoots_by_at_most_4_over_pi() {
        let input: Vec<f64> = (0..48_000)
            .map(|n| libm::sin(std::f64::consts::TAU * 8009.0 * n as f64 / 48_000.0))
            .collect();
        let out = shape(&input, 20.0, 256);
        let peak = out.iter().skip(1000).fold(0.0_f64, |m, y| m.max(y.abs()));
        assert!(peak > 1.0 && peak < 1.28, "{peak}");
    }

    /// Unprepared, the output is silence.
    #[test]
    fn unprepared_it_outputs_zero() {
        let mut shaper = TanhShaperOs8::default();
        let input = [1.0; 8];
        let drive = [1.0; 8];
        let run = render(
            &mut shaper,
            &Timeline {
                signals: &[&input],
                parameters: &[&drive],
                triggers: &[],
            },
            8,
            8,
        );
        assert!(run.signals[0].iter().all(|y| *y == 0.0));
    }
}
