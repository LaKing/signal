//! `std.adsr`.

use signal_core::{
    Combine, DirectPrimitive, Direction, EventPayload, Feedthrough, Inputs, ModuleDescriptor,
    Outputs, PortKind, PrepareContext, Range,
};

use crate::{FLUSH_THRESHOLD, at, event_port, parameter, round_half_up, signal_port};

const ATTACK_MS: f64 = 5.0;
const DECAY_MS: f64 = 200.0;
const SUSTAIN: f64 = 0.7;
const RELEASE_MS: f64 = 300.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.adsr",
    doc: "Gated ADSR envelope: a linear attack from the current level to 1, exponential decay to sustain, exponential release.",
    ports: &[
        signal_port("gate", Direction::In, PortKind::Gate, None, None),
        event_port("trigger", Direction::In),
        signal_port(
            "out",
            Direction::Out,
            PortKind::Control,
            None,
            Some(Range { min: 0.0, max: 1.0 }),
        ),
    ],
    parameters: &[
        parameter(
            "attack_ms",
            PortKind::Control,
            Some("ms"),
            0.0,
            60_000.0,
            ATTACK_MS,
            Combine::Add,
        ),
        parameter(
            "decay_ms",
            PortKind::Control,
            Some("ms"),
            0.1,
            60_000.0,
            DECAY_MS,
            Combine::Add,
        ),
        parameter(
            "sustain",
            PortKind::Control,
            None,
            0.0,
            1.0,
            SUSTAIN,
            Combine::Add,
        ),
        parameter(
            "release_ms",
            PortKind::Control,
            Some("ms"),
            0.1,
            60_000.0,
            RELEASE_MS,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

/// Where the envelope is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    /// Output 0 until the gate rises.
    #[default]
    Idle,
    /// Frame `frame` (0-based) of a linear attack of `length` ≥ 1 frames from `from`.
    Attack { from: f64, frame: u64, length: u64 },
    /// Frame `frame` of a decay from `from` toward `sustain` with time constant `tau`.
    Decay {
        from: f64,
        frame: u64,
        tau: f64,
        sustain: f64,
    },
    /// Holding `level`, the sustain value the decay reached.
    Sustain { level: f64 },
    /// Frame `frame` of a release `from·exp(−frame/tau)`.
    Release { from: f64, frame: u64, tau: f64 },
}

/// `std.adsr`: a gated attack–decay–sustain–release envelope.
///
/// **Ports**
/// - `gate` (signal in, gate, unbounded): on when `gate[n] > 0.5`; unconnected
///   it reads 0 (off). The gate is off before frame 0.
/// - `trigger` (event in, trigger): each frame with at least one `Trigger`
///   event while the gate is on restarts the attack; several on one frame, or
///   one on the frame the gate rises, count once. Triggers while the gate is
///   off, and `Set` and `Ramp` events, are ignored.
/// - `out` (signal out, control, `[0, 1]`).
///
/// **Parameters**
/// - `attack_ms` (control, ms, range `[0, 60 000]`, default 5, combine `add`):
///   read on the frame the attack starts.
/// - `decay_ms` (control, ms, range `[0.1, 60 000]`, default 200, combine `add`).
/// - `sustain` (control, level, no unit, range `[0, 1]`, default 0.7, combine `add`).
/// - `release_ms` (control, ms, range `[0.1, 60 000]`, default 300, combine `add`).
///
/// **Feedthrough:** direct (the gate and triggers of frame *n* change `out[n]`).
///
/// **Formula** (sr the sample rate; `L` the output of the frame before an
/// edge, 0 at the start; S = `sustain[n]`):
///
/// ```text
/// A  = round_half_up(attack_ms·sr/1000)   frames, at the attack frame n0
/// τd = decay_ms·sr/1000,  τr = release_ms·sr/1000   frames
/// out[n0 + i]     = L + (1 − L)·(i/A)              0 ≤ i < A   (attack)
/// out[n0 + A + t] = 1·e^(−t/τd) + S·(1 − e^(−t/τd))  t ≥ 0       (decay)
/// out[n1 + t]     = L·e^(−t/τr)                    t ≥ 0       (release)
/// ```
///
/// - **Attack** starts on a frame where the gate rises (`gate[n] > 0.5` and
///   `gate[n−1] ≤ 0.5`), or a trigger arrives while it is on, in any stage. It
///   rises from the current level `L`, so `out[n0] = out[n0 − 1]`: no jump.
///   With A = 0 the output is 1 on the attack frame and the decay starts there.
/// - **Decay** approaches `S`. When `|out − S|` falls below 1e-9 the output is
///   exactly `S` (sustain) until `S` changes.
/// - **Release** starts on a frame where the gate falls, in any stage, from the
///   current level `L` (`out[n1] = out[n1 − 1]`). When it falls below 1e-9 it is
///   flushed to exactly 0 and the envelope is idle (`docs/dsp-conventions.md`,
///   "Denormals").
///
/// If τd or `S` changes during the decay or sustain, or τr during the release,
/// the segment continues from the last output with the new value (its start
/// becomes that output and `t` restarts at 1), which keeps it continuous; with
/// constant parameters each segment is exactly the formula. `exp` is
/// `libm::exp`. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Adsr {
    sample_rate: f64,
    stage: Stage,
    /// The output of the previous frame.
    level: f64,
    /// Whether the gate was on in the previous frame.
    gate: bool,
}

impl DirectPrimitive for Adsr {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
        self.gate = false;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let gate = inputs.signal(0);
        let events = inputs.events(0);
        let attack_ms = inputs.parameter(0);
        let decay_ms = inputs.parameter(1);
        let sustain = inputs.parameter(2);
        let release_ms = inputs.parameter(3);
        let mut next = 0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let mut triggered = false;
            while let Some(event) = events.get(next) {
                if event.offset as usize > n {
                    break;
                }
                triggered |= event.payload == EventPayload::Trigger;
                next += 1;
            }
            let on = at(gate, n, 0.0) > 0.5;
            let decay_tau = at(decay_ms, n, DECAY_MS) * self.sample_rate / 1000.0;
            let sustain = at(sustain, n, SUSTAIN);
            let release_tau = at(release_ms, n, RELEASE_MS) * self.sample_rate / 1000.0;
            if on && (!self.gate || triggered) {
                let length =
                    round_half_up(at(attack_ms, n, ATTACK_MS) * self.sample_rate / 1000.0) as u64;
                self.stage = match length {
                    0 => Stage::Decay {
                        from: 1.0,
                        frame: 0,
                        tau: decay_tau,
                        sustain,
                    },
                    _ => Stage::Attack {
                        from: self.level,
                        frame: 0,
                        length,
                    },
                };
            } else if !on && self.gate {
                self.stage = Stage::Release {
                    from: self.level,
                    frame: 0,
                    tau: release_tau,
                };
            }
            self.gate = on;
            let value = self.advance(decay_tau, sustain, release_tau);
            self.level = value;
            *out = value;
        }
    }
}

impl Adsr {
    /// The output of the current frame, given its τd, `S` and τr; moves to the
    /// next frame.
    fn advance(&mut self, decay_tau: f64, sustain: f64, release_tau: f64) -> f64 {
        match &mut self.stage {
            Stage::Idle => 0.0,
            Stage::Attack {
                from,
                frame,
                length,
            } => {
                let value = *from + (1.0 - *from) * (*frame as f64 / *length as f64);
                *frame += 1;
                if *frame >= *length {
                    self.stage = Stage::Decay {
                        from: 1.0,
                        frame: 0,
                        tau: decay_tau,
                        sustain,
                    };
                }
                value
            }
            Stage::Decay {
                from,
                frame,
                tau,
                sustain: target,
            } => {
                if *frame > 0 && (decay_tau != *tau || sustain != *target) {
                    // A new τd or S: continue from the last output.
                    *from = self.level;
                    *frame = 1;
                }
                *tau = decay_tau;
                *target = sustain;
                let e = libm::exp(-(*frame as f64) / decay_tau);
                let value = *from * e + sustain * (1.0 - e);
                *frame += 1;
                if (value - sustain).abs() < FLUSH_THRESHOLD {
                    self.stage = Stage::Sustain { level: sustain };
                    sustain
                } else {
                    value
                }
            }
            Stage::Sustain { level } => {
                if sustain == *level {
                    return sustain;
                }
                // S changed: decay toward it from the held level.
                self.stage = Stage::Decay {
                    from: *level,
                    frame: 1,
                    tau: decay_tau,
                    sustain,
                };
                self.advance(decay_tau, sustain, release_tau)
            }
            Stage::Release { from, frame, tau } => {
                if *frame > 0 && release_tau != *tau {
                    *from = self.level;
                    *frame = 1;
                }
                *tau = release_tau;
                let value = *from * libm::exp(-(*frame as f64) / release_tau);
                *frame += 1;
                if value < FLUSH_THRESHOLD {
                    self.stage = Stage::Idle;
                    0.0
                } else {
                    value
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// Times in ms and a sustain level, constant over the render.
    #[derive(Clone, Copy)]
    struct Settings {
        attack: f64,
        decay: f64,
        sustain: f64,
        release: f64,
    }

    /// The output for a gate that is on in the frames `start..end` of each
    /// pair in `on`, with triggers at `triggers`.
    fn envelope(
        sample_rate: f64,
        settings: Settings,
        on: &[(usize, usize)],
        triggers: &[u64],
        frames: usize,
        block: usize,
    ) -> Vec<f64> {
        let mut gate = vec![0.0; frames];
        for &(start, end) in on {
            gate[start..end].fill(1.0);
        }
        let parameter = |value| vec![value; frames];
        let (attack, decay, sustain, release) = (
            parameter(settings.attack),
            parameter(settings.decay),
            parameter(settings.sustain),
            parameter(settings.release),
        );
        let mut adsr = Adsr::default();
        adsr.prepare(&context(sample_rate, block, 0));
        let run = render(
            &mut adsr,
            &Timeline {
                signals: &[&gate],
                parameters: &[&attack, &decay, &sustain, &release],
                triggers: &[triggers],
            },
            frames,
            block,
        );
        run.signals.into_iter().next().unwrap()
    }

    /// 48 kHz: A = 96 frames, τd = 480, S = 0.5, τr = 960.
    const SETTINGS: Settings = Settings {
        attack: 2.0,
        decay: 10.0,
        sustain: 0.5,
        release: 20.0,
    };

    fn decay(t: usize, from: f64, tau: f64, sustain: f64) -> f64 {
        let e = libm::exp(-(t as f64) / tau);
        from * e + sustain * (1.0 - e)
    }

    /// AC4a: attack, decay and release values at chosen frames equal the
    /// formulas, and the release is flushed to exactly 0 below 1e-9.
    #[test]
    fn attack_decay_and_release_follow_the_formulas() {
        let (n0, n1, frames) = (100, 3000, 40_000);
        let out = envelope(48_000.0, SETTINGS, &[(n0, n1)], &[], frames, 37);
        let (a, tau_d, tau_r) = (96, 480.0, 960.0);
        assert!(out[..n0].iter().all(|&y| y == 0.0));
        for i in [0, 1, 17, 48, 95] {
            assert_eq!(out[n0 + i], i as f64 / a as f64, "attack frame {i}");
        }
        assert_eq!(out[n0 + a], 1.0);
        for t in [0, 1, 2, 240, 480, 1000, 2800] {
            assert_eq!(
                out[n0 + a + t],
                decay(t, 1.0, tau_d, 0.5),
                "decay frame {t}"
            );
        }
        let level = out[n1 - 1];
        for t in [0, 1, 2, 960, 5000, 15_000] {
            assert_eq!(
                out[n1 + t],
                level * libm::exp(-(t as f64) / tau_r),
                "release frame {t}"
            );
        }
        // L·exp(−t/960) < 1e-9 from t = ⌈960·ln(L/1e-9)⌉.
        let flushed = libm::ceil(tau_r * libm::log(level / 1e-9)) as usize;
        assert!(out[n1 + flushed - 1] >= 1e-9);
        assert!(out[n1 + flushed..].iter().all(|&y| y == 0.0));
    }

    #[test]
    fn attack_length_rounds_half_up() {
        // sr 1000: attack 2.5 ms is 2.5 frames, rounded up to 3.
        let settings = Settings {
            attack: 2.5,
            ..SETTINGS
        };
        let out = envelope(1000.0, settings, &[(2, 10)], &[], 10, 4);
        assert_eq!(&out[..6], [0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
    }

    /// AC4c: with A = 0 the output is exactly 1 on the edge frame, then decays.
    #[test]
    fn zero_attack_jumps_to_one_on_the_edge_frame() {
        let settings = Settings {
            attack: 0.0,
            sustain: 0.7,
            ..SETTINGS
        };
        let out = envelope(48_000.0, settings, &[(5, 100)], &[], 100, 16);
        assert_eq!(&out[..5], [0.0; 5]);
        assert_eq!(out[5], 1.0);
        assert_eq!(out[6], decay(1, 1.0, 480.0, 0.7));
    }

    /// The decay settles on exactly S, and a change of S glides from there.
    #[test]
    fn decay_settles_on_the_sustain_level() {
        let frames = 30_000;
        let out = envelope(48_000.0, SETTINGS, &[(0, frames)], &[], frames, 512);
        // (The attack passes 0.5 too, at frame 48.)
        let sustained = 96 + out[96..].iter().position(|&y| y == 0.5).unwrap();
        assert!(out[sustained..].iter().all(|&y| y == 0.5));
        // 0.5·exp(−t/480) < 1e-9 from t = ⌈480·ln(5e8)⌉ after the attack.
        assert_eq!(sustained, 96 + libm::ceil(480.0 * libm::log(5e8)) as usize);

        let mut gate = vec![1.0; 2000];
        gate[1500..].fill(0.0);
        let mut sustain = vec![0.5; 2000];
        sustain[1000..].fill(0.2);
        let mut adsr = Adsr::default();
        adsr.prepare(&context(1000.0, 64, 0));
        let ms = |v| vec![v; 2000];
        let (attack, decay_ms, release) = (ms(0.0), ms(10.0), ms(10.0));
        let out = render(
            &mut adsr,
            &Timeline {
                signals: &[&gate],
                parameters: &[&attack, &decay_ms, &sustain, &release],
                triggers: &[&[]],
            },
            2000,
            64,
        )
        .signals
        .remove(0);
        assert_eq!(out[999], 0.5);
        assert_eq!(out[1000], decay(1, 0.5, 10.0, 0.2));
        assert_eq!(out[1499], 0.2);
    }

    /// AC4b: gate-off in every stage (attack, decay, sustain) and retrigger in
    /// every stage (attack, decay, sustain, release; by a trigger and by the
    /// gate) start the next segment from the current level: no jump.
    #[test]
    fn retrigger_and_gate_off_are_continuous_in_every_stage() {
        let sample_rate = 48_000.0;
        let frames = 30_000;
        // A = 96, τd = 480 (settled after frame 9715), τr = 960.
        // (scenario, gate-on spans, triggers, edge frame)
        type Scenario<'a> = (&'a str, &'a [(usize, usize)], &'a [u64], usize);
        let scenarios: [Scenario; 8] = [
            ("gate-off in attack", &[(0, 40)], &[], 40),
            ("gate-off in decay", &[(0, 500)], &[], 500),
            ("gate-off in sustain", &[(0, 12_000)], &[], 12_000),
            ("trigger in attack", &[(0, frames)], &[50], 50),
            ("trigger in decay", &[(0, frames)], &[700], 700),
            ("trigger in sustain", &[(0, frames)], &[12_000], 12_000),
            (
                "gate rises in release",
                &[(0, 500), (1500, frames)],
                &[],
                1500,
            ),
            (
                "gate rises in attack's release",
                &[(0, 40), (60, frames)],
                &[],
                60,
            ),
        ];
        for (name, on, triggers, edge) in scenarios {
            let out = envelope(sample_rate, SETTINGS, on, triggers, frames, 64);
            let level = out[edge - 1];
            assert!(level > 0.0, "{name}: level {level}");
            assert_eq!(out[edge], level, "{name}: no jump at the edge");
            let rising = !name.starts_with("gate-off");
            let expected = if rising {
                level + (1.0 - level) * (1.0 / 96.0)
            } else {
                level * libm::exp(-1.0 / 960.0)
            };
            assert_eq!(
                out[edge + 1],
                expected,
                "{name}: the next segment's frame 1"
            );
            let largest_step = out
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f64::max);
            assert!(
                largest_step <= 1.0 / 96.0 + 1e-15,
                "{name}: step {largest_step}"
            );
        }
    }

    #[test]
    fn triggers_while_the_gate_is_off_are_ignored() {
        let out = envelope(48_000.0, SETTINGS, &[], &[0, 10, 500], 1000, 64);
        assert!(out.iter().all(|&y| y == 0.0));
    }
}
