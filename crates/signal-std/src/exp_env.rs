//! `std.exp_env`.

use signal_core::{
    Combine, DirectPrimitive, Direction, EventPayload, Feedthrough, Inputs, ModuleDescriptor,
    Outputs, PortKind, PrepareContext, Range,
};

use crate::{FLUSH_THRESHOLD, at, event_port, parameter, round_half_up, signal_port};

const ATTACK_MS: f64 = 0.3;
const DECAY_MS: f64 = 400.0;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.exp_env",
    doc: "Triggered envelope: a linear attack from the current level to 1, then an exponential decay.",
    ports: &[
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
            1000.0,
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
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
};

/// Where the envelope is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    /// Output 0 until a trigger.
    #[default]
    Idle,
    /// Frame `frame` (0-based) of a linear attack of `length` ≥ 1 frames from `from`.
    Attack { from: f64, frame: u64, length: u64 },
    /// Frame `frame` of a decay `peak·exp(−frame/tau)`.
    Decay { peak: f64, frame: u64, tau: f64 },
}

/// `std.exp_env`: an event-triggered envelope with a linear attack and an
/// exponential decay.
///
/// **Ports**
/// - `trigger` (event in, trigger): each frame with at least one `Trigger`
///   event starts the attack; several on one frame count once. `Set` and
///   `Ramp` events are ignored.
/// - `out` (signal out, control, `[0, 1]`).
///
/// **Parameters**
/// - `attack_ms` (control, ms, range `[0, 1000]`, default 0.3, combine `add`):
///   read on the trigger frame.
/// - `decay_ms` (control, ms, range `[0.1, 60 000]`, default 400, combine
///   `add`): read on every decay frame.
///
/// **Feedthrough:** direct (a trigger on frame *n* changes `out[n]`).
///
/// **Formula** (sr the sample rate; `L` the output of the frame before the
/// trigger, 0 at the start):
///
/// ```text
/// A = round_half_up(attack_ms·sr/1000)       frames, at the trigger frame n0
/// τ = decay_ms·sr/1000                       frames
/// out[n0 + i] = L + (1 − L)·(i/A)            0 ≤ i < A       (attack)
/// out[n0 + A + t] = 1·exp(−t/τ)              t ≥ 0           (decay)
/// ```
///
/// `round_half_up(x) = ⌊x + ½⌋`. With A = 0 the output is 1 on the trigger
/// frame and the decay starts there. A retrigger restarts the attack from the
/// current level `L`, so `out[n0] = out[n0 − 1]`: no jump. If `decay_ms`
/// changes during a decay, the decay continues from the last output with the
/// new τ (`peak` becomes that output and `t` restarts at 1), which keeps it
/// continuous; with a constant `decay_ms` the decay is exactly the formula.
/// When the output falls below 1e-9 it is flushed to exactly 0 and the envelope
/// is idle until the next trigger (`docs/dsp-conventions.md`, "Denormals").
/// `exp` is `libm::exp`. Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct ExpEnv {
    sample_rate: f64,
    stage: Stage,
    /// The output of the previous frame.
    level: f64,
}

impl DirectPrimitive for ExpEnv {
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
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let events = inputs.events(0);
        let attack_ms = inputs.parameter(0);
        let decay_ms = inputs.parameter(1);
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
            if triggered {
                let length =
                    round_half_up(at(attack_ms, n, ATTACK_MS) * self.sample_rate / 1000.0) as u64;
                self.stage = match length {
                    0 => Stage::Decay {
                        peak: 1.0,
                        frame: 0,
                        tau: 0.0,
                    },
                    _ => Stage::Attack {
                        from: self.level,
                        frame: 0,
                        length,
                    },
                };
            }
            let tau = at(decay_ms, n, DECAY_MS) * self.sample_rate / 1000.0;
            let value = self.advance(tau);
            self.level = value;
            *out = value;
        }
    }
}

impl ExpEnv {
    /// The output of the current frame, given its τ; moves to the next frame.
    fn advance(&mut self, tau: f64) -> f64 {
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
                        peak: 1.0,
                        frame: 0,
                        tau,
                    };
                }
                value
            }
            Stage::Decay {
                peak,
                frame,
                tau: current,
            } => {
                if *frame > 0 && tau != *current {
                    // A new τ: continue from the last output.
                    *peak = self.level;
                    *frame = 1;
                }
                *current = tau;
                let value = *peak * libm::exp(-(*frame as f64) / tau);
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

    fn envelope(
        sample_rate: f64,
        attack_ms: &[f64],
        decay_ms: &[f64],
        triggers: &[u64],
        block: usize,
    ) -> Vec<f64> {
        let mut env = ExpEnv::default();
        env.prepare(&context(sample_rate, block, 0));
        let run = render(
            &mut env,
            &Timeline {
                signals: &[],
                parameters: &[attack_ms, decay_ms],
                triggers: &[triggers],
            },
            attack_ms.len(),
            block,
        );
        run.signals.into_iter().next().unwrap()
    }

    fn constant(attack: f64, decay: f64, frames: usize) -> (Vec<f64>, Vec<f64>) {
        (vec![attack; frames], vec![decay; frames])
    }

    /// AC7: attack and decay values at chosen frames equal the formulas.
    #[test]
    fn attack_and_decay_follow_the_formulas() {
        let sample_rate = 48_000.0;
        let frames = 20_000;
        // A = round_half_up(2·48) = 96 frames, τ = 10·48 = 480 frames.
        let (attack, decay) = constant(2.0, 10.0, frames);
        let n0 = 100;
        let out = envelope(sample_rate, &attack, &decay, &[n0 as u64], 37);
        let (a, tau) = (96, 480.0);
        assert!(out[..n0].iter().all(|&y| y == 0.0));
        for i in [0, 1, 17, 48, 95] {
            assert_eq!(out[n0 + i], i as f64 / a as f64, "attack frame {i}");
        }
        for t in [0, 1, 2, 240, 480, 1000, 4800] {
            assert_eq!(
                out[n0 + a + t],
                libm::exp(-(t as f64) / tau),
                "decay frame {t}"
            );
        }
        assert_eq!(out[n0 + a], 1.0);
    }

    #[test]
    fn attack_length_rounds_half_up_and_zero_jumps_to_one() {
        // sr 1000: attack 2.5 ms is 2.5 frames, rounded up to 3.
        let (attack, decay) = constant(2.5, 100.0, 10);
        let out = envelope(1000.0, &attack, &decay, &[2], 4);
        assert_eq!(&out[..6], [0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);

        // A = 0: 1 on the trigger frame, then the decay.
        let (attack, decay) = constant(0.0, 100.0, 10);
        let out = envelope(1000.0, &attack, &decay, &[2], 4);
        assert_eq!(&out[..4], [0.0, 0.0, 1.0, libm::exp(-1.0 / 100.0)]);
    }

    /// AC7: a retrigger restarts the attack from the current level, with no jump,
    /// during the decay and during the attack.
    #[test]
    fn retrigger_is_continuous() {
        let sample_rate = 48_000.0;
        let frames = 6000;
        let (attack, decay) = constant(1.0, 20.0, frames); // A = 48, τ = 960
        for retrigger in [3000u64, 20] {
            let out = envelope(sample_rate, &attack, &decay, &[0, retrigger], 64);
            let m = retrigger as usize;
            let level = out[m - 1];
            assert!(level > 0.0 && level < 1.0);
            assert_eq!(out[m], level, "no jump at the retrigger");
            for i in 0..48 {
                let expected = level + (1.0 - level) * (i as f64 / 48.0);
                assert_eq!(out[m + i], expected, "attack frame {i} after the retrigger");
            }
            assert_eq!(out[m + 48], 1.0);
            let largest_step = out
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f64::max);
            assert!(largest_step <= 1.0 / 48.0 + 1e-15, "step {largest_step}");
        }
    }

    #[test]
    fn decay_flushes_to_exactly_zero_below_1e_9() {
        let frames = 30_000;
        let (attack, decay) = constant(0.0, 10.0, frames); // τ = 480 frames at 48 kHz
        let out = envelope(48_000.0, &attack, &decay, &[0], 512);
        assert!(out.iter().all(|&y| y == 0.0 || y >= 1e-9));
        let zero = out.iter().position(|&y| y == 0.0).unwrap();
        // exp(−t/480) < 1e-9 from t = ⌈480·ln 1e9⌉ = ⌈9947.18⌉ = 9948.
        assert_eq!(zero, 9948);
        assert!(out[zero..].iter().all(|&y| y == 0.0));
    }

    #[test]
    fn a_decay_time_change_continues_from_the_current_level() {
        let frames = 2000;
        let attack = vec![0.0; frames];
        let mut decay = vec![10.0; frames]; // τ = 10 frames at 1 kHz
        decay[500..].fill(100.0); // τ = 100 frames from frame 500
        let out = envelope(1000.0, &attack, &decay, &[490], 16);
        assert_eq!(out[499], libm::exp(-9.0 / 10.0));
        assert_eq!(out[500], out[499] * libm::exp(-1.0 / 100.0));
        assert_eq!(out[510], out[499] * libm::exp(-11.0 / 100.0));
    }
}
