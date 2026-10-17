//! `std.modal`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    ParameterDescriptor, PortKind, PrepareContext,
};

use crate::sine_osc::FREQ_MAX;
use crate::svf::flush;
use crate::{at, parameter, round_half_up, signal_port};

/// The most modes a bank has.
pub(crate) const MAX_MODES: usize = 64;
/// The explicit ratios.
const RATIOS: usize = 16;
const FREQ: f64 = 220.0;
/// The default `material`: bar.
const BAR: f64 = 1.0;
const MODES: f64 = 16.0;
const DECAY_S: f64 = 2.0;
const LAW: f64 = 1.0;
const POSITION: f64 = 0.3;
/// The parameters before `ratio_1`.
const FIXED: usize = 6;

const fn ratio(name: &'static str, default: f64) -> ParameterDescriptor {
    parameter(
        name,
        PortKind::Generic,
        None,
        0.01,
        100.0,
        default,
        Combine::Add,
    )
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.modal",
    doc: "Modal resonator bank: up to 64 two-pole resonators at the mode ratios of a string, bar, plate, membrane or bell, or 16 explicit ratios; T60 per mode decay_s·ratio^(−law), amplitudes from position.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "freq",
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            FREQ_MAX,
            FREQ,
            Combine::Add,
        ),
        parameter(
            "material",
            PortKind::Generic,
            None,
            0.0,
            5.0,
            BAR,
            Combine::Replace,
        ),
        parameter(
            "modes",
            PortKind::Generic,
            None,
            1.0,
            MAX_MODES as f64,
            MODES,
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
        parameter("law", PortKind::Control, None, 0.0, 3.0, LAW, Combine::Add),
        parameter(
            "position",
            PortKind::Control,
            None,
            0.01,
            0.99,
            POSITION,
            Combine::Add,
        ),
        ratio("ratio_1", 1.0),
        ratio("ratio_2", 2.0),
        ratio("ratio_3", 3.0),
        ratio("ratio_4", 4.0),
        ratio("ratio_5", 5.0),
        ratio("ratio_6", 6.0),
        ratio("ratio_7", 7.0),
        ratio("ratio_8", 8.0),
        ratio("ratio_9", 9.0),
        ratio("ratio_10", 10.0),
        ratio("ratio_11", 11.0),
        ratio("ratio_12", 12.0),
        ratio("ratio_13", 13.0),
        ratio("ratio_14", 14.0),
        ratio("ratio_15", 15.0),
        ratio("ratio_16", 16.0),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::PHYSICAL),
};

/// A free–free bar (Euler–Bernoulli beam): `(β_k/β_1)²`, β_k the roots of
/// `cos β·cosh β = 1`, to 10 decimals.
const BAR_RATIOS: [f64; MAX_MODES] = [
    1.0000000000,
    2.7565385071,
    5.4039176324,
    8.9329503524,
    13.3442866937,
    18.6378878866,
    24.8137560688,
    31.8718911281,
    39.8122930701,
    48.6349618944,
    58.3398976013,
    68.9271001905,
    80.3965696623,
    92.7483060164,
    105.9823092530,
    120.0985793720,
    135.0971163735,
    150.9779202574,
    167.7409910237,
    185.3863286725,
    203.9139332037,
    223.3238046174,
    243.6159429135,
    264.7903480920,
    286.8470201530,
    309.7859590964,
    333.6071649222,
    358.3106376305,
    383.8963772213,
    410.3643836944,
    437.7146570500,
    465.9471972881,
    495.0620044086,
    525.0590784115,
    555.9384192969,
    587.7000270647,
    620.3439017149,
    653.8700432476,
    688.2784516627,
    723.5691269603,
    759.7420691403,
    796.7972782027,
    834.7347541476,
    873.5544969749,
    913.2565066847,
    953.8407832769,
    995.3073267515,
    1037.6561371086,
    1080.8872143481,
    1125.0005584700,
    1169.9961694744,
    1215.8740473612,
    1262.6341921305,
    1310.2766037822,
    1358.8012823164,
    1408.2082277329,
    1458.4974400320,
    1509.6689192134,
    1561.7226652773,
    1614.6586782237,
    1668.4769580524,
    1723.1775047637,
    1778.7603183573,
    1835.2253988334,
];

/// A simply supported square plate: the distinct values of `(m² + n²)/2`,
/// m, n ≥ 1, in increasing order (degenerate pairs once).
const PLATE_RATIOS: [f64; MAX_MODES] = [
    1.0, 2.5, 4.0, 5.0, 6.5, 8.5, 9.0, 10.0, 12.5, 13.0, 14.5, 16.0, 17.0, 18.5, 20.0, 20.5, 22.5,
    25.0, 26.0, 26.5, 29.0, 30.5, 32.5, 34.0, 36.0, 36.5, 37.0, 40.0, 41.0, 42.5, 44.5, 45.0, 48.5,
    49.0, 50.0, 50.5, 52.0, 53.0, 54.5, 56.5, 58.0, 58.5, 61.0, 62.5, 64.0, 65.0, 68.0, 68.5, 72.5,
    73.0, 74.0, 74.5, 76.5, 78.5, 80.0, 81.0, 82.0, 84.5, 85.0, 86.5, 89.0, 90.0, 90.5, 92.5,
];

/// An ideal circular membrane: the zeros `j_mn` of the Bessel functions `J_m`
/// (m ≥ 0, n ≥ 1) over `j_01` = 2.4048255577, in increasing order, to 10
/// decimals (each `j_mn` once).
const MEMBRANE_RATIOS: [f64; MAX_MODES] = [
    1.0000000000,
    1.5933405057,
    2.1355487866,
    2.2954172674,
    2.6530664045,
    2.9172954551,
    3.1554648154,
    3.5001474903,
    3.5984846740,
    3.6474511791,
    4.0589318833,
    4.1317381597,
    4.2304391279,
    4.6010445344,
    4.6100516454,
    4.8318852629,
    4.9032805732,
    5.0835671739,
    5.1307689067,
    5.4121184300,
    5.5403985099,
    5.5531264772,
    5.6508423769,
    5.9765402216,
    6.0193558074,
    6.1526091716,
    6.1631367313,
    6.2087321306,
    6.4827354461,
    6.5286124515,
    6.6689969007,
    6.7462132995,
    6.8489916028,
    6.9436429102,
    7.0707081490,
    7.1694266253,
    7.3252573325,
    7.4023810568,
    7.4682421091,
    7.5145009625,
    7.6045361269,
    7.6651978386,
    7.8591970601,
    7.8925200268,
    8.0710283390,
    8.1313741732,
    8.1568737689,
    8.1569188423,
    8.3142956319,
    8.4500055102,
    8.6450787640,
    8.6522066944,
    8.6604755552,
    8.7678484810,
    8.7810930757,
    8.8204471056,
    8.9992144963,
    9.1300776464,
    9.1678106523,
    9.2200012379,
    9.2388405577,
    9.3905894841,
    9.4643390277,
    9.5413045900,
];

/// The partials of an ideally tuned minor-third church bell, relative to the
/// prime: hum, prime, tierce, quint, nominal, deciem, undeciem, duodeciem,
/// double octave, upper undeciem, upper duodeciem, triple octave.
const BELL_RATIOS: [f64; 12] = [
    0.5,
    1.0,
    1.2,
    1.5,
    2.0,
    2.5,
    8.0 / 3.0,
    3.0,
    4.0,
    16.0 / 3.0,
    6.0,
    8.0,
];

/// `std.modal`: a bank of up to 64 two-pole resonators, one per mode.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded): the excitation, fed to every mode;
///   unconnected it reads 0.
/// - `out` (signal out, audio): the sum of the modes.
///
/// **Parameters**
/// - `freq` (frequency, Hz, range `[1, sr/2]`, default 220, combine `add`):
///   the frequency of ratio 1. Declared `[0, (2³² − 1)/2]` as `std.osc`'s;
///   `process` limits it to at least 1 Hz, and a mode at or above `0.49·sr`
///   is silent.
/// - `material` (generic, no unit, choice, range `[0, 5]`, default 1,
///   combine `replace`): `round_half_up(material)` selects the mode ratios
///   `r_k`: 0 string (`k`), 1 bar (free–free beam), 2 plate (simply
///   supported square), 3 membrane (circular), 4 bell (12 partials of a
///   minor-third bell, from the hum at ½), 5 explicit (`ratio_1` …
///   `ratio_16`). The tables are in the source, to 10 decimals.
/// - `modes` (generic, integer, range `[1, 64]`, default 16, combine `add`):
///   `round_half_up(modes)` modes sound, and no more than the material has
///   (12 for the bell, 16 explicit).
/// - `decay_s` (control, seconds, range `[0.01, 60]`, default 2, combine
///   `add`): the T60 of ratio 1.
/// - `law` (control, no unit, range `[0, 3]`, default 1, combine `add`): the
///   decay law, `T60_k = decay_s·r_k^(−law)`; 0 decays every mode alike,
///   1 makes higher modes decay as many periods as ratio 1.
/// - `position` (control, fraction, range `[0.01, 0.99]`, default 0.3,
///   combine `add`): where the bank is struck and heard; mode `k` (1-based,
///   in the table's order) has amplitude `sin(π·k·position)`.
/// - `ratio_1` … `ratio_16` (generic, no unit, range `[0.01, 100]`, defaults
///   1 … 16, combine `add`): the explicit ratios.
///
/// **Feedthrough:** direct (`in[n]` reaches `out[n]`).
///
/// **Formula** (sr the sample rate; per mode k = 1 … K):
///
/// ```text
/// f₁   = max(freq, 1),   f_k = f₁·r_k,   θ_k = 2π·f_k/sr
/// ρ_k  = 10^(−3·r_k^law/(decay_s·sr))                pole radius
/// a_k  = sin(π·k·position)                          0 when f_k ≥ 0.49·sr
/// g_k  = a_k·(2π·f₁/sr)
/// u_k[n] = ρ_k·(cos θ_k·u_k[n−1] − sin θ_k·v_k[n−1]) + g_k·cos θ_k·in[n]
/// v_k[n] = ρ_k·(sin θ_k·u_k[n−1] + cos θ_k·v_k[n−1]) + g_k·sin θ_k·in[n]
/// out[n] = Σ_k v_k[n]
/// ```
///
/// Each mode is a two-pole resonator in coupled form: with
/// `z_k = u_k + j·v_k` it is `z_k[n] = e^(jθ_k)·(ρ_k·z_k[n−1] + g_k·in[n])`,
/// and the output is `Im z_k`.
/// Its poles are `ρ_k·e^(±jθ_k)`, as a direct-form
/// `y[n] = 2ρ·cos θ·y[n−1] − ρ²·y[n−2]` has, but its state is rotated and
/// scaled by `ρ_k < 1` on every frame, so `|z_k|` never grows by more than
/// the input adds, **whatever θ_k and ρ_k do from frame to frame**: the bank
/// stays bounded under any modulation of its parameters (a direct form does
/// not: alternating its coefficients every frame can grow without bound).
///
/// An impulse into mode k rings as
/// `a_k·(2π·f₁/sr)·ρ_k^n·sin((n + 1)·θ_k)`: a damped sinusoid at exactly
/// `f_k` with a T60 of `decay_s·r_k^(−law)` seconds. **The input is a force
/// density**: an excitation of area `J = Σ in[n]/sr` (unit·seconds), short
/// against the periods, rings every mode at amplitude `a_k·2π·f₁·J`, at any
/// sample rate. (Without the factor `2π·f₁/sr` the same excitation, which has
/// twice the frames at twice the rate, would ring twice as loud.) `pow`,
/// `sin` and `cos` are `libm`'s. Starting state 0. The coefficients are
/// recomputed only on a frame where a parameter differs from the frame
/// before (always on the first frame); a mode's state is kept when they
/// change. Each state is flushed to exactly 0 below 1e-9
/// (`docs/dsp-conventions.md`, "Denormals").
/// Work per frame is proportional to the sounding modes, at most 64, plus a
/// coefficient update on a changed frame.
#[derive(Clone, Debug)]
pub struct Modal {
    sample_rate: f64,
    /// The parameters the coefficients were computed for (NaN: none yet).
    computed: [f64; FIXED + RATIOS],
    /// The modes that sound.
    count: usize,
    modes: [Mode; MAX_MODES],
    /// `(u_k, v_k)` of the last frame.
    state: [(f64, f64); MAX_MODES],
}

/// One mode's coefficients: `ρ·cos θ`, `ρ·sin θ`, `g·cos θ`, `g·sin θ`.
#[derive(Clone, Copy, Debug, Default)]
struct Mode {
    rc: f64,
    rs: f64,
    gc: f64,
    gs: f64,
}

impl Default for Modal {
    fn default() -> Self {
        Modal {
            sample_rate: 0.0,
            computed: [f64::NAN; FIXED + RATIOS],
            count: 0,
            modes: [Mode::default(); MAX_MODES],
            state: [(0.0, 0.0); MAX_MODES],
        }
    }
}

/// The ratio of mode `k` (0-based) of `material`, with the explicit ratios
/// `explicit`; `None` past the material's last mode.
pub(crate) fn mode_ratio(material: f64, k: usize, explicit: &[f64]) -> Option<f64> {
    match round_half_up(material) as i64 {
        0 => (k < MAX_MODES).then_some(k as f64 + 1.0),
        1 => BAR_RATIOS.get(k).copied(),
        2 => PLATE_RATIOS.get(k).copied(),
        3 => MEMBRANE_RATIOS.get(k).copied(),
        4 => BELL_RATIOS.get(k).copied(),
        _ => explicit.get(k).copied(),
    }
}

impl Modal {
    fn update(&mut self, p: &[f64; FIXED + RATIOS]) {
        let [freq, material, modes, decay_s, law, position, ..] = *p;
        let explicit = p.get(FIXED..).unwrap_or(&[]);
        let freq = freq.max(1.0);
        let wanted = (round_half_up(modes).max(1.0) as usize).min(MAX_MODES);
        let sr = self.sample_rate;
        let density = std::f64::consts::TAU * freq / sr;
        self.count = 0;
        for k in 0..wanted {
            let Some(r) = mode_ratio(material, k, explicit) else {
                break;
            };
            let f = freq * r;
            let theta = std::f64::consts::TAU * f / sr;
            let rho = libm::pow(10.0, -3.0 * libm::pow(r, law) / (decay_s * sr));
            let amplitude = if f < 0.49 * sr {
                libm::sin(std::f64::consts::PI * (k as f64 + 1.0) * position)
            } else {
                0.0
            };
            if let Some(mode) = self.modes.get_mut(k) {
                let (cos, sin) = (libm::cos(theta), libm::sin(theta));
                let g = amplitude * density;
                *mode = Mode {
                    rc: rho * cos,
                    rs: rho * sin,
                    gc: g * cos,
                    gs: g * sin,
                };
            }
            self.count = k + 1;
        }
    }
}

impl DirectPrimitive for Modal {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.computed = [f64::NAN; FIXED + RATIOS];
        self.count = 0;
        self.state = [(0.0, 0.0); MAX_MODES];
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let defaults = DESCRIPTOR.parameters;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let mut wanted = [0.0; FIXED + RATIOS];
            for (i, value) in wanted.iter_mut().enumerate() {
                let default = defaults.get(i).map_or(0.0, |d| d.default);
                *value = at(inputs.parameter(i), n, default);
            }
            if wanted != self.computed {
                self.computed = wanted;
                self.update(&wanted);
            }
            let input = at(x, n, 0.0);
            let mut sum = 0.0;
            let modes = self.modes.iter().zip(self.state.iter_mut());
            for (m, (u, v)) in modes.take(self.count) {
                let next_u = flush(m.rc * *u - m.rs * *v + m.gc * input);
                *v = flush(m.rs * *u + m.rc * *v + m.gs * input);
                *u = next_u;
                sum += *v;
            }
            *out = sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::string::measure::partial;
    use crate::testing::{Timeline, context, render};

    /// The bank's response to a unit impulse on frame 0.
    fn impulse(params: &[(usize, f64)], sample_rate: f64, frames: usize) -> Vec<f64> {
        let mut modal = Modal::default();
        modal.prepare(&context(sample_rate, 256, 0));
        let mut lanes: Vec<Vec<f64>> = DESCRIPTOR
            .parameters
            .iter()
            .map(|p| vec![p.default; frames])
            .collect();
        for &(i, value) in params {
            lanes[i] = vec![value; frames];
        }
        let lanes: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
        let mut x = vec![0.0; frames];
        x[0] = 1.0;
        render(
            &mut modal,
            &Timeline {
                signals: &[&x],
                parameters: &lanes,
                triggers: &[],
            },
            frames,
            256,
        )
        .signals[0]
            .clone()
    }

    /// AC2: for every material, each mode's frequency is within 0.1 % of
    /// `freq·r_k` and its T60 within 1 % of `decay_s·r_k^(−law)`, at two laws.
    #[test]
    fn modes_ring_at_their_ratios_and_decay_by_the_law() {
        let (sample_rate, freq, decay_s) = (48_000.0, 110.0, 3.0);
        let explicit = [1.0, 1.37, 2.71, 4.05, 5.92, 7.3, 9.1, 11.55];
        for material in 0..=5 {
            for law in [1.0, 0.5] {
                let mut params = vec![(0, freq), (1, material as f64), (2, 8.0)];
                params.extend([(3, decay_s), (4, law), (5, 0.37)]);
                params.extend(explicit.iter().enumerate().map(|(i, &r)| (FIXED + i, r)));
                let y = impulse(&params, sample_rate, 2 * 48_000);
                for k in 0..8 {
                    let r = mode_ratio(material as f64, k, &explicit).unwrap();
                    // Neighbours closer than 3 % are not resolved by the
                    // windows below; none are among the first 8 modes.
                    let f = freq * r;
                    let t60 = decay_s * libm::pow(r, -law);
                    // Windows a third of the T60 apart: a 20 dB fall.
                    let span = ((t60 / 3.0).min(0.5) * sample_rate) as usize;
                    let (measured, measured_t60) = partial(&y, f, 0, span, span, sample_rate);
                    println!(
                        "material {material}, law {law}, mode {}: {measured:.4} Hz \
                         (expected {f:.4}), T60 {measured_t60:.4} s (expected {t60:.4})",
                        k + 1
                    );
                    assert!((measured / f - 1.0).abs() < 1e-3, "frequency");
                    assert!((measured_t60 / t60 - 1.0).abs() < 0.01, "T60");
                }
            }
        }
    }

    /// The amplitudes follow the position: struck in the middle, a string's
    /// even modes are silent; mode 1's impulse response is exactly
    /// `a₁·θ₁·ρ^n·sin((n + 1)θ₁)`.
    #[test]
    fn amplitudes_follow_the_position() {
        let sample_rate = 48_000.0;
        let y = impulse(
            &[(0, 300.0), (1, 0.0), (2, 2.0), (5, 0.5)],
            sample_rate,
            4800,
        );
        let theta = std::f64::consts::TAU * 300.0 / sample_rate;
        let rho = libm::pow(10.0, -3.0 / (DECAY_S * sample_rate));
        for (n, &v) in y.iter().enumerate().take(1000) {
            let expected = theta * libm::pow(rho, n as f64) * libm::sin((n as f64 + 1.0) * theta);
            approx::assert_abs_diff_eq!(v, expected, epsilon = 1e-12);
        }
    }

    /// The input is a force density: the same 1 ms raised-cosine strike rings
    /// every mode alike at 44.1, 48, 96 and 192 kHz (to the sampling of the
    /// pulse), at `a_k·2π·f₁·J`.
    #[test]
    fn a_strike_rings_alike_at_every_rate() {
        let (freq, position) = (200.0, 0.37);
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut modal = Modal::default();
            modal.prepare(&context(sample_rate, 256, 0));
            let frames = sample_rate as usize;
            let mut lanes: Vec<Vec<f64>> = DESCRIPTOR
                .parameters
                .iter()
                .map(|p| vec![p.default; frames])
                .collect();
            for (i, value) in [(0, freq), (1, 0.0), (2, 3.0), (3, 2.0), (5, position)] {
                lanes[i] = vec![value; frames];
            }
            let lanes: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
            let width = 0.001 * sample_rate;
            let x: Vec<f64> = (0..frames)
                .map(|n| match (n as f64) < width {
                    true => 0.5 - 0.5 * libm::cos(std::f64::consts::TAU * n as f64 / width),
                    false => 0.0,
                })
                .collect();
            let area: f64 = x.iter().sum::<f64>() / sample_rate;
            let timeline = Timeline {
                signals: &[&x],
                parameters: &lanes,
                triggers: &[],
            };
            let y = render(&mut modal, &timeline, frames, 256).signals[0].clone();
            let span = frames / 4;
            for k in 1..=3 {
                let f = freq * k as f64;
                // The windowed DFT of a damped sinusoid of amplitude A starting
                // at t = 0, measured on [span, 2·span): A/4·e^(−σ·t)·span
                // (Hann gain ½, one side ½), σ from the T60.
                let (magnitude, _) =
                    crate::string::measure::component(&y, span, span, f, sample_rate);
                let t60 = DECAY_S / k as f64;
                let sigma = 3.0 * std::f64::consts::LN_10 / t60;
                let window: f64 = (0..span)
                    .map(|i| {
                        let w =
                            0.5 - 0.5 * libm::cos(std::f64::consts::TAU * i as f64 / span as f64);
                        w * libm::exp(-sigma * (span + i) as f64 / sample_rate)
                    })
                    .sum();
                let amplitude = 2.0 * magnitude / window;
                let a_k = libm::sin(std::f64::consts::PI * k as f64 * position);
                // A 1 ms pulse is short against 1.7 ms (600 Hz): its spectrum
                // at f_k, |X(f_k)|/J, is the raised cosine's, below 1.
                let shape = {
                    let u = f * 0.001;
                    (libm::sin(std::f64::consts::PI * u) / (std::f64::consts::PI * u))
                        / (1.0 - u * u)
                };
                let expected = (a_k * std::f64::consts::TAU * freq * area * shape).abs();
                println!(
                    "{sample_rate} Hz, mode {k}: amplitude {amplitude:.6} (expected {expected:.6})"
                );
                assert!((amplitude / expected - 1.0).abs() < 0.01);
            }
        }
    }

    /// Modes at or above 0.49·sr are silent, and a material has no more modes
    /// than its table.
    #[test]
    fn modes_stop_below_nyquist_and_at_the_table() {
        let mut modal = Modal::default();
        modal.prepare(&context(48_000.0, 1, 0));
        let mut p = [0.0; FIXED + RATIOS];
        for (i, d) in DESCRIPTOR.parameters.iter().enumerate() {
            p[i] = d.default;
        }
        p[0] = 1000.0;
        p[2] = 64.0;
        modal.update(&p);
        assert_eq!(modal.count, 64);
        // Bar mode 6 is 18.6 kHz, mode 7 is 24.8 kHz ≥ 23.52 kHz.
        assert!(modal.modes[5].gs != 0.0);
        assert!(modal.modes[6..].iter().all(|m| m.gc == 0.0 && m.gs == 0.0));
        p[1] = 4.0;
        modal.update(&p);
        assert_eq!(modal.count, 12);
        p[1] = 5.0;
        modal.update(&p);
        assert_eq!(modal.count, 16);
    }
}
