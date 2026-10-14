//! The audio adapter in real time: a [`Runner`] that a driver's audio
//! callback calls with the device buffer, and its [`Controller`]. Each wraps
//! the engine's own (and dereferences to it); the adapter adds only the
//! audio policy (`crate::audio`): the sink's channels times its `gain`,
//! converted for the device, mapped to the same device channels, metered and
//! counted as clipped.
//!
//! [`Runner::process`] allocates, frees, locks and blocks nowhere.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use signal_core::{PortAddress, Registry};
use signal_engine::{ControlError, ControlSink, Engine, MakeWaiter, RealtimeConfig, SinkOutput};
use signal_graph::ExecutionPlan;

use crate::audio::{AudioSink, Meter, to_device};
use crate::scope::{ScopeBlock, ScopeControl, ScopeCounts, ScopeError, Scopes, scopes};

/// Builds a real-time run of `plan` with the audio adapter: the [`Runner`]
/// for the audio thread and the [`Controller`] for the control thread. The
/// plan must have exactly one audio sink with at least one signal input, as
/// for [`render`](crate::render).
pub fn realtime(
    plan: &ExecutionPlan,
    registry: &Registry,
    config: RealtimeConfig,
) -> Result<(Runner, Controller), ControlError> {
    realtime_with(plan, registry, config, None)
}

/// [`realtime`], with `make` making the workers' [`Waiter`](signal_engine::Waiter)
/// ([`signal_engine::realtime_with`]).
pub fn realtime_with(
    plan: &ExecutionPlan,
    registry: &Registry,
    config: RealtimeConfig,
    make: Option<MakeWaiter>,
) -> Result<(Runner, Controller), ControlError> {
    let (runner, controller) = signal_engine::realtime_with(plan, registry, config, make)?;
    let sink = AudioSink::select(runner.engine(), usize::MAX)
        .map_err(|error| ControlError::Sink(Box::new(error)))?;
    let (meter_tx, meter_rx) = RingBuffer::new(config.meters.max(1));
    let counts = Arc::new(Counts::default());
    let (scopes, scope_control) = scopes();
    let runner = Runner {
        runner,
        levels: vec![(0.0, 0.0); sink.width],
        sink,
        meters: meter_tx,
        clipped: 0,
        meter_overflows: 0,
        counts: Arc::clone(&counts),
        scopes,
    };
    let controller = Controller {
        controller,
        meters: meter_rx,
        counts,
        scopes: scope_control,
    };
    Ok((runner, controller))
}

/// What the runner counts for the control thread.
#[derive(Debug, Default)]
struct Counts {
    clipped: AtomicU64,
    meter_overflows: AtomicU64,
    scopes: ScopeCounts,
}

/// The audio-thread half of a real-time run: the engine's
/// [`Runner`](signal_engine::Runner) (which it dereferences to) with the
/// audio adapter. It is moved into a [`Driver`](crate::Driver), whose
/// callback calls [`process`](Runner::process).
pub struct Runner {
    runner: signal_engine::Runner,
    sink: AudioSink,
    /// Per sink channel: (peak, sum of squares) of the current callback.
    levels: Vec<(f32, f64)>,
    meters: Producer<Meter>,
    clipped: u64,
    meter_overflows: u64,
    counts: Arc<Counts>,
    /// The scope adapter's slots (`crate::scope`).
    scopes: Scopes,
}

impl Runner {
    /// Runs one callback: `out` holds `out.len() / channels` frames of
    /// `channels` interleaved device channels. The engine runs them
    /// ([`signal_engine::Runner::process`]); each frame's audio-sink channels
    /// (times the sink's `gain`) are written clamped to [−1, 1] as f32 to the
    /// same device channels, extra device channels 0; then a meter per sink
    /// channel is pushed. Frames the engine did not run (it stopped,
    /// `docs/semantics.md` §14) and a trailing partial frame are silence.
    ///
    /// The scopes started since the last callback take effect first, and
    /// capture the frames run (`crate::scope`).
    ///
    /// Allocates, frees, locks and blocks nowhere; a full meter or scope
    /// queue is counted.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        let frames = out.len().checked_div(channels).unwrap_or(0);
        let first_frame = self.runner.next_frame();
        self.levels.fill((0.0, 0.0));
        self.scopes.commands(self.runner.engine());
        let done = {
            let mut device = Device {
                sink: &mut self.sink,
                out: &mut *out,
                channels,
                at: 0,
                first: first_frame.0,
                levels: &mut self.levels,
                clipped: &mut self.clipped,
                scopes: &mut self.scopes,
            };
            self.runner.process(frames, &mut device)
        };
        if let Some(rest) = out.get_mut(done.saturating_mul(channels)..) {
            rest.fill(0.0);
        }
        if done > 0 {
            for (channel, &(peak, sum_squares)) in self.levels.iter().enumerate() {
                let meter = Meter {
                    first_frame,
                    frames: done as u64,
                    channel,
                    peak,
                    sum_squares,
                };
                if self.meters.push(meter).is_err() {
                    self.meter_overflows = self.meter_overflows.saturating_add(1);
                }
            }
        }
        self.counts.clipped.store(self.clipped, Ordering::Relaxed);
        self.counts
            .meter_overflows
            .store(self.meter_overflows, Ordering::Relaxed);
        self.scopes.publish(&self.counts.scopes);
    }

    /// Samples clamped to [−1, 1] (or non-finite, written as 0), since the
    /// start.
    pub fn clipped(&self) -> u64 {
        self.clipped
    }

    /// Meters dropped because the meter queue was full.
    pub fn meter_overflows(&self) -> u64 {
        self.meter_overflows
    }
}

impl Deref for Runner {
    type Target = signal_engine::Runner;

    fn deref(&self) -> &signal_engine::Runner {
        &self.runner
    }
}

impl DerefMut for Runner {
    fn deref_mut(&mut self) -> &mut signal_engine::Runner {
        &mut self.runner
    }
}

/// One callback's device buffer, as the engine's output.
struct Device<'a> {
    sink: &'a mut AudioSink,
    out: &'a mut [f32],
    channels: usize,
    /// The next frame of `out`.
    at: usize,
    /// The callback's first frame.
    first: u64,
    levels: &'a mut [(f32, f64)],
    clipped: &'a mut u64,
    scopes: &'a mut Scopes,
}

impl SinkOutput for Device<'_> {
    fn bind(&mut self, engine: &Engine) -> bool {
        self.sink.follow(engine);
        self.scopes.bind(engine);
        true
    }

    fn block(&mut self, engine: &Engine, frames: usize) {
        self.scopes
            .block(engine, self.first + self.at as u64, frames);
        let gain = self.sink.gain_index(engine);
        for f in 0..frames {
            let start = self.at.saturating_mul(self.channels);
            let Some(row) = self.out.get_mut(start..start + self.channels) else {
                break;
            };
            self.at += 1;
            row.fill(0.0);
            let gain = gain.and_then(|j| engine.parameter_at_frame(j, f));
            for channel in 0..self.sink.width {
                let x = engine
                    .sink_input_at(self.sink.id, channel, f)
                    .unwrap_or(0.0);
                let sample = to_device(gain.map_or(x, |g| g * x), self.clipped);
                if let Some(level) = self.levels.get_mut(channel) {
                    level.0 = level.0.max(sample.abs());
                    level.1 += f64::from(sample) * f64::from(sample);
                }
                if let Some(out) = row.get_mut(channel) {
                    *out = sample;
                }
            }
        }
    }
}

/// The control-thread half of a real-time run: the engine's
/// [`Controller`](signal_engine::Controller) (which it dereferences to) and
/// the audio adapter's meters and counts.
pub struct Controller {
    controller: signal_engine::Controller,
    meters: Consumer<Meter>,
    counts: Arc<Counts>,
    scopes: ScopeControl,
}

impl Controller {
    /// [`signal_engine::Controller::poll`], then drops the meters that
    /// arrived.
    pub fn poll(&mut self, sink: &mut impl ControlSink) {
        self.poll_with(sink, |_| {});
    }

    /// [`signal_engine::Controller::poll`], then hands each meter that
    /// arrived to `meter`. Allocates nothing (unless a hang is reported).
    pub fn poll_with(&mut self, sink: &mut impl ControlSink, mut meter: impl FnMut(&Meter)) {
        self.controller.poll(sink);
        while let Ok(m) = self.meters.pop() {
            meter(&m);
        }
    }

    /// The runner's clipped samples, as of its last callback.
    pub fn clipped(&self) -> u64 {
        self.counts.clipped.load(Ordering::Relaxed)
    }

    /// The runner's dropped meters, as of its last callback.
    pub fn meter_overflows(&self) -> u64 {
        self.counts.meter_overflows.load(Ordering::Relaxed)
    }

    /// Starts scope `slot` (below [`SCOPE_SLOTS`](crate::SCOPE_SLOTS)) on
    /// signal output or signal input `port`, one point every `decimation` frames, from the
    /// next callback; it replaces what the slot ran. Its blocks carry the
    /// generation returned.
    pub fn scope_start(
        &mut self,
        slot: usize,
        port: PortAddress,
        decimation: u64,
    ) -> Result<u32, ScopeError> {
        self.scopes.start(slot, port, decimation)
    }

    /// Stops scope `slot`; its blocks still queued are dropped.
    pub fn scope_stop(&mut self, slot: usize) -> Result<(), ScopeError> {
        self.scopes.stop(slot)
    }

    /// Hands each scope block that arrived to `block`. Allocates nothing.
    pub fn poll_scopes(&mut self, block: impl FnMut(&ScopeBlock)) {
        self.scopes.poll(block);
    }

    /// Whether scope `slot` found its port in the running plan, as of the
    /// runner's last callback.
    pub fn scope_bound(&self, slot: usize) -> bool {
        ScopeControl::bound(&self.counts.scopes, slot)
    }

    /// Scope blocks dropped because the scope queue was full.
    pub fn scope_overflows(&self) -> u64 {
        ScopeControl::overflows(&self.counts.scopes)
    }
}

impl Deref for Controller {
    type Target = signal_engine::Controller;

    fn deref(&self) -> &signal_engine::Controller {
        &self.controller
    }
}

impl DerefMut for Controller {
    fn deref_mut(&mut self) -> &mut signal_engine::Controller {
        &mut self.controller
    }
}
