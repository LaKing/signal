//! What the checks feed a module: a whole timeline of inputs, generated from a
//! seed so that every failure can be reproduced from the seed it prints.

use signal_core::{
    Curve, Direction, EventPayload, ModuleDescriptor, PortShape, Range, SplitMix64,
    Xoshiro256PlusPlus,
};

/// The range drawn from for signal inputs and event values without a declared range.
const DEFAULT_RANGE: Range = Range {
    min: -1.0,
    max: 1.0,
};

/// The inputs of a module for every frame of a render, in the groups and
/// descriptor order of the module's [`Inputs`](signal_core::Inputs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stimulus {
    /// One sample per frame for every signal input port.
    pub signals: Vec<Vec<f64>>,
    /// The effective value per frame of every parameter, within its range.
    pub parameters: Vec<Vec<f64>>,
    /// For every event input port, `(frame, payload)` pairs sorted by frame.
    pub events: Vec<Vec<(u64, EventPayload)>>,
}

/// A stimulus generator: the stimulus of `frames` frames for a module type,
/// determined by `seed`. [`random`] is the default; a module that needs a
/// particular input shape passes its own to
/// [`contract_tests!`](crate::contract_tests).
pub type StimulusFn = fn(descriptor: &ModuleDescriptor, seed: u64, frames: usize) -> Stimulus;

/// The default generator: random values within the declared ranges.
///
/// - Signal inputs: the range endpoints and 0 (when in range) on the first
///   frames, then mostly uniform values with endpoints and 0 mixed in. A port
///   without a range draws from `[-1, 1]`.
/// - Parameters: effective values shaped as the executor makes them (ADR 0016
///   §4a): segments that start with a `set` (jump) or a linear `ramp` from the
///   current value, reaching the target exactly. The first three segments set
///   the minimum, the maximum and 0 (or the default when 0 is out of range).
/// - Event inputs: triggers, sets and ramps at random frames, sometimes
///   several on one frame.
pub fn random(descriptor: &ModuleDescriptor, seed: u64, frames: usize) -> Stimulus {
    let mut draw = Draw::new(seed);
    let mut stimulus = Stimulus::default();
    for port in inputs(descriptor, PortShape::Signal) {
        let range = port.range.unwrap_or(DEFAULT_RANGE);
        let corners = corners(range, None);
        let samples = (0..frames)
            .map(|n| match corners.get(n) {
                Some(&value) => value,
                None => draw.value(range),
            })
            .collect();
        stimulus.signals.push(samples);
    }
    for parameter in descriptor.parameters {
        let range = parameter.range;
        let corners = corners(range, Some(parameter.default));
        stimulus.parameters.push(parameter_curve(
            &mut draw,
            range,
            parameter.default,
            &corners,
            frames,
        ));
    }
    for port in inputs(descriptor, PortShape::Events) {
        let range = port.range.unwrap_or(DEFAULT_RANGE);
        let mut events = Vec::new();
        for frame in 0..frames as u64 {
            if draw.below(16) == 0 {
                for _ in 0..=draw.below(2) {
                    events.push((frame, draw.payload(range)));
                }
            }
        }
        stimulus.events.push(events);
    }
    stimulus
}

/// One impulse followed by silence, for the subnormal check: every signal input
/// is 1 (clamped into its range) on frame 0 and 0 (or the in-range value
/// nearest 0) afterwards, every event input gets one trigger on frame 0, and
/// every parameter holds its default. When clamping 1 lands on the silence
/// value (a range such as `[-1, 0]`), the impulse is the endpoint farther from
/// silence, so it differs from silence whenever the range is not a single point.
pub fn impulse(descriptor: &ModuleDescriptor, frames: usize) -> Stimulus {
    let mut stimulus = Stimulus::default();
    for port in inputs(descriptor, PortShape::Signal) {
        let range = port.range.unwrap_or(DEFAULT_RANGE);
        let silence = clamp(0.0, range);
        let mut peak = clamp(1.0, range);
        if peak == silence {
            peak = if range.max - silence >= silence - range.min {
                range.max
            } else {
                range.min
            };
        }
        let mut samples = vec![silence; frames];
        if let Some(first) = samples.first_mut() {
            *first = peak;
        }
        stimulus.signals.push(samples);
    }
    for parameter in descriptor.parameters {
        let value = clamp(parameter.default, parameter.range);
        stimulus.parameters.push(vec![value; frames]);
    }
    for _ in inputs(descriptor, PortShape::Events) {
        stimulus.events.push(vec![(0, EventPayload::Trigger)]);
    }
    stimulus
}

/// `stimulus` with every input of frame `frame` changed: each signal input and
/// parameter moves to another in-range value (unless its range is a single
/// point), and each event input gets an extra trigger. For the state-output
/// honesty check.
pub(crate) fn perturb(
    descriptor: &ModuleDescriptor,
    stimulus: &Stimulus,
    frame: usize,
) -> Stimulus {
    let mut perturbed = stimulus.clone();
    let signal_ranges =
        inputs(descriptor, PortShape::Signal).map(|p| p.range.unwrap_or(DEFAULT_RANGE));
    for (samples, range) in perturbed.signals.iter_mut().zip(signal_ranges) {
        if let Some(x) = samples.get_mut(frame) {
            *x = other_value(*x, range);
        }
    }
    let parameter_ranges = descriptor.parameters.iter().map(|p| p.range);
    for (values, range) in perturbed.parameters.iter_mut().zip(parameter_ranges) {
        if let Some(x) = values.get_mut(frame) {
            *x = other_value(*x, range);
        }
    }
    for events in &mut perturbed.events {
        let at = events.partition_point(|&(f, _)| f <= frame as u64);
        events.insert(at, (frame as u64, EventPayload::Trigger));
    }
    perturbed
}

/// Whether `stimulus` has the shape `descriptor` asks for, covering `frames` frames.
pub(crate) fn check_shape(
    descriptor: &ModuleDescriptor,
    stimulus: &Stimulus,
    frames: usize,
) -> Result<(), String> {
    let groups = [
        (
            "signal inputs",
            inputs(descriptor, PortShape::Signal).count(),
            stimulus.signals.len(),
        ),
        (
            "parameters",
            descriptor.parameters.len(),
            stimulus.parameters.len(),
        ),
        (
            "event inputs",
            inputs(descriptor, PortShape::Events).count(),
            stimulus.events.len(),
        ),
    ];
    for (group, declared, given) in groups {
        if declared != given {
            return Err(format!(
                "the stimulus has {given} {group}, the descriptor declares {declared}"
            ));
        }
    }
    let lengths = stimulus.signals.iter().chain(&stimulus.parameters);
    if let Some(short) = lengths.map(Vec::len).find(|&len| len < frames) {
        return Err(format!(
            "the stimulus covers {short} frames, the check renders {frames}"
        ));
    }
    if stimulus
        .events
        .iter()
        .any(|events| events.windows(2).any(|w| w[0].0 > w[1].0))
    {
        return Err("the stimulus has events that are not sorted by frame".to_owned());
    }
    Ok(())
}

fn inputs(
    descriptor: &ModuleDescriptor,
    shape: PortShape,
) -> impl Iterator<Item = &'static signal_core::PortDescriptor> {
    descriptor
        .ports
        .iter()
        .filter(move |p| p.direction == Direction::In && p.shape == shape)
}

/// The minimum, the maximum, and 0 if it is in range (else `fallback`, if given).
fn corners(range: Range, fallback: Option<f64>) -> Vec<f64> {
    let middle = if contains(range, 0.0) {
        Some(0.0)
    } else {
        fallback.map(|v| clamp(v, range))
    };
    [Some(range.min), Some(range.max), middle]
        .into_iter()
        .flatten()
        .collect()
}

fn parameter_curve(
    draw: &mut Draw,
    range: Range,
    default: f64,
    corners: &[f64],
    frames: usize,
) -> Vec<f64> {
    let mut values = Vec::with_capacity(frames);
    let mut base = clamp(default, range);
    let mut segment = 0;
    while values.len() < frames {
        let remaining = frames - values.len();
        let length = if segment < corners.len() {
            1 + draw.below(16)
        } else {
            1 + draw.below(512)
        }
        .min(remaining);
        let target = match corners.get(segment) {
            Some(&corner) => corner,
            None => draw.value(range),
        };
        if segment < corners.len() || draw.below(2) == 0 {
            // `set`: the base jumps at the segment start.
            base = target;
            values.extend(std::iter::repeat_n(base, length));
        } else {
            // `ramp(target, N)`: base stays `b` on the start frame and is
            // `b + (to - b) * c(i / N)` until it is `to` exactly (ADR 0016 §4a).
            let start = base;
            let ramp_frames = 1 + draw.below(length);
            for i in 0..length {
                let value = if i < ramp_frames {
                    start + (target - start) * Curve::Linear.eval(i as f64 / ramp_frames as f64)
                } else {
                    target
                };
                values.push(clamp(value, range));
            }
            base = target;
        }
        segment += 1;
    }
    values
}

fn contains(range: Range, value: f64) -> bool {
    range.min <= value && value <= range.max
}

fn clamp(value: f64, range: Range) -> f64 {
    value.max(range.min).min(range.max)
}

/// Another value in `range` than `value`: an endpoint, or `value` itself when
/// the range is a single point.
fn other_value(value: f64, range: Range) -> f64 {
    if value != range.max {
        range.max
    } else {
        range.min
    }
}

/// Deterministic draws from Xoshiro256++ seeded through SplitMix64.
struct Draw {
    rng: Xoshiro256PlusPlus,
}

impl Draw {
    fn new(seed: u64) -> Draw {
        let mut seeder = SplitMix64::new(seed);
        let state = [
            seeder.next_u64(),
            seeder.next_u64(),
            seeder.next_u64(),
            seeder.next_u64(),
        ];
        Draw {
            rng: Xoshiro256PlusPlus::from_state(state),
        }
    }

    /// Uniform in `0..n` (`n` ≥ 1; slightly biased, which does not matter here).
    fn below(&mut self, n: usize) -> usize {
        (self.rng.next_u64() % n.max(1) as u64) as usize
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A value in `range`: the minimum, the maximum or 0 (when in range) with
    /// probability 1/8 each, else uniform.
    fn value(&mut self, range: Range) -> f64 {
        match self.below(8) {
            0 => range.min,
            1 => range.max,
            2 if contains(range, 0.0) => 0.0,
            _ => {
                let u = self.unit();
                clamp(range.min * (1.0 - u) + range.max * u, range)
            }
        }
    }

    fn payload(&mut self, range: Range) -> EventPayload {
        match self.below(3) {
            0 => EventPayload::Trigger,
            1 => EventPayload::Set(self.value(range)),
            _ => EventPayload::Ramp {
                to: self.value(range),
                frames: self.below(1024) as u64,
                curve: Curve::Linear,
            },
        }
    }
}
