//! `std.modal`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    ParameterDescriptor, PortDescriptor, PortKind, PortShape, PrepareContext,
};

use crate::math::bounded;
use crate::sine_osc::FREQ_MAX;
use crate::svf::flush;
use crate::{at, parameter, port_at, round_half_up, signal_port};

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
const PICKUP: f64 = 0.5;
/// The parameters before `ratio_1`.
const FIXED: usize = 6;
/// The index of `inharmonicity`, the first parameter after `ratio_16`; the
/// others follow in descriptor order.
const INHARMONICITY: usize = FIXED + RATIOS;
const DAMPING_TILT: usize = INHARMONICITY + 1;
const BRIGHTNESS: usize = INHARMONICITY + 2;
const PICKUP_AT: usize = INHARMONICITY + 3;
const PICKUP_MIX: usize = INHARMONICITY + 4;
const SPREAD: usize = INHARMONICITY + 5;
const GAIN: usize = INHARMONICITY + 6;
/// Every parameter.
const PARAMETERS: usize = GAIN + 1;
/// The largest pole radius, so `ρ < 1` for every setting: far enough below
/// 1 (about 450 ε) that the rounding of `ρ·cos θ`, `ρ·sin θ` and of the
/// recurrence (a few ε) cannot make a step grow `|z|`, and above every radius
/// of the bank before M2.10-8 (the largest is 1 − 6·10⁻¹³), so it changes
/// none of them.
const RHO_MAX: f64 = 1.0 - 1e-13;
/// The Nyquist fade: a mode is heard fully below `FADE_START·sr` and not at
/// all from `FADE_END·sr` (Nyquist) up, with a raised cosine between.
const FADE_START: f64 = 0.49;
const FADE_END: f64 = 0.5;
/// The non-finite guard's bounds: an infinite input sample, a recomputed
/// state component, and an output term.
const INPUT_LIMIT: f64 = 1e100;
const STATE_LIMIT: f64 = 1e150;
const TERM_LIMIT: f64 = 1e300;
/// The bounds of the mode-vector factors `1 + v`.
const FREQ_FACTOR: (f64, f64) = (0.0, 16.0);
const GAIN_FACTOR: (f64, f64) = (-16.0, 16.0);
const DECAY_FACTOR: (f64, f64) = (1.0 / 64.0, 64.0);

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

const fn modes_port(name: &'static str) -> PortDescriptor {
    port_at(
        name,
        Direction::In,
        PortShape::Vector(MAX_MODES as u32),
        1,
        PortKind::Generic,
        None,
        None,
    )
}

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.modal",
    doc: "Modal resonator bank: up to 64 two-pole resonators at the mode ratios of a string, bar, plate, membrane or bell, or 16 explicit ratios, morphing continuously between neighbouring materials; T60 per mode decay_s·ratio^(−law), amplitudes from position, with inharmonicity, damping tilt, brightness, a pickup, seeded spread and per-mode vector inputs, all modulatable at audio rate.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        modes_port("mode_freq"),
        modes_port("mode_gain"),
        modes_port("mode_decay"),
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
            Combine::Add,
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
        parameter(
            "inharmonicity",
            PortKind::Control,
            None,
            0.0,
            0.1,
            0.0,
            Combine::Add,
        ),
        parameter(
            "damping_tilt",
            PortKind::Control,
            None,
            0.0,
            10.0,
            0.0,
            Combine::Add,
        ),
        parameter(
            "brightness",
            PortKind::Control,
            None,
            -1.0,
            1.0,
            0.0,
            Combine::Add,
        ),
        parameter(
            "pickup",
            PortKind::Control,
            None,
            0.01,
            0.99,
            PICKUP,
            Combine::Add,
        ),
        parameter(
            "pickup_mix",
            PortKind::Control,
            None,
            0.0,
            1.0,
            0.0,
            Combine::Add,
        ),
        parameter(
            "spread",
            PortKind::Control,
            None,
            0.0,
            0.1,
            0.0,
            Combine::Add,
        ),
        parameter("gain", PortKind::Gain, None, 0.0, 4.0, 1.0, Combine::Add),
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
/// - `mode_freq`, `mode_gain`, `mode_decay` (signal in, vector of 64,
///   generic): per-mode factors on the computed frequency, excitation gain
///   and T60. Row `k` (0-based) gives mode `k + 1` the factor `1 + v`,
///   clamped to `[0, 16]`, `[−16, 16]` and `[1/64, 64]`: an unconnected row
///   reads 0, so it multiplies by exactly 1; to multiply by `m`, send
///   `m − 1` (−1 mutes a mode's excitation, 1 doubles it). A NaN row reads
///   as 0.
/// - `out` (signal out, audio): the sum of the modes, within ±10⁶.
///
/// **Parameters** (all combine `add`, all read on every frame)
/// - `freq` (frequency, Hz, range `[1, sr/2]`, default 220): the frequency
///   of ratio 1. Declared `[0, (2³² − 1)/2]` as `std.osc`'s; `process`
///   limits it to at least 1 Hz, and a mode at or above `0.49·sr` gets no
///   excitation; a mode that rings there (moved up by a parameter while
///   ringing) fades out of the output by `h_k` and is silent from Nyquist
///   up, so it never aliases.
/// - `material` (generic, no unit, range `[0, 5]`, default 1): the mode
///   ratios `r_k`. At an integer: 0 string (`k`), 1 bar (free–free beam),
///   2 plate (simply supported square), 3 membrane (circular), 4 bell (12
///   partials of a minor-third bell, from the hum at ½), 5 explicit
///   (`ratio_1` … `ratio_16`); the tables are in the source, to 10
///   decimals. Between two integers `i < m < i + 1` (`t = m − i`) it morphs:
///   a mode both tables have rings at `r_i + t·(r_{i+1} − r_i)`; a mode only
///   one has keeps its ratio and is heard with the weight `1 − t` (only in
///   `i`) or `t` (only in `i + 1`), so a ringing mode fades out continuously
///   where its table ends (3–4: membrane 64 modes, bell 12; 4–5: bell 12,
///   explicit 16). The amplitude law is the same for every material.
/// - `modes` (generic, integer, range `[1, 64]`, default 16):
///   `round_half_up(modes)` modes sound, and no more than the material has
///   (between two materials, the larger of the two).
/// - `decay_s` (control, seconds, range `[0.01, 60]`, default 2): the T60
///   of ratio 1.
/// - `law` (control, no unit, range `[0, 3]`, default 1): the decay law,
///   `T60_k = decay_s·r_k^(−law)`; 0 decays every mode alike, 1 makes higher
///   modes decay as many periods as ratio 1.
/// - `position` (control, fraction, range `[0.01, 0.99]`, default 0.3):
///   where the bank is struck; mode `k` (1-based, in the table's order) is
///   excited with amplitude `sin(π·k·position)`.
/// - `ratio_1` … `ratio_16` (generic, no unit, range `[0.01, 100]`, defaults
///   1 … 16): the explicit ratios.
/// - `inharmonicity` (control, `B`, range `[0, 0.1]`, default 0): stiffness,
///   `f_k ×= √((1 + B·k²)/(1 + B))`, so mode 1 stays at `freq`
///   (`n·f·√(1 + B·n²)` normalised to the first mode).
/// - `damping_tilt` (control, `τ`, range `[0, 10]`, default 0): frequency
///   dependent damping beyond `law`, `T60_k /= 1 + τ·f_k/1000 Hz`.
/// - `brightness` (control, `b`, range `[−1, 1]`, default 0): the excitation
///   of mode `k` times `k^b`.
/// - `pickup` (control, fraction, range `[0.01, 0.99]`, default 0.5) and
///   `pickup_mix` (control, range `[0, 1]`, default 0): where the bank is
///   heard, separate from `position`. Mode `k` is heard with
///   `w_k = 1 − pickup_mix + pickup_mix·sin(π·k·pickup)`; at `pickup_mix` 0
///   every mode is heard alike.
/// - `spread` (control, range `[0, 0.1]`, default 0): mode `k`'s frequency
///   times `1 + spread·j_k`, with `j_k = 2u − 1` the instance's 64 first
///   `next_f64()` draws, taken in `prepare` (deterministic for a seed and an
///   address; `reset` keeps them).
/// - `gain` (gain, linear, range `[0, 4]`, default 1): on the sum.
///
/// **Feedthrough:** direct (`in[n]` reaches `out[n]`).
///
/// **Formula** (sr the sample rate; per mode k = 1 … K; φF, φG, φD the
/// mode-vector factors; m_k the morph weight):
///
/// ```text
/// f₁   = max(freq, 1)
/// f_k  = f₁·r_k·√((1 + B·k²)/(1 + B))·(1 + spread·j_k)·φF_k,   θ_k = 2π·f_k/sr
/// ρ_k  = min(10^(−3·r_k^law·(1 + τ·f_k/1000)/(decay_s·φD_k·sr)), 1 − 10⁻¹³)
/// a_k  = sin(π·k·position)·k^b                       0 when f_k ≥ 0.49·sr
/// g_k  = a_k·(2π·f₁/sr)·φG_k
/// h_k  = 1 (f_k < 0.49·sr), ½·(1 + cos(π·(f_k/sr − 0.49)/0.01)), 0 (f_k ≥ sr/2)
/// u_k[n] = ρ_k·(cos θ_k·u_k[n−1] − sin θ_k·v_k[n−1]) + g_k·cos θ_k·in[n]
/// v_k[n] = ρ_k·(sin θ_k·u_k[n−1] + cos θ_k·v_k[n−1]) + g_k·sin θ_k·in[n]
/// out[n] = clamp(gain·Σ_k w_k·m_k·h_k·v_k[n], ±10⁶)
/// ```
///
/// At an integer `material` and every added parameter and vector row at its
/// default, every added factor is exactly 1 and the operations are those of
/// the bank before M2.10-8, so its output is bit-identical.
///
/// Each mode is a two-pole resonator in coupled form: with
/// `z_k = u_k + j·v_k` it is `z_k[n] = e^(jθ_k)·(ρ_k·z_k[n−1] + g_k·in[n])`,
/// and the output is `Im z_k`.
/// Its poles are `ρ_k·e^(±jθ_k)`, as a direct-form
/// `y[n] = 2ρ·cos θ·y[n−1] − ρ²·y[n−2]` has, but its state is rotated and
/// scaled by `ρ_k < 1` on every frame, so `|z_k|` never grows by more than
/// the input adds, **whatever θ_k and ρ_k do from frame to frame**: with
/// ρ̄ and Ḡ the largest `ρ_k` and `|g_k|` so far and X the largest `|in|`,
/// `|z_k[n]| ≤ ρ̄·|z_k[n−1]| + Ḡ·X`, so `|z_k| ≤ Ḡ·X/(1 − ρ̄)` from rest
/// (a direct form does not: alternating its coefficients every frame can
/// grow without bound). Every coefficient may change on every frame and the
/// state is never reset by it: no change of a parameter makes the output
/// jump (the frequency and decay act on the state, the excitation gain on
/// new input, the pickup, morph weight and `gain` on the output, all per
/// frame), so modulation at audio rate has no zipper steps.
///
/// **Nyquist.** A mode is excited only below `0.49·sr`. One that rings
/// when a parameter (`freq`, `inharmonicity`, `spread`, `mode_freq`, a
/// ratio, the morph) moves it to `0.49·sr` or above keeps its state but is
/// heard with the weight `h_k`, a raised cosine from 1 at `0.49·sr` to 0 at
/// Nyquist, and not at all above: a sweep across Nyquist fades the mode out
/// instead of folding it back down as an alias. A mode at or above
/// `0.49·sr` with static parameters has never been excited and its state
/// is 0, so the fade changes no output of fixed parameters, and none of the
/// bank before M2.10-8 whose modes never moved past `0.49·sr` while
/// ringing.
///
/// **Bounds.** `ρ_k < 1` for every setting: the exponent is negative
/// (`r_k^law > 0`, `1 + τ·f_k/1000 ≥ 1`, `decay_s·φD_k > 0`), and the clamp
/// to `1 − 10⁻¹³` (well clear of the rounding of `ρ·cos θ`, `ρ·sin θ` and the
/// recurrence, a few ε) leaves every radius of the bank before M2.10-8 unchanged (the
/// largest, 1 − 6·10⁻¹³, is explicit ratio 0.01, law 3, decay_s 60 at
/// 192 kHz). Ḡ ≤ 64·16·2π·(2³² − 1)/2/sr (3.13·10⁸ at 44.1 kHz, `freq` at its
/// declared maximum and `mode_freq` −1 keeping the mode below the cutoff),
/// so for a unit input a mode can reach `Ḡ/(1 − ρ̄)` ≈ 7.7·10¹⁵ at every
/// rate (T60 = 60 s × 64, law 0) and the sum, before the clamp,
/// 4·64·7.7·10¹⁵ ≈ 2·10¹⁸: no range of the parameters keeps the output
/// within the fault limit (`docs/semantics.md` §7) for every input, so the
/// output is clamped to ±10⁶ instead, which changes no value the limit
/// accepts.
///
/// **Non-finite values** (an input sample from scaled sources can be any
/// finite value, ±∞ or NaN, and a parameter NaN): an infinite input sample
/// reads as ±10¹⁰⁰ and a NaN one as 0; a NaN `material` reads as 0 (string,
/// as `round_half_up(NaN) as i64` did); a NaN added parameter reads as its
/// default; a mode whose coefficients are not finite (a NaN `decay_s`,
/// `law`, `position` or ratio) is silent; a state component that would not
/// be finite is recomputed from the state clamped to ±10¹⁵⁰ and the input to
/// ±10¹⁰⁰, and clamped to ±10¹⁵⁰; a sum that would not be finite is taken
/// over the terms clamped to ±10³⁰⁰. Each acts only on a frame where the bank
/// before M2.10-8 produced a value that was not finite and reached its
/// output, on that frame or the next one the mode sounded, which the engine
/// faults: so the output is unchanged before such a fault, and the state and
/// output are always finite.
///
/// An impulse into mode k rings as
/// `a_k·(2π·f₁/sr)·ρ_k^n·sin((n + 1)·θ_k)`: a damped sinusoid at exactly
/// `f_k` with a T60 of `decay_s·r_k^(−law)` seconds. **The input is a force
/// density**: an excitation of area `J = Σ in[n]/sr` (unit·seconds), short
/// against the periods, rings every mode at amplitude `a_k·2π·f₁·J`, at any
/// sample rate. (Without the factor `2π·f₁/sr` the same excitation, which has
/// twice the frames at twice the rate, would ring twice as loud.) `pow`,
/// `sin`, `cos` and `sqrt` are `libm`'s. Starting state 0.
///
/// **Coefficients.** On a frame where a parameter differs (bitwise) from the
/// frame before, or a mode's vector rows do, the modes concerned are
/// visited: every sounding mode for a change of `freq`, `material`,
/// `decay_s`, `law`, `position` or an added parameter but `gain` (which acts
/// on the sum only and visits none); none for `modes`, which changes only
/// the count, but every mode that sounds again (it may have missed changes
/// while dormant); mode k alone for `ratio_k`, and only while the explicit
/// ratios sound; mode k alone for its vector rows. A visit compares the
/// mode's own inputs and recomputes `f_k` (a `sqrt`) only if `f₁`, `r_k`,
/// `inharmonicity`, `spread` or `φF_k` changed, the four coefficients only
/// if θ_k, ρ_k, `a_k`, the density or `φG_k` changed, and θ_k (`cos`, `sin`)
/// only if `f_k` changed, ρ_k only if `r_k`, `law`, `decay_s`, `φD_k`, `τ` or (while `τ ≠ 0`) `f_k`
/// did (one `pow`, and a second, `r_k^law`, only if `r_k` or `law` did),
/// `a_k` only if `position`, `brightness` or the cutoff did (a `sin`, and a
/// `pow`, `k^b`, only if `brightness` did), and the output weight only if
/// `pickup`, `pickup_mix`, `m_k` or `h_k` did (`h_k`, a `cos` inside the
/// fade band, with θ_k). A mode's state is kept when its coefficients
/// change. A mode outside the sounding `K` (fewer `modes`, or a morph) is
/// neither advanced nor heard: its state stays frozen where it stopped,
/// without decaying, and when the mode returns it resumes from there (the
/// morph weight brings it back from 0, so the output stays continuous).
/// Each state is flushed to exactly 0 below 1e-9
/// (`docs/dsp-conventions.md`, "Denormals").
/// Work per frame is proportional to the sounding modes, at most 64, plus a
/// bounded recomputation for the modes visited.
#[derive(Clone, Debug)]
pub struct Modal {
    sample_rate: f64,
    /// The parameters of the last frame, as read (`None`: none yet).
    params: Option<[f64; PARAMETERS]>,
    /// From the parameters: the modes that sound, `2π·f₁/sr`, and the
    /// material as the table below and the step `t` towards the next one.
    count: usize,
    density: f64,
    table: usize,
    step: f64,
    /// `j_k`, drawn in `prepare`.
    jitter: [f64; MAX_MODES],
    modes: [Mode; MAX_MODES],
    /// `(u_k, v_k)` of the last frame.
    state: [(f64, f64); MAX_MODES],
    /// Test-only: how often the guard acted and how many frames recomputed
    /// a transcendental function, and a switch that visits every mode from
    /// scratch on every frame (the oracle the cache is checked against).
    #[cfg(test)]
    guards: usize,
    #[cfg(test)]
    recomputes: usize,
    #[cfg(test)]
    visits: usize,
    #[cfg(test)]
    rebuilds: usize,
    #[cfg(test)]
    force: bool,
}

/// One mode's coefficients and what they were computed from.
#[derive(Clone, Copy, Debug, Default)]
struct Mode {
    /// `ρ·cos θ`, `ρ·sin θ`, `g·cos θ`, `g·sin θ`.
    rc: f64,
    rs: f64,
    gc: f64,
    gs: f64,
    /// The output weight `w_k·m_k`.
    weight: f64,
    /// Whether the fields below describe the coefficients (false after
    /// `reset`).
    valid: bool,
    /// The vector rows' factors `(φF, φG, φD)` of the last visit.
    factors: [f64; 3],
    /// `f_k`, and its `cos θ`, `sin θ` and Nyquist fade `h_k`.
    f: f64,
    cos: f64,
    sin: f64,
    fade: f64,
    /// `(f₁, r_k, B, spread, φF)` of `f_k`.
    f_key: [f64; 5],
    /// `(density, φG)` of `gc`, `gs`.
    g_key: [f64; 2],
    /// `(r_k, law)`, and `r_k^law`.
    power_key: [f64; 2],
    power: f64,
    /// `(r_k, law, decay_s, φD, τ, f_k or 0)`, and `ρ_k`.
    rho_key: [f64; 6],
    rho: f64,
    /// `brightness`, and `k^b`.
    bright_key: f64,
    bright: f64,
    /// `(position, brightness, cutoff)`, and `a_k`.
    amplitude_key: [f64; 3],
    amplitude: f64,
    /// `(pickup, pickup_mix, m_k, h_k)` of the weight.
    weight_key: [f64; 4],
}

impl Default for Modal {
    fn default() -> Self {
        Modal {
            sample_rate: 0.0,
            params: None,
            count: 0,
            density: 0.0,
            table: 0,
            step: 0.0,
            jitter: [0.0; MAX_MODES],
            modes: [Mode::default(); MAX_MODES],
            state: [(0.0, 0.0); MAX_MODES],
            #[cfg(test)]
            guards: 0,
            #[cfg(test)]
            recomputes: 0,
            #[cfg(test)]
            visits: 0,
            #[cfg(test)]
            rebuilds: 0,
            #[cfg(test)]
            force: false,
        }
    }
}

/// The ratio of mode `k` (0-based) of table `table` (0 string … 5 explicit),
/// with the explicit ratios `explicit`; `None` past the table's last mode.
fn table_ratio(table: usize, k: usize, explicit: &[f64]) -> Option<f64> {
    match table {
        0 => (k < MAX_MODES).then_some(k as f64 + 1.0),
        1 => BAR_RATIOS.get(k).copied(),
        2 => PLATE_RATIOS.get(k).copied(),
        3 => MEMBRANE_RATIOS.get(k).copied(),
        4 => BELL_RATIOS.get(k).copied(),
        _ => explicit.get(k).copied(),
    }
}

/// The modes of table `table`.
fn table_len(table: usize) -> usize {
    match table {
        0..=3 => MAX_MODES,
        4 => BELL_RATIOS.len(),
        _ => RATIOS,
    }
}

/// The ratio of mode `k` (0-based) of the integer `material`, with the
/// explicit ratios `explicit`; `None` past the material's last mode.
pub(crate) fn mode_ratio(material: f64, k: usize, explicit: &[f64]) -> Option<f64> {
    table_ratio(
        round_half_up(material).clamp(0.0, 5.0) as usize,
        k,
        explicit,
    )
}

/// The Nyquist fade `h(f)` of a mode at `f` Hz, at `sample_rate`: 1 below
/// `0.49·sr` (exactly), 0 from `0.5·sr` up (exactly), and
/// `½·(1 + cos(π·(f/sr − 0.49)/0.01))` between, so it falls smoothly (with a
/// continuous slope) as the mode approaches Nyquist. A NaN `f` gives NaN.
pub(crate) fn nyquist_fade(f: f64, sample_rate: f64) -> f64 {
    if f < FADE_START * sample_rate {
        1.0
    } else if f >= FADE_END * sample_rate {
        0.0
    } else {
        let x = (f / sample_rate - FADE_START) / (FADE_END - FADE_START);
        0.5 * (1.0 + libm::cos(std::f64::consts::PI * x))
    }
}

/// Whether two keys are the same bits (a NaN equals itself).
fn same<const N: usize>(a: &[f64; N], b: &[f64; N]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

/// The factor `1 + v` of vector row `k` on frame `n` of a call of `frames`,
/// clamped to `(lo, hi)`; a missing row reads 0, as does a NaN.
fn factor(rows: &[f64], k: usize, frames: usize, n: usize, (lo, hi): (f64, f64)) -> f64 {
    let v = rows.get(k * frames + n).copied().unwrap_or(0.0);
    let v = if v.is_nan() { 0.0 } else { v };
    (1.0 + v).clamp(lo, hi)
}

/// The value of a guarded input sample: ±∞ reads as ±[`INPUT_LIMIT`], NaN
/// as 0.
fn finite_input(x: f64) -> f64 {
    match x {
        x if x.is_finite() => x,
        x if x.is_nan() => 0.0,
        x => INPUT_LIMIT.copysign(x),
    }
}

impl Modal {
    /// The parameters of frame `n`, with a NaN `material` read as 0 and a
    /// NaN added parameter as its default.
    fn read(lanes: &[&[f64]; PARAMETERS], n: usize) -> [f64; PARAMETERS] {
        let defaults = DESCRIPTOR.parameters;
        std::array::from_fn(|i| {
            let default = defaults.get(i).map_or(0.0, |d| d.default);
            let value = at(lanes.get(i).copied().unwrap_or(&[]), n, default);
            match i {
                1 if value.is_nan() => 0.0,
                1 => value.clamp(0.0, 5.0),
                i if i >= INHARMONICITY && value.is_nan() => default,
                _ => value,
            }
        })
    }

    /// What follows from the parameters alone: the sounding modes, the
    /// density factor and the material's tables.
    fn globals(&mut self, p: &[f64; PARAMETERS]) {
        let freq = p[0].max(1.0);
        self.density = std::f64::consts::TAU * freq / self.sample_rate;
        let wanted = (round_half_up(p[2]).max(1.0) as usize).min(MAX_MODES);
        let lower = libm::floor(p[1]);
        self.table = lower as usize;
        self.step = p[1] - lower;
        let available = match self.step == 0.0 {
            true => table_len(self.table),
            false => table_len(self.table).max(table_len(self.table + 1)),
        };
        self.count = wanted.min(available);
    }

    /// The ratio `r_k` and morph weight `m_k` of mode `k` (0-based).
    fn ratio(&self, p: &[f64; PARAMETERS], k: usize) -> (f64, f64) {
        let explicit = p.get(FIXED..FIXED + RATIOS).unwrap_or(&[]);
        let t = self.step;
        if t == 0.0 {
            return (mode_ratio(p[1], k, explicit).unwrap_or(1.0), 1.0);
        }
        let a = table_ratio(self.table, k, explicit);
        let b = table_ratio(self.table + 1, k, explicit);
        match (a, b) {
            (Some(a), Some(b)) => (a + t * (b - a), 1.0),
            (Some(a), None) => (a, 1.0 - t),
            (None, Some(b)) => (b, t),
            (None, None) => (1.0, 0.0),
        }
    }

    /// Recomputes what changed of mode `k`'s coefficients, given the
    /// parameters `p` and its vector rows' factors; true if a transcendental
    /// function was evaluated.
    fn visit(&mut self, p: &[f64; PARAMETERS], k: usize, factors: [f64; 3]) -> bool {
        use std::f64::consts::{PI, TAU};
        #[cfg(test)]
        {
            self.visits += 1;
        }
        let (r, m) = self.ratio(p, k);
        let [freq_factor, gain_factor, decay_factor] = factors;
        let kk = k as f64 + 1.0;
        let jitter = self.jitter.get(k).copied().unwrap_or(0.0);
        let sr = self.sample_rate;
        let density = self.density;
        let Some(mode) = self.modes.get_mut(k) else {
            return false;
        };
        let fresh = !mode.valid;
        let mut recomputed = false;
        // f_k from (f₁, r_k, B, spread, φF): the root only when they moved.
        let f_key = [p[0].max(1.0), r, p[INHARMONICITY], p[SPREAD], freq_factor];
        let f = if !fresh && same(&f_key, &mode.f_key) {
            mode.f
        } else {
            let [freq, r, b, spread, freq_factor] = f_key;
            // At B = 0 the factor is √(1/1) = 1 exactly; skip the root.
            let stiffness = match b == 0.0 {
                true => 1.0,
                false => libm::sqrt((1.0 + b * kk * kk) / (1.0 + b)),
            };
            mode.f_key = f_key;
            freq * r * stiffness * (1.0 + spread * jitter) * freq_factor
        };
        let theta_moved = fresh || f.to_bits() != mode.f.to_bits();
        if theta_moved {
            let theta = TAU * f / sr;
            (mode.f, mode.cos, mode.sin) = (f, libm::cos(theta), libm::sin(theta));
            mode.fade = nyquist_fade(f, sr);
            recomputed = true;
        }
        let mut moved = theta_moved;
        let tau = p[DAMPING_TILT];
        let rho_key = [
            r,
            p[4],
            p[3],
            decay_factor,
            tau,
            if tau == 0.0 { 0.0 } else { f },
        ];
        if fresh || !same(&rho_key, &mode.rho_key) {
            let [r, law, decay_s, ..] = rho_key;
            // `r_k^law` only when r_k or law moved (not for decay_s, φD, τ).
            if fresh || !same(&[r, law], &mode.power_key) {
                mode.power = libm::pow(r, law);
                mode.power_key = [r, law];
            }
            let tilt = if tau == 0.0 {
                1.0
            } else {
                1.0 + tau * f / 1000.0
            };
            let rho = libm::pow(
                10.0,
                -3.0 * mode.power * tilt / (decay_s * decay_factor * sr),
            );
            // Not `min`, which would turn a NaN into a radius.
            mode.rho = if rho > RHO_MAX { RHO_MAX } else { rho };
            mode.rho_key = rho_key;
            recomputed = true;
            moved = true;
        }
        let cutoff = if f < 0.49 * sr { 1.0 } else { 0.0 };
        let amplitude_key = [p[5], p[BRIGHTNESS], cutoff];
        if fresh || !same(&amplitude_key, &mode.amplitude_key) {
            let [position, brightness, _] = amplitude_key;
            // `k^b` only when brightness moved (not for position).
            if fresh || brightness.to_bits() != mode.bright_key.to_bits() {
                mode.bright = libm::pow(kk, brightness);
                mode.bright_key = brightness;
            }
            mode.amplitude = if cutoff == 1.0 {
                libm::sin(PI * kk * position) * mode.bright
            } else {
                0.0
            };
            mode.amplitude_key = amplitude_key;
            recomputed = true;
            moved = true;
        }
        let weight_key = [p[PICKUP_AT], p[PICKUP_MIX], m, mode.fade];
        if fresh || !same(&weight_key, &mode.weight_key) {
            let [pickup, mix, m, fade] = weight_key;
            let heard = if mix == 0.0 {
                0.0
            } else {
                libm::sin(PI * kk * pickup)
            };
            let weight = (1.0 - mix + mix * heard) * m * fade;
            mode.weight = if weight.is_finite() { weight } else { 0.0 };
            mode.weight_key = weight_key;
            recomputed = true;
        }
        // The four coefficients only when θ, ρ, a_k, the density or φG moved
        // (the weight is applied on the output and needs none of them).
        let g_key = [density, gain_factor];
        let rebuilt = moved || !same(&g_key, &mode.g_key);
        if rebuilt {
            let g = mode.amplitude * density * gain_factor;
            let (rho, cos, sin) = (mode.rho, mode.cos, mode.sin);
            let (rc, rs, gc, gs) = (rho * cos, rho * sin, g * cos, g * sin);
            (mode.rc, mode.rs, mode.gc, mode.gs) =
                if rc.is_finite() && rs.is_finite() && gc.is_finite() && gs.is_finite() {
                    (rc, rs, gc, gs)
                } else {
                    (0.0, 0.0, 0.0, 0.0)
                };
            mode.g_key = g_key;
        }
        mode.factors = factors;
        mode.valid = true;
        #[cfg(test)]
        {
            self.rebuilds += usize::from(rebuilt);
        }
        recomputed
    }
}

impl DirectPrimitive for Modal {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        let mut stream = ctx.rng.clone();
        for j in &mut self.jitter {
            *j = 2.0 * stream.next_f64() - 1.0;
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.params = None;
        self.count = 0;
        for mode in &mut self.modes {
            mode.valid = false;
        }
        self.state = [(0.0, 0.0); MAX_MODES];
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let frames = inputs.frames();
        let lanes: [&[f64]; PARAMETERS] = std::array::from_fn(|i| inputs.parameter(i));
        let freq_rows = inputs.rows(1, MAX_MODES);
        let gain_rows = inputs.rows(1 + MAX_MODES, MAX_MODES);
        let decay_rows = inputs.rows(1 + 2 * MAX_MODES, MAX_MODES);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let p = Self::read(&lanes, n);
            // What changed: `gain` acts on the sum only, `modes` on the count
            // only, `ratio_k` on mode k only (and only while the explicit
            // ratios sound), every other parameter on every mode.
            let (global, count_changed, ratios) = match &self.params {
                None => (true, true, 0u32),
                Some(q) => q.iter().zip(&p).enumerate().fold(
                    (false, false, 0u32),
                    |(global, count, ratios), (i, (a, b))| match a.to_bits() == b.to_bits() {
                        true => (global, count, ratios),
                        false if i == GAIN => (global, count, ratios),
                        false if i == 2 => (global, true, ratios),
                        false if (FIXED..FIXED + RATIOS).contains(&i) => {
                            (global, count, ratios | 1 << (i - FIXED))
                        }
                        false => (true, count, ratios),
                    },
                ),
            };
            self.params = Some(p);
            let before = self.count;
            if global || count_changed {
                self.globals(&p);
            }
            let explicit = match self.step == 0.0 {
                true => self.table == 5,
                false => self.table == 4,
            };
            let ratios = if explicit { ratios } else { 0 };
            #[cfg(test)]
            let force = self.force;
            #[cfg(not(test))]
            let force = false;
            let mut recomputed = false;
            for k in 0..self.count {
                let factors = [
                    factor(freq_rows, k, frames, n, FREQ_FACTOR),
                    factor(gain_rows, k, frames, n, GAIN_FACTOR),
                    factor(decay_rows, k, frames, n, DECAY_FACTOR),
                ];
                let Some(mode) = self.modes.get_mut(k) else {
                    break;
                };
                if force {
                    mode.valid = false;
                }
                let ratio_changed = k < RATIOS && ratios >> k & 1 == 1;
                // A mode that sounds again may have missed changes while it
                // was dormant; its visit compares them and recomputes only
                // what they touched.
                let returning = k >= before;
                if global
                    || ratio_changed
                    || returning
                    || !mode.valid
                    || !same(&factors, &mode.factors)
                {
                    recomputed |= self.visit(&p, k, factors);
                }
            }
            #[cfg(test)]
            {
                self.recomputes += usize::from(recomputed);
            }
            let _ = recomputed;
            let input = finite_input(at(x, n, 0.0));
            let mut sum = 0.0;
            let modes = self.modes.iter().zip(self.state.iter_mut());
            for (m, (u, v)) in modes.take(self.count) {
                let mut next_u = flush(m.rc * *u - m.rs * *v + m.gc * input);
                let mut next_v = flush(m.rs * *u + m.rc * *v + m.gs * input);
                if !(next_u.is_finite() && next_v.is_finite()) {
                    let (lu, lv) = (
                        u.clamp(-STATE_LIMIT, STATE_LIMIT),
                        v.clamp(-STATE_LIMIT, STATE_LIMIT),
                    );
                    let li = input.clamp(-INPUT_LIMIT, INPUT_LIMIT);
                    if !next_u.is_finite() {
                        next_u =
                            (m.rc * lu - m.rs * lv + m.gc * li).clamp(-STATE_LIMIT, STATE_LIMIT);
                    }
                    if !next_v.is_finite() {
                        next_v =
                            (m.rs * lu + m.rc * lv + m.gs * li).clamp(-STATE_LIMIT, STATE_LIMIT);
                    }
                    #[cfg(test)]
                    {
                        self.guards += 1;
                    }
                }
                *u = next_u;
                *v = next_v;
                sum += m.weight * *v;
            }
            let gain = p[GAIN];
            let mut y = gain * sum;
            if !y.is_finite() {
                let terms = self.modes.iter().zip(self.state.iter()).take(self.count);
                let sum: f64 = terms
                    .map(|(m, (_, v))| (m.weight * v).clamp(-TERM_LIMIT, TERM_LIMIT))
                    .sum();
                y = gain * sum;
                #[cfg(test)]
                {
                    self.guards += 1;
                }
            }
            let y = bounded(y);
            *out = if y.is_nan() { 0.0 } else { y };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::string::measure::partial;
    use crate::testing::{Timeline, context, render, stream};
    use signal_core::RngStream;

    /// The vector rows: `mode_freq`, `mode_gain`, `mode_decay`.
    const ROWS: usize = 3 * MAX_MODES;

    /// The bank's response to a unit impulse on frame 0.
    fn impulse(params: &[(usize, f64)], sample_rate: f64, frames: usize) -> Vec<f64> {
        let mut modal = bank(sample_rate);
        let mut x = vec![0.0; frames];
        x[0] = 1.0;
        run(&mut modal, &lanes(frames, params), &x, &[], 256)
    }

    /// A bank prepared at `sample_rate` (seed 0).
    fn bank(sample_rate: f64) -> Modal {
        let mut modal = Modal::default();
        modal.prepare(&context(sample_rate, 256, 0));
        modal
    }

    /// Every parameter's lane at its default, with `set` held.
    fn lanes(frames: usize, set: &[(usize, f64)]) -> Vec<Vec<f64>> {
        let mut lanes: Vec<Vec<f64>> = DESCRIPTOR
            .parameters
            .iter()
            .map(|p| vec![p.default; frames])
            .collect();
        for &(i, value) in set {
            lanes[i] = vec![value; frames];
        }
        lanes
    }

    /// Renders `x` through `modal` with `lanes` and the vector `rows` (all
    /// 192, or none), in calls of `block`.
    fn run(
        modal: &mut Modal,
        lanes: &[Vec<f64>],
        x: &[f64],
        rows: &[Vec<f64>],
        block: usize,
    ) -> Vec<f64> {
        let mut signals: Vec<&[f64]> = vec![x];
        signals.extend(rows.iter().map(Vec::as_slice));
        let parameters: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
        let timeline = Timeline {
            signals: &signals,
            parameters: &parameters,
            triggers: &[],
        };
        render(modal, &timeline, x.len(), block).signals[0].clone()
    }

    /// One frame: input `x`, parameters `p`, vector rows `rows` (empty: none).
    fn step(modal: &mut Modal, p: &[f64], x: f64, rows: &[f64]) -> f64 {
        let mut signals = vec![x];
        signals.extend_from_slice(rows);
        let mut out = [f64::NAN];
        modal.process(
            &Inputs::new(1, &signals, p, &[], &[]),
            &mut Outputs::new(1, &mut out, &mut []),
        );
        out[0]
    }

    fn defaults() -> Vec<f64> {
        DESCRIPTOR.parameters.iter().map(|p| p.default).collect()
    }

    /// Sets the coefficients for the parameters `p`, every mode visited.
    fn coefficients(modal: &mut Modal, p: &[f64; PARAMETERS]) {
        modal.params = Some(*p);
        modal.globals(p);
        for k in 0..modal.count {
            modal.visit(p, k, [1.0; 3]);
        }
    }

    fn uniform(s: &mut RngStream, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * s.next_f64()
    }

    /// The bank as it was before M2.10-8 (M2.8), verbatim but for one
    /// switch: with `fade` its modes are heard with the Nyquist fade `h_k`
    /// (the one intended change for parameters that move a ringing mode to
    /// `0.49·sr` or above); without it, verbatim. The reference for
    /// bit-identity.
    struct Legacy {
        sample_rate: f64,
        fade: bool,
        computed: [f64; FIXED + RATIOS],
        count: usize,
        modes: [(f64, f64, f64, f64); MAX_MODES],
        heard: [f64; MAX_MODES],
        state: [(f64, f64); MAX_MODES],
    }

    fn legacy_ratio(material: f64, k: usize, explicit: &[f64]) -> Option<f64> {
        match round_half_up(material) as i64 {
            0 => (k < MAX_MODES).then_some(k as f64 + 1.0),
            1 => BAR_RATIOS.get(k).copied(),
            2 => PLATE_RATIOS.get(k).copied(),
            3 => MEMBRANE_RATIOS.get(k).copied(),
            4 => BELL_RATIOS.get(k).copied(),
            _ => explicit.get(k).copied(),
        }
    }

    impl Legacy {
        fn new(sample_rate: f64) -> Legacy {
            Legacy {
                sample_rate,
                fade: true,
                computed: [f64::NAN; FIXED + RATIOS],
                count: 0,
                modes: [(0.0, 0.0, 0.0, 0.0); MAX_MODES],
                heard: [1.0; MAX_MODES],
                state: [(0.0, 0.0); MAX_MODES],
            }
        }

        /// The bank before M2.10-8 exactly, without the fade.
        fn verbatim(sample_rate: f64) -> Legacy {
            Legacy {
                fade: false,
                ..Legacy::new(sample_rate)
            }
        }

        fn update(&mut self, p: &[f64; FIXED + RATIOS]) {
            let [freq, material, modes, decay_s, law, position, ..] = *p;
            let explicit = p.get(FIXED..).unwrap_or(&[]);
            let freq = freq.max(1.0);
            let wanted = (round_half_up(modes).max(1.0) as usize).min(MAX_MODES);
            let sr = self.sample_rate;
            let density = std::f64::consts::TAU * freq / sr;
            self.count = 0;
            for k in 0..wanted {
                let Some(r) = legacy_ratio(material, k, explicit) else {
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
                let (cos, sin) = (libm::cos(theta), libm::sin(theta));
                let g = amplitude * density;
                self.modes[k] = (rho * cos, rho * sin, g * cos, g * sin);
                self.heard[k] = if self.fade { nyquist_fade(f, sr) } else { 1.0 };
                self.count = k + 1;
            }
        }

        /// One frame with the first 22 parameters of `p`.
        fn frame(&mut self, p: &[f64], input: f64) -> f64 {
            let mut wanted = [0.0; FIXED + RATIOS];
            wanted.copy_from_slice(&p[..FIXED + RATIOS]);
            if wanted != self.computed {
                self.computed = wanted;
                self.update(&wanted);
            }
            let mut sum = 0.0;
            let modes = self
                .modes
                .iter()
                .zip(&self.heard)
                .zip(self.state.iter_mut());
            for ((&(rc, rs, gc, gs), &heard), (u, v)) in modes.take(self.count) {
                let next_u = flush(rc * *u - rs * *v + gc * input);
                *v = flush(rs * *u + rc * *v + gs * input);
                *u = next_u;
                // `1.0 * v` is `v`: without the fade, the verbatim sum.
                sum += heard * *v;
            }
            sum
        }
    }

    /// Random settings of the bank's parameters before M2.10-8 (an integer
    /// material), the added ones at their defaults.
    fn legacy_setting(s: &mut RngStream) -> Vec<f64> {
        let mut p = defaults();
        p[0] = uniform(s, 20.0, 6000.0);
        p[1] = libm::floor(uniform(s, 0.0, 6.0)).min(5.0);
        p[2] = libm::floor(uniform(s, 1.0, 65.0));
        p[3] = uniform(s, 0.01, 60.0);
        p[4] = uniform(s, 0.0, 3.0);
        p[5] = uniform(s, 0.01, 0.99);
        for r in &mut p[FIXED..FIXED + RATIOS] {
            *r = uniform(s, 0.01, 100.0);
        }
        p
    }

    /// T-M1 (AC2, AC11): with every added parameter at its default and no
    /// vector rows (or rows of 0), the bank is bit-identical to the bank
    /// before M2.10-8 for every integer material: static settings, settings
    /// that step every few hundred frames and settings that change on every
    /// frame, in calls of 1, 64 and 256 frames; and its guard never acts
    /// (T-M9(4)). Where the settings move a ringing mode to `0.49·sr` or
    /// above, the reference applies the Nyquist fade; with static settings
    /// the fade changes nothing (the reference with it equals the verbatim
    /// bank).
    #[test]
    fn integer_materials_are_bit_identical_to_the_bank_before() {
        let frames = 3000;
        let mut s = stream(11);
        for case in 0..18 {
            let sample_rate = [44_100.0, 48_000.0, 96_000.0][case % 3];
            // 0–5: static, one per material; 6–11 step; 12–17 every frame.
            let mut lanes: Vec<Vec<f64>> = vec![Vec::new(); PARAMETERS];
            let mut p = legacy_setting(&mut s);
            if case < 6 {
                p[1] = case as f64;
            }
            for n in 0..frames {
                if ((6..12).contains(&case) && n % 400 == 0) || case >= 12 {
                    p = legacy_setting(&mut s);
                }
                for (lane, &v) in lanes.iter_mut().zip(&p) {
                    lane.push(v);
                }
            }
            let x: Vec<f64> = (0..frames)
                .map(|n| match n {
                    0..200 => uniform(&mut s, -1.0, 1.0),
                    n if n % 700 == 0 => 1.0,
                    _ => 0.0,
                })
                .collect();
            let mut legacy = Legacy::new(sample_rate);
            let mut verbatim = Legacy::verbatim(sample_rate);
            let mut crossed = false;
            let expected: Vec<f64> = (0..frames)
                .map(|n| {
                    let p: Vec<f64> = lanes.iter().map(|l| l[n]).collect();
                    let y = legacy.frame(&p, x[n]);
                    let old = verbatim.frame(&p, x[n]);
                    crossed |= y.to_bits() != old.to_bits();
                    y
                })
                .collect();
            // The fade acts only where a ringing mode was moved to 0.49·sr or
            // above, which fixed parameters never do.
            if case < 6 {
                assert!(!crossed, "case {case}: the fade changed a static bank");
            }
            for (block, rows) in [(1, 0), (64, 0), (256, ROWS)] {
                let mut modal = bank(sample_rate);
                let rows = vec![vec![0.0; frames]; rows];
                let y = run(&mut modal, &lanes, &x, &rows, block);
                for n in 0..frames {
                    assert_eq!(
                        y[n].to_bits(),
                        expected[n].to_bits(),
                        "case {case}, block {block}, frame {n}: {} vs {}",
                        y[n],
                        expected[n]
                    );
                }
                assert_eq!(modal.guards, 0, "case {case}: the guard acted");
            }
        }
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
            let mut modal = bank(sample_rate);
            let frames = sample_rate as usize;
            let lanes = lanes(
                frames,
                &[(0, freq), (1, 0.0), (2, 3.0), (3, 2.0), (5, position)],
            );
            let width = 0.001 * sample_rate;
            let x: Vec<f64> = (0..frames)
                .map(|n| match (n as f64) < width {
                    true => 0.5 - 0.5 * libm::cos(std::f64::consts::TAU * n as f64 / width),
                    false => 0.0,
                })
                .collect();
            let area: f64 = x.iter().sum::<f64>() / sample_rate;
            let y = run(&mut modal, &lanes, &x, &[], 256);
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

    /// Modes at or above 0.49·sr get no excitation, and a material has no
    /// more modes than its table.
    #[test]
    fn modes_stop_below_nyquist_and_at_the_table() {
        let mut modal = bank(48_000.0);
        let mut p = [0.0; PARAMETERS];
        p.copy_from_slice(&defaults());
        p[0] = 1000.0;
        p[2] = 64.0;
        coefficients(&mut modal, &p);
        assert_eq!(modal.count, 64);
        // Bar mode 6 is 18.6 kHz, mode 7 is 24.8 kHz ≥ 23.52 kHz.
        assert!(modal.modes[5].gs != 0.0);
        assert!(modal.modes[6..].iter().all(|m| m.gc == 0.0 && m.gs == 0.0));
        p[1] = 4.0;
        coefficients(&mut modal, &p);
        assert_eq!(modal.count, 12);
        p[1] = 5.0;
        coefficients(&mut modal, &p);
        assert_eq!(modal.count, 16);
        // Between the bell and the explicit ratios, the larger table.
        p[1] = 4.5;
        coefficients(&mut modal, &p);
        assert_eq!(modal.count, 16);
        p[1] = 3.5;
        coefficients(&mut modal, &p);
        assert_eq!(modal.count, 64);
    }

    /// T-M2 (AC3, AC10): with `inharmonicity` B, mode k of a string rings
    /// within 0.1 % of `freq·k·√((1 + B·k²)/(1 + B))` (mode 1 at `freq`), and
    /// its T60 is still `decay_s·k^(−law)`; with `damping_tilt` τ the T60 is
    /// `decay_s·k^(−law)/(1 + τ·f_k/1000)` within 1 %; with `spread` s, mode
    /// k rings at `freq·k·(1 + s·j_k)`, `j_k` replayed from the instance's
    /// stream.
    #[test]
    fn inharmonicity_damping_tilt_and_spread_follow_their_formulas() {
        let (sample_rate, freq, decay_s, law) = (48_000.0, 110.0, 3.0, 0.5);
        let mut draws = stream(0);
        let jitter: Vec<f64> = (0..MAX_MODES)
            .map(|_| 2.0 * draws.next_f64() - 1.0)
            .collect();
        let cases: [(f64, f64, f64); 4] = [
            (1e-3, 0.0, 0.0),
            (1e-2, 0.0, 0.0),
            (0.0, 2.0, 0.0),
            (0.0, 0.0, 0.05),
        ];
        for (b, tau, spread) in cases {
            let params = [
                (0, freq),
                (1, 0.0),
                (2, 8.0),
                (3, decay_s),
                (4, law),
                (5, 0.37),
                (INHARMONICITY, b),
                (DAMPING_TILT, tau),
                (SPREAD, spread),
            ];
            let y = impulse(&params, sample_rate, 2 * 48_000);
            for k in 1..=8 {
                let kk = k as f64;
                let f = freq
                    * kk
                    * libm::sqrt((1.0 + b * kk * kk) / (1.0 + b))
                    * (1.0 + spread * jitter[k - 1]);
                let t60 = decay_s * libm::pow(kk, -law) / (1.0 + tau * f / 1000.0);
                let span = ((t60 / 3.0).min(0.5) * sample_rate) as usize;
                let (measured, measured_t60) = partial(&y, f, 0, span, span, sample_rate);
                println!(
                    "B {b}, τ {tau}, spread {spread}, mode {k}: {measured:.4} Hz (expected \
                     {f:.4}), T60 {measured_t60:.4} s (expected {t60:.4})"
                );
                assert!((measured / f - 1.0).abs() < 1e-3, "frequency");
                assert!((measured_t60 / t60 - 1.0).abs() < 0.01, "T60");
            }
        }
    }

    /// T-M2 (AC3, AC4): `gain`, the pickup and the mode vectors act exactly:
    /// `gain` scales the output; at `pickup_mix` 1 a single mode is heard at
    /// `sin(π·pickup)`; `brightness` multiplies mode k's excitation by
    /// `k^b`; a `mode_gain` row of 0 leaves the bank unchanged, 1 doubles the
    /// mode and −1 mutes it; `mode_freq` 1 doubles the frequency and
    /// `mode_decay` 1 the T60.
    #[test]
    fn gain_pickup_brightness_and_mode_vectors_act_exactly() {
        let (sample_rate, frames) = (48_000.0, 48_000);
        let base = [(0, 300.0), (1, 0.0), (2, 1.0), (5, 0.5)];
        let mut x = vec![0.0; frames];
        x[0] = 1.0;
        let render_with = |set: &[(usize, f64)], row: Option<(usize, f64)>| {
            let mut all = base.to_vec();
            all.extend_from_slice(set);
            let mut modal = bank(sample_rate);
            let rows = match row {
                Some((r, v)) => {
                    let mut rows = vec![vec![0.0; frames]; ROWS];
                    rows[r] = vec![v; frames];
                    rows
                }
                None => Vec::new(),
            };
            run(&mut modal, &lanes(frames, &all), &x, &rows, 64)
        };
        let plain = render_with(&[], None);
        let scaled = render_with(&[(GAIN, 2.5)], None);
        assert!(
            plain
                .iter()
                .zip(&scaled)
                .all(|(a, b)| b.to_bits() == (2.5 * a).to_bits())
        );
        let heard = render_with(&[(PICKUP_MIX, 1.0), (PICKUP_AT, 0.3)], None);
        let w = libm::sin(std::f64::consts::PI * 0.3);
        assert!(
            plain
                .iter()
                .zip(&heard)
                .all(|(a, b)| b.to_bits() == (w * a).to_bits())
        );
        assert_eq!(render_with(&[], Some((MAX_MODES, 0.0))), plain);
        let doubled = render_with(&[], Some((MAX_MODES, 1.0)));
        assert!(
            plain
                .iter()
                .zip(&doubled)
                .all(|(a, b)| b.to_bits() == (2.0 * a).to_bits())
        );
        assert!(
            render_with(&[], Some((MAX_MODES, -1.0)))
                .iter()
                .all(|&y| y == 0.0)
        );
        let span = 12_000;
        let octave = render_with(&[], Some((0, 1.0)));
        let (f, _) = partial(&octave, 600.0, 0, span, span, sample_rate);
        assert!((f / 600.0 - 1.0).abs() < 1e-3, "mode_freq: {f}");
        let longer = render_with(&[], Some((2 * MAX_MODES, 1.0)));
        let (_, t60) = partial(&longer, 300.0, 0, span, span, sample_rate);
        assert!(
            (t60 / (2.0 * DECAY_S) - 1.0).abs() < 0.01,
            "mode_decay: {t60}"
        );
        // brightness b: mode k's amplitude is sin(π·k·position)·k^b.
        let mut modal = bank(sample_rate);
        let mut p = [0.0; PARAMETERS];
        p.copy_from_slice(&defaults());
        p[BRIGHTNESS] = 0.7;
        coefficients(&mut modal, &p);
        for k in 0..modal.count {
            let kk = k as f64 + 1.0;
            let expected = match modal.modes[k].f < 0.49 * sample_rate {
                true => libm::sin(std::f64::consts::PI * kk * POSITION) * libm::pow(kk, 0.7),
                false => 0.0,
            };
            assert_eq!(modal.modes[k].amplitude, expected);
        }
    }

    /// T-M3 (AC2, AC10): the morph is continuous. Sweeping `material` from 0
    /// to 5 in steps of 10⁻⁴, every mode's ratio moves by at most the largest
    /// table difference times the step and its weight by at most the step (a
    /// mode that leaves the bank has weight 0); at every integer the ratios
    /// are the table's and every weight is 1.
    #[test]
    fn the_morph_is_continuous_and_exact_at_the_integers() {
        let mut modal = bank(48_000.0);
        let mut p = [0.0; PARAMETERS];
        p.copy_from_slice(&defaults());
        p[2] = 64.0;
        let explicit = [
            3.0, 0.5, 7.0, 1.0, 9.5, 2.0, 11.0, 4.0, 1.5, 6.0, 0.25, 8.0, 5.0, 10.0, 12.0, 0.75,
        ];
        p[FIXED..FIXED + RATIOS].copy_from_slice(&explicit);
        let state = |modal: &mut Modal, m: f64| -> Vec<(f64, f64)> {
            let mut q = p;
            q[1] = m.min(5.0);
            modal.globals(&q);
            (0..MAX_MODES)
                .map(|k| match k < modal.count {
                    true => modal.ratio(&q, k),
                    false => (f64::NAN, 0.0),
                })
                .collect()
        };
        let step = 1e-4;
        let largest = 1835.3;
        let mut previous = state(&mut modal, 0.0);
        for i in 1..=50_000 {
            let m = i as f64 / 10_000.0;
            let now = state(&mut modal, m);
            for k in 0..MAX_MODES {
                let ((r0, w0), (r1, w1)) = (previous[k], now[k]);
                assert!(
                    (w1 - w0).abs() <= step * (1.0 + 1e-6),
                    "weight, mode {k} at {m}"
                );
                if w0 > 0.0 && w1 > 0.0 {
                    assert!(
                        (r1 - r0).abs() <= largest * step * 1.01,
                        "ratio, mode {k} at {m}"
                    );
                }
            }
            if i % 10_000 == 0 {
                let material = m.round();
                for (k, &(r, w)) in now.iter().enumerate().take(modal.count) {
                    assert_eq!(r, mode_ratio(material, k, &explicit).unwrap());
                    assert_eq!(w, 1.0);
                }
            }
            previous = now;
        }
    }

    /// The parameters of frame `n` of a random modulation: every parameter
    /// anywhere in its range.
    fn random_parameters(s: &mut RngStream) -> Vec<f64> {
        DESCRIPTOR
            .parameters
            .iter()
            .enumerate()
            .map(|(i, d)| match i {
                // freq within the audible band, so modes ring.
                0 => uniform(s, 20.0, 20_000.0),
                _ => uniform(s, d.range.min, d.range.max),
            })
            .collect()
    }

    /// T-M4 (AC1, AC6, AC10): under modulation of every parameter and vector
    /// row on every frame, at 44.1, 48, 96 and 192 kHz: every stored radius is
    /// below 1; every mode's state stays within the uniform contraction bound
    /// `B[n] = (ρ̄·B[n−1] + Ḡ·X)·(1 + 4ε)`, ρ̄ the largest stored radius so far
    /// and Ḡ the largest `|gc| + |gs|` (≥ |g|) so far; the output is
    /// `clamp(gain·Σ W_k·v_k, ±10⁶)` exactly; and the corner of the largest
    /// gain (decay 60 s × 64, law 0, `mode_gain` 16, `gain` 4) reaches the
    /// clamp. The auditor's case (60 s of accumulation, then 0.01 s) is the
    /// first second.
    #[test]
    fn modulation_stays_within_the_contraction_bound() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut s = stream(sample_rate as u64);
            let mut modal = bank(sample_rate);
            let frames = 6000;
            let mut bound = [0.0f64; MAX_MODES];
            let mut rho_max = [0.0f64; MAX_MODES];
            let mut g_max = [0.0f64; MAX_MODES];
            let mut x_max = 0.0f64;
            let mut clamped = false;
            for n in 0..frames {
                let (p, rows, x) = match n {
                    // The auditor's case: one 12 kHz mode accumulates at
                    // decay 60 s under a unit quadrature input, then the
                    // decay drops to 0.01 s.
                    0..1500 => {
                        let mut p = defaults();
                        (p[0], p[1], p[2], p[3], p[5]) = (12_000.0, 0.0, 1.0, 60.0, 0.5);
                        if n >= 1000 {
                            p[3] = 0.01;
                        }
                        let x =
                            libm::sin(std::f64::consts::TAU * 12_000.0 * n as f64 / sample_rate);
                        (p, vec![0.0; ROWS], if n < 1000 { x } else { 0.0 })
                    }
                    // The largest-gain corner, unit input: `freq` 10⁷ brought
                    // to 20 kHz by `mode_freq` (the density stays that of
                    // 10⁷), `mode_gain` 16, `mode_decay` 64, `gain` 4, driven
                    // at resonance.
                    1500..3000 => {
                        let mut p = defaults();
                        (p[0], p[1], p[2], p[3], p[4], p[GAIN]) = (1e7, 0.0, 64.0, 60.0, 0.0, 4.0);
                        let mut rows = vec![0.0; ROWS];
                        rows[0] = -0.998;
                        rows[MAX_MODES..2 * MAX_MODES].fill(15.0);
                        rows[2 * MAX_MODES..].fill(63.0);
                        let x =
                            libm::sin(std::f64::consts::TAU * 20_000.0 * n as f64 / sample_rate);
                        (p, rows, x)
                    }
                    _ => {
                        let rows = (0..ROWS).map(|_| uniform(&mut s, -2.0, 20.0)).collect();
                        (random_parameters(&mut s), rows, uniform(&mut s, -1.0, 1.0))
                    }
                };
                let y = step(&mut modal, &p, x, &rows);
                x_max = x_max.max(x.abs());
                let mut sum = 0.0;
                for k in 0..modal.count {
                    let m = modal.modes[k];
                    assert!(m.rho < 1.0, "ρ at {sample_rate} Hz, frame {n}");
                    rho_max[k] = rho_max[k].max(m.rho);
                    g_max[k] = g_max[k].max(m.gc.abs() + m.gs.abs());
                    bound[k] =
                        (rho_max[k] * bound[k] + g_max[k] * x_max) * (1.0 + 4.0 * f64::EPSILON);
                    let (u, v) = modal.state[k];
                    assert!(
                        libm::sqrt(u * u + v * v) <= bound[k] * (1.0 + 1e-12),
                        "state of mode {k} at {sample_rate} Hz, frame {n}"
                    );
                    sum += m.weight * v;
                }
                let expected = (p[GAIN] * sum).clamp(-1e6, 1e6);
                assert_eq!(y.to_bits(), expected.to_bits(), "output, frame {n}");
                clamped |= y.abs() == 1e6;
            }
            assert!(clamped, "the corner reaches the clamp at {sample_rate} Hz");
        }
    }

    /// T-M5 (AC6): ρ < 1 at the extreme corners (the smallest ratio, the
    /// largest law and decay, `mode_decay` 64, 192 kHz), and the clamp to
    /// 1 − 10⁻¹³ leaves the most extreme radius of the bank before M2.10-8
    /// unchanged.
    #[test]
    fn every_radius_is_below_one() {
        let sr = 192_000.0;
        let before = libm::pow(10.0, -3.0 * libm::pow(0.01, 3.0) / (60.0 * sr));
        assert!(before < RHO_MAX, "{before}");
        let mut modal = bank(sr);
        let mut p = [0.0; PARAMETERS];
        p.copy_from_slice(&defaults());
        (p[1], p[2], p[3], p[4]) = (5.0, 16.0, 60.0, 3.0);
        p[FIXED..FIXED + RATIOS].fill(0.01);
        modal.params = Some(p);
        modal.globals(&p);
        for k in 0..modal.count {
            modal.visit(&p, k, [16.0, 16.0, 64.0]);
            let m = modal.modes[k];
            assert!(
                m.rho < 1.0 && m.rc * m.rc + m.rs * m.rs < 1.0,
                "mode {k}: {}",
                m.rho
            );
        }
        modal.visit(&p, 0, [1.0; 3]);
        assert_eq!(modal.modes[0].rho, before);
    }

    /// T-M5(ii) (review of M2.10-8): at random corners of every parameter and
    /// row (each at a limit of its range, the rows at their factors' limits)
    /// at 44.1–192 kHz, every radius is at most `1 − 10⁻¹³` and one step of
    /// the recurrence, as `process` rounds it, shrinks `|z|²` for unit
    /// states in every direction, so rounding never makes a mode grow.
    #[test]
    fn every_rounded_step_contracts_at_the_corners() {
        let mut s = stream(41);
        let pick = |s: &mut RngStream, lo: f64, hi: f64| if s.next_f64() < 0.5 { lo } else { hi };
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut modal = bank(sr);
            for _ in 0..400 {
                let mut p = [0.0; PARAMETERS];
                for (i, d) in DESCRIPTOR.parameters.iter().enumerate() {
                    p[i] = pick(&mut s, d.range.min, d.range.max);
                }
                p[0] = pick(&mut s, 1.0, 20_000.0);
                p[1] = libm::floor(uniform(&mut s, 0.0, 6.0)).min(5.0);
                p[2] = 64.0;
                modal.params = Some(p);
                modal.globals(&p);
                for k in 0..modal.count {
                    let factors = [
                        pick(&mut s, FREQ_FACTOR.0, FREQ_FACTOR.1),
                        pick(&mut s, GAIN_FACTOR.0, GAIN_FACTOR.1),
                        pick(&mut s, DECAY_FACTOR.0, DECAY_FACTOR.1),
                    ];
                    modal.modes[k].valid = false;
                    modal.visit(&p, k, factors);
                    let m = modal.modes[k];
                    assert!(m.rho <= RHO_MAX, "ρ {} at {sr} Hz", m.rho);
                    for (u, v) in [(1.0, 0.0), (0.0, 1.0), (0.6, 0.8), (-0.8, 0.6)] {
                        let next_u: f64 = m.rc * u - m.rs * v;
                        let next_v: f64 = m.rs * u + m.rc * v;
                        assert!(
                            next_u * next_u + next_v * next_v < u * u + v * v,
                            "mode {k} at {sr} Hz grows: ρ {}",
                            m.rho
                        );
                    }
                }
            }
        }
    }

    /// Review of M2.10-8 (Nyquist): a ringing mode moved to `0.49·sr` or
    /// above is heard with the fade `h_k` and not at all from Nyquist up,
    /// whatever moves it. (1) A single 10 kHz mode at 48 kHz, rung by an
    /// impulse, then `freq` held at 0.495·sr: the output is exactly
    /// `h·v = ½·v`; at 0.5·sr, 0.6·sr and 1.3·sr exactly 0 while the state
    /// still rings. (2) The same mode swept at audio rate from 0.4·sr to
    /// 0.7·sr over 2000 frames, at 44.1–192 kHz: no output at all on the
    /// frames above Nyquist (the alias the bank before produced), and no
    /// sample beyond the unfaded mode's within the band. (3) `inharmonicity`
    /// and `mode_freq` pushing a ringing string's upper modes past Nyquist:
    /// those modes keep their state and contribute exactly 0.
    #[test]
    fn modes_moved_past_nyquist_fade_out() {
        let sr = 48_000.0;
        let ring = |modal: &mut Modal, p: &mut Vec<f64>, freq: f64| {
            (p[0], p[1], p[2], p[3], p[4], p[5]) = (freq, 0.0, 1.0, 60.0, 0.0, 0.5);
            step(modal, p, 1.0, &[]);
            for _ in 0..200 {
                step(modal, p, 0.0, &[]);
            }
        };
        // (1)
        for (at, fade) in [(0.495, 0.5), (0.5, 0.0), (0.6, 0.0), (1.3, 0.0)] {
            let mut modal = bank(sr);
            let mut p = defaults();
            ring(&mut modal, &mut p, 10_000.0);
            p[0] = at * sr;
            for n in 0..500 {
                let y = step(&mut modal, &p, 0.0, &[]);
                let v = modal.state[0].1;
                assert!(modal.state[0] != (0.0, 0.0), "still ringing");
                assert!(
                    (y - fade * v).abs() <= 1e-15 * v.abs(),
                    "{at}·sr, frame {n}: {y} vs {}",
                    fade * v
                );
                if fade == 0.0 {
                    assert_eq!(y, 0.0);
                }
            }
        }
        // (2)
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut modal = bank(sr);
            let mut p = defaults();
            ring(&mut modal, &mut p, 0.4 * sr);
            let mut aliased = 0.0;
            for n in 0..2000 {
                let f = sr * (0.4 + 0.3 * n as f64 / 2000.0);
                p[0] = f;
                let y = step(&mut modal, &p, 0.0, &[]);
                let v = modal.state[0].1;
                assert!(y.abs() <= v.abs(), "{sr} Hz, frame {n}");
                if f >= 0.5 * sr {
                    aliased += y * y;
                }
            }
            assert_eq!(aliased, 0.0, "{sr} Hz: energy above Nyquist");
        }
        // (3) A 3 kHz string of 16 modes rung, then B = 0.1 (mode k at
        // 3k·√((1 + 0.1k²)/1.1) kHz: modes 5 and up pass 24 kHz) and mode 3's
        // `mode_freq` 7 (×8: 72 kHz).
        let mut modal = bank(sr);
        let mut p = defaults();
        (p[0], p[1], p[2], p[3], p[4]) = (3000.0, 0.0, 16.0, 60.0, 0.0);
        step(&mut modal, &p, 1.0, &[]);
        p[INHARMONICITY] = 0.1;
        let mut rows = vec![0.0; ROWS];
        rows[2] = 7.0;
        for _ in 0..200 {
            let y = step(&mut modal, &p, 0.0, &rows);
            let mut heard = 0.0;
            for k in 0..modal.count {
                let m = modal.modes[k];
                if m.f >= 0.5 * sr {
                    assert_eq!(m.weight, 0.0, "mode {}", k + 1);
                } else {
                    heard += m.weight * modal.state[k].1;
                }
            }
            assert_eq!(y.to_bits(), heard.to_bits());
        }
        // Modes 3 and 7 (9 and 21 kHz, below the cutoff, so rung) now ring
        // above Nyquist, unheard.
        for k in [2, 6] {
            assert!(modal.modes[k].f >= 0.5 * sr, "mode {}", k + 1);
            assert!(modal.state[k] != (0.0, 0.0), "mode {} rings", k + 1);
        }
    }

    /// T-M6 (AC1, AC10): a change of frequency keeps the state (with no
    /// input, `|z|` falls by exactly ρ across the change), and a modulated
    /// bank renders the same in calls of 1, 7 and 256 frames.
    #[test]
    fn coefficient_changes_keep_the_state_and_any_call_size_renders_alike() {
        let sample_rate = 48_000.0;
        let mut modal = bank(sample_rate);
        let mut p = defaults();
        (p[0], p[1], p[2]) = (300.0, 0.0, 1.0);
        step(&mut modal, &p, 1.0, &[]);
        for _ in 0..499 {
            step(&mut modal, &p, 0.0, &[]);
        }
        let (u, v) = modal.state[0];
        let before = libm::sqrt(u * u + v * v);
        p[0] = 900.0;
        step(&mut modal, &p, 0.0, &[]);
        let (u, v) = modal.state[0];
        let after = libm::sqrt(u * u + v * v);
        let rho = modal.modes[0].rho;
        assert!(
            (after / (rho * before) - 1.0).abs() < 1e-12,
            "{after} vs {}",
            rho * before
        );

        let frames = 2000;
        let mut s = stream(5);
        let mut lanes: Vec<Vec<f64>> = vec![Vec::new(); PARAMETERS];
        for _ in 0..frames {
            for (lane, v) in lanes.iter_mut().zip(random_parameters(&mut s)) {
                lane.push(v);
            }
        }
        let rows: Vec<Vec<f64>> = (0..ROWS)
            .map(|_| (0..frames).map(|_| uniform(&mut s, -1.0, 2.0)).collect())
            .collect();
        let x: Vec<f64> = (0..frames).map(|_| uniform(&mut s, -1.0, 1.0)).collect();
        let reference = run(&mut bank(sample_rate), &lanes, &x, &rows, 1);
        for block in [7, 256] {
            assert_eq!(
                run(&mut bank(sample_rate), &lanes, &x, &rows, block),
                reference
            );
        }
    }

    /// A scripted modulation: on frame `n`, sets the parameters and rows and
    /// returns the input.
    type Script<'a> = dyn FnMut(usize, &mut Vec<f64>, &mut Vec<f64>) -> f64 + 'a;

    /// Runs `frames` of a scripted modulation through a cached bank and an
    /// oracle that recomputes every mode from scratch on every frame, and
    /// asserts the same output and the same coefficients on every frame.
    /// Returns the cached bank's coefficients per frame.
    fn against_the_oracle(
        sample_rate: f64,
        frames: usize,
        script: &mut Script<'_>,
    ) -> Vec<[Mode; MAX_MODES]> {
        let mut cached = bank(sample_rate);
        let mut oracle = bank(sample_rate);
        oracle.force = true;
        let mut p = defaults();
        let mut rows = vec![0.0; ROWS];
        let mut history = Vec::with_capacity(frames);
        for n in 0..frames {
            let x = script(n, &mut p, &mut rows);
            let a = step(&mut cached, &p, x, &rows);
            let b = step(&mut oracle, &p, x, &rows);
            assert_eq!(a.to_bits(), b.to_bits(), "output, frame {n}");
            assert_eq!(cached.count, oracle.count);
            for k in 0..cached.count {
                let (c, o) = (cached.modes[k], oracle.modes[k]);
                let bits = |m: Mode| [m.rc, m.rs, m.gc, m.gs, m.weight].map(f64::to_bits);
                assert_eq!(bits(c), bits(o), "coefficients of mode {k}, frame {n}");
            }
            history.push(cached.modes);
        }
        history
    }

    /// T-M7 (AC5): the cache never leaves a coefficient stale. Random
    /// piecewise-constant lanes on every parameter and row, and the targeted
    /// cases, match an oracle that recomputes every mode on every frame; and
    /// in each targeted case the coefficient that must change does: (i) an
    /// explicit ratio at law 1 changes ρ; (ii) a material change moves a
    /// mode across 0.49·sr; (iii) a `mode_freq` row, inharmonicity or spread
    /// does; (iv) a frequency change changes the excitation (density); (v) one
    /// `mode_decay` row changes only that mode's ρ; (vi) an explicit ratio
    /// between the bell and the explicit ratios; (vii) `freq` changes ρ only
    /// while `damping_tilt` ≠ 0; (viii) a material change reaches the weight.
    #[test]
    fn the_cache_matches_a_full_recomputation() {
        let sr = 48_000.0;
        let mut s = stream(17);
        let mut lanes_now = random_parameters(&mut s);
        against_the_oracle(sr, 4000, &mut |n, p, rows| {
            if n % 37 == 0 {
                let i = (s.next_f64() * PARAMETERS as f64) as usize;
                lanes_now[i] = random_parameters(&mut s)[i];
            }
            if n % 53 == 0 {
                let r = (s.next_f64() * ROWS as f64) as usize;
                rows[r] = uniform(&mut s, -2.0, 4.0);
            }
            p.copy_from_slice(&lanes_now);
            if n % 500 == 0 { 1.0 } else { 0.0 }
        });

        let changed = |history: &[[Mode; MAX_MODES]], k: usize, at: usize, f: fn(&Mode) -> f64| {
            f(&history[at][k]).to_bits() != f(&history[at - 1][k]).to_bits()
        };
        // (i)
        let h = against_the_oracle(sr, 20, &mut |n, p, _| {
            (p[1], p[4]) = (5.0, 1.0);
            p[FIXED] = if n < 10 { 1.0 } else { 2.0 };
            0.0
        });
        assert!(changed(&h, 0, 10, |m| m.rho));
        // (ii) string mode 4 at 12 kHz, bar mode 4 at 26.8 kHz ≥ 23.52 kHz.
        let h = against_the_oracle(sr, 20, &mut |n, p, _| {
            (p[0], p[1]) = (3000.0, if n < 10 { 0.0 } else { 1.0 });
            0.0
        });
        assert!(h[9][3].gs != 0.0 && h[10][3].gs == 0.0);
        // (iii) mode 4 pushed over the cutoff by its row, by inharmonicity
        // and by spread.
        for (i, script) in [0, 1, 2].into_iter().zip([
            (|n: usize, p: &mut Vec<f64>, rows: &mut Vec<f64>| {
                (p[0], p[1]) = (3000.0, 0.0);
                rows[3] = if n < 10 { 0.0 } else { 1.0 };
            }) as fn(usize, &mut Vec<f64>, &mut Vec<f64>),
            |n, p, _| {
                (p[0], p[1]) = (5800.0, 0.0);
                p[INHARMONICITY] = if n < 10 { 0.0 } else { 0.1 };
            },
            |n, p, _| {
                (p[0], p[1]) = (5850.0, 0.0);
                p[SPREAD] = if n < 10 { 0.0 } else { 0.1 };
            },
        ]) {
            let h = against_the_oracle(sr, 20, &mut |n, p, rows| {
                script(n, p, rows);
                0.0
            });
            if i < 2 {
                assert!(h[9][3].gs != 0.0 && h[10][3].gs == 0.0, "case (iii) {i}");
            } else {
                // Spread moves mode 4 by up to 10 %: whichever way, it moves.
                assert!(changed(&h, 3, 10, |m| m.rc), "case (iii) spread");
            }
        }
        // (iv)
        let h = against_the_oracle(sr, 20, &mut |n, p, _| {
            p[0] = if n < 10 { 220.0 } else { 330.0 };
            0.0
        });
        assert!(changed(&h, 0, 10, |m| m.gc));
        // (v)
        let h = against_the_oracle(sr, 20, &mut |n, _, rows| {
            rows[2 * MAX_MODES + 2] = if n < 10 { 0.0 } else { 1.0 };
            0.0
        });
        assert!(changed(&h, 2, 10, |m| m.rho));
        assert!(
            (0..16)
                .filter(|&k| k != 2)
                .all(|k| !changed(&h, k, 10, |m| m.rho))
        );
        // (vi)
        let h = against_the_oracle(sr, 20, &mut |n, p, _| {
            p[1] = 4.5;
            p[FIXED] = if n < 10 { 1.0 } else { 3.0 };
            0.0
        });
        assert!(changed(&h, 0, 10, |m| m.rc));
        // (vii)
        for tau in [0.0, 1.0] {
            let h = against_the_oracle(sr, 20, &mut |n, p, _| {
                p[DAMPING_TILT] = tau;
                p[0] = if n < 10 { 220.0 } else { 330.0 };
                0.0
            });
            assert_eq!(changed(&h, 0, 10, |m| m.rho), tau != 0.0, "τ {tau}");
        }
        // (viii)
        let h = against_the_oracle(sr, 20, &mut |n, p, _| {
            (p[1], p[2]) = (if n < 10 { 3.5 } else { 3.6 }, 64.0);
            0.0
        });
        assert!(changed(&h, 20, 10, |m| m.weight));
    }

    /// T-M8(i) (AC2): a ringing mode fades out continuously where the morph
    /// removes it. Only membrane mode 13 is excited (the other rows of
    /// `mode_gain` −1), then, with no input, `material` is held at 4 − δ:
    /// the mode is heard at no more than δ times its amplitude, and at 4 not
    /// at all.
    #[test]
    fn a_removed_mode_fades_out_with_the_morph() {
        let sr = 48_000.0;
        let excite = |modal: &mut Modal, p: &mut Vec<f64>| {
            (p[0], p[1], p[2], p[3], p[4], p[5]) = (110.0, 3.0, 64.0, 60.0, 0.0, 1.0 / 26.0);
            let mut rows = vec![0.0; ROWS];
            rows[MAX_MODES..2 * MAX_MODES].fill(-1.0);
            rows[MAX_MODES + 12] = 0.0;
            step(modal, p, 1.0, &rows);
            rows
        };
        let mut reference = bank(sr);
        let mut p = defaults();
        let rows = excite(&mut reference, &mut p);
        let peak = (0..1000)
            .map(|_| step(&mut reference, &p, 0.0, &rows).abs())
            .fold(0.0, f64::max);
        assert!(peak > 0.0);
        for delta in [1e-2, 1e-4, 1e-6, 0.0] {
            let mut modal = bank(sr);
            let mut p = defaults();
            let rows = excite(&mut modal, &mut p);
            p[1] = 4.0 - delta;
            for n in 0..1000 {
                let y = step(&mut modal, &p, 0.0, &rows);
                assert!(
                    y.abs() <= delta * peak * (1.0 + 1e-6),
                    "δ {delta}, frame {n}: {y}"
                );
            }
        }
    }

    /// T-M8(ii) (AC2): every boundary, pre-excited, with no input and
    /// `material` ramping by ±10⁻⁵ per frame across it, at 48 and 192 kHz:
    /// on every frame `|Δy| ≤ Σ|W_k[n]|·|Δv_k| + 2·10⁻⁵·Σ|v_k[n−1]| + 10⁻¹²`.
    /// At 4, where the number of modes changes, only the modes that leave
    /// are excited (13–64 from below, 13–16 from above), so a hard cut-off,
    /// a jump of |Σ v_k| over them, fails it; every case also runs with only
    /// mode 13.
    #[test]
    fn the_output_is_continuous_across_every_boundary() {
        for sr in [48_000.0, 192_000.0] {
            let mut cases = Vec::new();
            for boundary in [1.0, 2.0, 3.0] {
                cases.push((boundary, 1.0, 0..64));
                cases.push((boundary, -1.0, 0..64));
            }
            cases.push((4.0, 1.0, 12..64));
            cases.push((4.0, -1.0, 12..16));
            cases.push((4.0, 1.0, 12..13));
            cases.push((4.0, -1.0, 12..13));
            for (boundary, direction, excited) in cases {
                let mut modal = bank(sr);
                let mut p = defaults();
                (p[0], p[2], p[3], p[4], p[5]) = (110.0, 64.0, 60.0, 0.0, 0.37);
                let start = boundary - direction * 0.002;
                p[1] = start;
                let mut rows = vec![0.0; ROWS];
                rows[MAX_MODES..2 * MAX_MODES].fill(-1.0);
                for k in excited.clone() {
                    rows[MAX_MODES + k] = 0.0;
                }
                let mut y0 = step(&mut modal, &p, 1.0, &rows);
                let mut v0: Vec<f64> = modal.state.iter().map(|s| s.1).collect();
                let mut crossed = false;
                for n in 1..=400 {
                    p[1] = start + direction * n as f64 * 1e-5;
                    let y = step(&mut modal, &p, 0.0, &rows);
                    let v: Vec<f64> = modal.state.iter().map(|s| s.1).collect();
                    let heard = |k: usize| match k < modal.count {
                        true => modal.modes[k].weight,
                        false => 0.0,
                    };
                    let bound: f64 = (0..MAX_MODES)
                        .map(|k| heard(k).abs() * (v[k] - v0[k]).abs() + 2e-5 * v0[k].abs())
                        .sum::<f64>()
                        + 1e-12;
                    assert!(
                        (y - y0).abs() <= bound,
                        "{sr} Hz, boundary {boundary}, direction {direction}, modes \
                         {excited:?}, frame {n}: |Δy| {} > {bound}",
                        (y - y0).abs()
                    );
                    crossed |= (p[1] - boundary) * direction >= 0.0;
                    (y0, v0) = (y, v);
                }
                assert!(crossed);
            }
        }
    }

    /// T-M9 (AC6, auditor-1-006, -007): non-finite and overflowing values.
    /// (1) The auditor's case, a finite 10³⁰⁸ input at `freq` 10⁶ with
    /// `mode_freq` −1 (θ = 0) and `mode_gain` 15, then silence; (2) single
    /// input frames of ±∞ and NaN: the state stays finite and the output
    /// within ±10⁶. (3) A NaN on one frame of every parameter and row: the
    /// output stays finite and within ±10⁶; `freq`, `material`, `modes` and
    /// a NaN `law` on a single mode of ratio 1 are bit-identical to the bank
    /// before M2.10-8; every added parameter, `gain` included, renders as at
    /// its default; a NaN `decay_s` or `position` silences the modes.
    #[test]
    fn non_finite_values_keep_the_bank_finite() {
        let sr = 48_000.0;
        let frames = 4800;
        let corner = || {
            let mut p = defaults();
            (p[0], p[1], p[2], p[5]) = (1e6, 0.0, 1.0, 0.5);
            let mut rows = vec![0.0; ROWS];
            rows[0] = -1.0;
            rows[MAX_MODES] = 15.0;
            (p, rows)
        };
        let finite = |modal: &Modal, y: f64, what: &str| {
            assert!(y.is_finite() && y.abs() <= 1e6, "{what}: output {y}");
            assert!(
                modal
                    .state
                    .iter()
                    .all(|(u, v)| u.is_finite() && v.is_finite()),
                "{what}: state"
            );
        };
        for first in [1e308, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            for at_corner in [true, false] {
                let mut modal = bank(sr);
                let (p, rows) = if at_corner {
                    corner()
                } else {
                    (defaults(), Vec::new())
                };
                for n in 0..frames {
                    let y = step(&mut modal, &p, if n == 0 { first } else { 0.0 }, &rows);
                    finite(
                        &modal,
                        y,
                        &format!("input {first}, corner {at_corner}, frame {n}"),
                    );
                }
            }
        }

        let mut x = vec![0.0; frames];
        x[0] = 1.0;
        let with_nan = |i: usize| {
            let mut l = lanes(frames, &[(2, 8.0)]);
            l[i][100] = f64::NAN;
            l
        };
        for i in 0..PARAMETERS {
            let l = with_nan(i);
            let mut modal = bank(sr);
            let y = run(&mut modal, &l, &x, &[], 1);
            assert!(
                y.iter().all(|y| y.is_finite() && y.abs() <= 1e6),
                "NaN on parameter {i}"
            );
            if [0, 1, 2].contains(&i) {
                let mut legacy = Legacy::new(sr);
                for n in 0..frames {
                    let p: Vec<f64> = l.iter().map(|l| l[n]).collect();
                    assert_eq!(
                        y[n].to_bits(),
                        legacy.frame(&p, x[n]).to_bits(),
                        "parameter {i}, frame {n}"
                    );
                }
            }
            if i >= INHARMONICITY {
                let plain = run(&mut bank(sr), &lanes(frames, &[(2, 8.0)]), &x, &[], 1);
                assert_eq!(y, plain, "added parameter {i}");
            }
            if i == 3 || i == 5 {
                let mut modal = bank(sr);
                let mut p = defaults();
                p[2] = 8.0;
                step(&mut modal, &p, 1.0, &[]);
                p[i] = f64::NAN;
                step(&mut modal, &p, 0.0, &[]);
                assert!(
                    modal.modes[..8].iter().all(|m| m.rc == 0.0 && m.gs == 0.0),
                    "parameter {i}"
                );
            }
        }
        // law NaN on a single mode of ratio 1: pow(1, NaN) = 1, as before.
        let l = {
            let mut l = lanes(frames, &[(1, 0.0), (2, 1.0)]);
            l[4][100] = f64::NAN;
            l
        };
        let mut modal = bank(sr);
        let y = run(&mut modal, &l, &x, &[], 1);
        let mut legacy = Legacy::new(sr);
        for n in 0..frames {
            let p: Vec<f64> = l.iter().map(|l| l[n]).collect();
            assert_eq!(y[n].to_bits(), legacy.frame(&p, x[n]).to_bits());
        }
        assert_eq!(modal.guards, 0);
        for r in [0, MAX_MODES, 2 * MAX_MODES, 7, MAX_MODES + 7] {
            let mut rows = vec![vec![0.0; frames]; ROWS];
            rows[r][100] = f64::NAN;
            let y = run(&mut bank(sr), &lanes(frames, &[(2, 8.0)]), &x, &rows, 1);
            let plain = run(&mut bank(sr), &lanes(frames, &[(2, 8.0)]), &x, &[], 1);
            assert_eq!(y, plain, "NaN row {r}");
        }
    }

    /// T-M9(5) (auditor-2-g3-identity-argument): against the bank before
    /// M2.10-8, the output is bit-identical on every frame before the old
    /// bank's first fault (a value that is not finite or beyond ±10⁶), for
    /// the auditor's corner and for random runs mixing inputs of 10³⁰⁰, ±∞,
    /// NaN and NaN `decay_s`, `position` or `law` (with 8 modes); every run
    /// has such a fault.
    #[test]
    fn the_output_matches_the_bank_before_until_its_first_fault() {
        let sr = 48_000.0;
        let mut s = stream(23);
        for case in 0..24 {
            let frames = 3000;
            let mut lanes = lanes(frames, &[(2, 8.0)]);
            let mut x: Vec<f64> = (0..frames).map(|_| uniform(&mut s, -1.0, 1.0)).collect();
            let at = 200 + (s.next_f64() * 2500.0) as usize;
            match case % 6 {
                0 => x[at] = 1e300,
                1 => x[at] = f64::INFINITY,
                2 => x[at] = f64::NAN,
                3 => lanes[3][at] = f64::NAN,
                4 => lanes[5][at] = f64::NAN,
                _ => lanes[4][at] = f64::NAN,
            }
            let mut legacy = Legacy::new(sr);
            let mut modal = bank(sr);
            let y = run(&mut modal, &lanes, &x, &[], 1);
            let mut fault = None;
            for n in 0..frames {
                let p: Vec<f64> = lanes.iter().map(|l| l[n]).collect();
                let old = legacy.frame(&p, x[n]);
                if !old.is_finite() || old.abs() > 1e6 {
                    fault = Some(n);
                    break;
                }
                assert_eq!(y[n].to_bits(), old.to_bits(), "case {case}, frame {n}");
            }
            assert!(fault.is_some(), "case {case} never faults the bank before");
            assert!(
                y.iter().all(|y| y.is_finite() && y.abs() <= 1e6),
                "case {case}"
            );
        }
        // A finite 10³⁰⁸ input (the auditor's input; its corner needs
        // `mode_freq`, which the old bank lacks): the old bank faults on the
        // input frame, and the new one stays finite.
        let mut legacy = Legacy::new(sr);
        let p = defaults();
        let old = legacy.frame(&p, 1e308);
        assert!(!old.is_finite() || old.abs() > 1e6);
        let y = step(&mut bank(sr), &p, 1e308, &[]);
        assert!(y.is_finite() && y.abs() <= 1e6);
    }

    /// AC5 (auditor-1-008): coefficients are recomputed only for the modes
    /// whose inputs changed. With 16 explicit modes, counting the modes
    /// visited and the modes whose coefficients were rebuilt: held
    /// parameters, none; `ratio_1`, mode 1; `gain`, none; `freq`, every
    /// sounding mode; one `mode_decay` row, its mode; `modes` 16 → 15, none;
    /// 15 → 16 with nothing changed, mode 16 visited and not rebuilt; 12 → 16
    /// after a `freq` change while modes 13–16 were dormant, those four, and
    /// their coefficients then equal a recomputation from scratch; `ratio_1`
    /// under the bar table, none.
    #[test]
    fn only_the_modes_whose_inputs_changed_are_recomputed() {
        let mut modal = bank(48_000.0);
        let mut p = defaults();
        (p[1], p[2]) = (5.0, 16.0);
        let mut rows = vec![0.0; ROWS];
        step(&mut modal, &p, 1.0, &rows);
        assert_eq!(modal.count, 16);
        // (modes visited, modes whose coefficients were rebuilt) by one frame.
        let count = |modal: &mut Modal, p: &[f64], rows: &[f64]| {
            let (visits, rebuilds) = (modal.visits, modal.rebuilds);
            step(modal, p, 0.0, rows);
            (modal.visits - visits, modal.rebuilds - rebuilds)
        };
        assert_eq!(count(&mut modal, &p, &rows), (0, 0), "held");
        p[FIXED] = 1.5;
        assert_eq!(count(&mut modal, &p, &rows), (1, 1), "ratio_1");
        p[GAIN] = 2.0;
        assert_eq!(count(&mut modal, &p, &rows), (0, 0), "gain");
        p[0] = 330.0;
        assert_eq!(count(&mut modal, &p, &rows), (16, 16), "freq");
        rows[2 * MAX_MODES + 3] = 1.0;
        assert_eq!(count(&mut modal, &p, &rows), (1, 1), "one mode_decay row");
        // `modes` changes only the count: fewer visits nothing; more visits
        // only the returning modes, which rebuild only if something they
        // depend on changed while they were dormant.
        p[2] = 15.0;
        assert_eq!(count(&mut modal, &p, &rows), (0, 0), "modes 16 → 15");
        p[2] = 16.0;
        assert_eq!(
            count(&mut modal, &p, &rows),
            (1, 0),
            "modes 15 → 16, unchanged"
        );
        p[2] = 12.0;
        assert_eq!(count(&mut modal, &p, &rows), (0, 0), "modes 16 → 12");
        p[0] = 440.0;
        assert_eq!(
            count(&mut modal, &p, &rows),
            (12, 12),
            "freq with 12 sounding"
        );
        p[2] = 16.0;
        assert_eq!(
            count(&mut modal, &p, &rows),
            (4, 4),
            "modes 12 → 16 after freq"
        );
        // A global parameter visits every mode but rebuilds only the modes
        // whose derived inputs moved: inharmonicity leaves mode 1 at `freq`
        // (√((1 + B)/(1 + B)) = 1), so 15 of 16 rebuild.
        p[INHARMONICITY] = 0.01;
        assert_eq!(count(&mut modal, &p, &rows), (16, 15), "inharmonicity");
        p[1] = 1.0;
        assert_eq!(count(&mut modal, &p, &rows), (16, 16), "material");
        p[FIXED] = 2.5;
        assert_eq!(
            count(&mut modal, &p, &rows),
            (0, 0),
            "ratio_1 under the bar"
        );
        // The skipped visits changed nothing: the same lanes from scratch.
        let mut oracle = bank(48_000.0);
        oracle.force = true;
        let a = step(&mut modal, &p, 0.5, &rows);
        let b = {
            let mut q = defaults();
            (q[1], q[2]) = (5.0, 16.0);
            step(&mut oracle, &q, 1.0, &vec![0.0; ROWS]);
            step(&mut oracle, &p, 0.5, &rows)
        };
        let bits = |m: &Modal| -> Vec<[u64; 5]> {
            m.modes[..m.count]
                .iter()
                .map(|m| [m.rc, m.rs, m.gc, m.gs, m.weight].map(f64::to_bits))
                .collect()
        };
        assert_eq!(bits(&modal), bits(&oracle));
        assert!(a.is_finite() && b.is_finite());
    }

    /// T-M9(6) (auditor-2-nan-compare-bitwise): a persistent NaN recomputes
    /// once, not on every frame.
    #[test]
    fn a_persistent_nan_recomputes_once() {
        let mut modal = bank(48_000.0);
        let mut p = defaults();
        step(&mut modal, &p, 1.0, &[]);
        let start = modal.recomputes;
        p[3] = f64::NAN;
        let mut rows = vec![0.0; ROWS];
        rows[5] = f64::NAN;
        for _ in 0..1000 {
            step(&mut modal, &p, 0.0, &rows);
        }
        assert_eq!(modal.recomputes - start, 1);
    }

    /// AC9 (indicative, not a threshold): the cost per mode and frame,
    /// static and with each group of parameters modulated, in this build.
    /// `cargo test --release -p signal-std --lib -- --ignored modal_cost --nocapture`.
    #[test]
    #[ignore = "timing: docs/benchmarks.md, M2.10-8"]
    fn modal_cost() {
        let sr = 48_000.0;
        let frames = 48_000;
        let mut s = stream(3);
        let x: Vec<f64> = (0..frames).map(|_| uniform(&mut s, -1.0, 1.0)).collect();
        let sine = |hz: f64, lo: f64, hi: f64| -> Vec<f64> {
            (0..frames)
                .map(|n| {
                    let u = 0.5 + 0.5 * libm::sin(std::f64::consts::TAU * hz * n as f64 / sr);
                    lo + (hi - lo) * u
                })
                .collect()
        };
        type Lanes = Vec<(usize, Vec<f64>)>;
        let groups: [(&str, Lanes); 6] = [
            ("static", vec![]),
            ("freq", vec![(0, sine(1000.0, 100.0, 400.0))]),
            ("decay_s", vec![(3, sine(1000.0, 0.5, 4.0))]),
            ("position", vec![(5, sine(1000.0, 0.1, 0.9))]),
            ("material", vec![(1, sine(1000.0, 0.0, 5.0))]),
            (
                "all",
                vec![
                    (0, sine(1000.0, 100.0, 400.0)),
                    (1, sine(700.0, 0.0, 5.0)),
                    (3, sine(900.0, 0.5, 4.0)),
                    (4, sine(800.0, 0.0, 3.0)),
                    (5, sine(1100.0, 0.1, 0.9)),
                    (INHARMONICITY, sine(1300.0, 0.0, 0.1)),
                    (DAMPING_TILT, sine(1200.0, 0.0, 10.0)),
                    (BRIGHTNESS, sine(1400.0, -1.0, 1.0)),
                    (PICKUP_MIX, sine(1500.0, 0.0, 1.0)),
                    (SPREAD, sine(1600.0, 0.0, 0.1)),
                ],
            ),
        ];
        for modes in [1.0, 16.0, 64.0] {
            for (name, set) in &groups {
                let mut l = lanes(frames, &[(1, 0.0), (2, modes)]);
                for (i, lane) in set {
                    l[*i] = lane.clone();
                }
                let mut modal = bank(sr);
                let start = std::time::Instant::now();
                let y = run(&mut modal, &l, &x, &[], 256);
                let ns = start.elapsed().as_nanos() as f64;
                std::hint::black_box(y);
                println!(
                    "modes {modes:>2}, {name:>8}: {:.1} ns per frame, {:.2} ns per mode-frame",
                    ns / frames as f64,
                    ns / frames as f64 / modes
                );
            }
        }
    }
}
