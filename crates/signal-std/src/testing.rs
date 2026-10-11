//! Test-only driver for the analytic tests: runs one direct primitive over
//! whole-timeline inputs in calls of a fixed size, as the engine lends them.

use signal_core::{
    DirectPrimitive, Direction, Event, EventPayload, EventSink, Inputs, InstanceAddress, Outputs,
    PortShape, PrepareContext, RngStream,
};

/// The address the analytic tests derive random streams from.
pub(crate) const ADDRESS: &str = "/std_test";

/// A context at `sample_rate` for calls of up to `max_frames`, with the stream
/// of [`ADDRESS`] under `seed`.
pub(crate) fn context(sample_rate: f64, max_frames: usize, seed: u64) -> PrepareContext {
    PrepareContext {
        sample_rate,
        max_frames,
        rng: stream(seed),
    }
}

/// The random stream an instance at [`ADDRESS`] gets under `seed`.
pub(crate) fn stream(seed: u64) -> RngStream {
    let address: InstanceAddress = ADDRESS.parse().unwrap();
    RngStream::derive(seed, &address)
}

/// What a render produced.
#[derive(Debug, Default)]
pub(crate) struct Run {
    /// One sample per frame for every signal output.
    pub signals: Vec<Vec<f64>>,
    /// The frames of the triggers on every event output.
    pub events: Vec<Vec<u64>>,
}

/// The whole-timeline inputs of a render.
#[derive(Debug, Default)]
pub(crate) struct Timeline<'a> {
    /// One sample per frame for every signal input.
    pub signals: &'a [&'a [f64]],
    /// One effective value per frame for every parameter.
    pub parameters: &'a [&'a [f64]],
    /// Sorted trigger frames for every event input.
    pub triggers: &'a [&'a [u64]],
}

/// Renders `frames` frames of a prepared `primitive` in calls of `block` frames.
pub(crate) fn render<P: DirectPrimitive>(
    primitive: &mut P,
    timeline: &Timeline<'_>,
    frames: usize,
    block: usize,
) -> Run {
    let descriptor = P::describe();
    let count = |direction, shape| {
        descriptor
            .ports
            .iter()
            .filter(|p| p.direction == direction && p.shape == shape)
            .count()
    };
    let signal_outputs = count(Direction::Out, PortShape::Signal);
    let event_outputs = count(Direction::Out, PortShape::Events);
    let mut run = Run {
        signals: vec![Vec::with_capacity(frames); signal_outputs],
        events: vec![Vec::new(); event_outputs],
    };
    let mut sinks = vec![EventSink::with_capacity(16 * block); event_outputs];
    let mut start = 0;
    while start < frames {
        let k = block.min(frames - start);
        let end = start + k;
        let signals: Vec<f64> = timeline
            .signals
            .iter()
            .flat_map(|s| s[start..end].iter().copied())
            .collect();
        let parameters: Vec<f64> = timeline
            .parameters
            .iter()
            .flat_map(|p| p[start..end].iter().copied())
            .collect();
        let mut events = Vec::new();
        let mut event_ends = Vec::new();
        for triggers in timeline.triggers {
            for &frame in triggers.iter() {
                if (start as u64..end as u64).contains(&frame) {
                    events.push(Event {
                        offset: (frame - start as u64) as u32,
                        payload: EventPayload::Trigger,
                    });
                }
            }
            event_ends.push(events.len());
        }
        let mut out = vec![f64::NAN; signal_outputs * k];
        primitive.process(
            &Inputs::new(k, &signals, &parameters, &events, &event_ends),
            &mut Outputs::new(k, &mut out, &mut sinks),
        );
        for (samples, chunk) in run.signals.iter_mut().zip(out.chunks(k)) {
            samples.extend_from_slice(chunk);
        }
        for (frames, sink) in run.events.iter_mut().zip(sinks.iter_mut()) {
            assert_eq!(sink.overflows(), 0);
            frames.extend(
                sink.events()
                    .iter()
                    .map(|e| start as u64 + u64::from(e.offset)),
            );
            sink.clear();
        }
        start = end;
    }
    run
}
