//! The audio adapter in real time: a [`Runner`] that a driver's audio
//! callback calls with the device buffer, and its [`Controller`]. Each wraps
//! the engine's own (and dereferences to it); the adapter adds only the
//! audio policy (`crate::audio`): the sink's channels times its `gain`,
//! converted for the device, mapped to the same device channels, metered and
//! counted as clipped; with a [`ceiling`](Runner::set_ceiling) (`signal
//! mcp`'s `--max-gain`), every device sample is then held within it.
//!
//! [`Runner::process`] allocates, frees, locks and blocks nowhere.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use signal_core::{EventPayload, InstanceAddress, PortAddress, Registry};
use signal_engine::{
    CommandId, ControlError, ControlSink, Engine, MakeWaiter, RealtimeConfig, SinkOutput, Stamp,
};
use signal_graph::ExecutionPlan;

use crate::audio::{AudioSink, Meter, to_device};
use crate::fade::Fade;
use crate::scope::{ScopeBlock, ScopeControl, ScopeCounts, ScopeError, Scopes, scopes};
use crate::vectors::{
    VectorControl, VectorCounts, VectorError, VectorSnapshot, VectorUnbound, Vectors, vectors,
};

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
    let (vectors, vector_control) = vectors(plan);
    let runner = Runner {
        runner,
        levels: vec![(0.0, 0.0); sink.width],
        sink,
        meters: meter_tx,
        clipped: 0,
        ceiling: None,
        ceilinged: 0,
        meter_overflows: 0,
        counts: Arc::clone(&counts),
        scopes,
        vectors,
        fade: Fade::default(),
    };
    let controller = Controller {
        controller,
        meters: meter_rx,
        counts,
        scopes: scope_control,
        vectors: vector_control,
    };
    Ok((runner, controller))
}

/// What the runner counts for the control thread.
#[derive(Debug, Default)]
struct Counts {
    clipped: AtomicU64,
    ceilinged: AtomicU64,
    meter_overflows: AtomicU64,
    scopes: ScopeCounts,
    vectors: VectorCounts,
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
    /// The output ceiling: every device sample is held within ±it.
    ceiling: Option<f32>,
    /// Device samples the ceiling held, since the start.
    ceilinged: u64,
    meter_overflows: u64,
    counts: Arc<Counts>,
    /// The scope adapter's slots (`crate::scope`).
    scopes: Scopes,
    /// The vector adapter's slots (`crate::vectors`).
    vectors: Vectors,
    /// The device fade (`crate::fade`, ADR 0036): steady unless a driver
    /// asks.
    fade: Fade,
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
    /// The scopes and vector streams started since the last callback take
    /// effect first, and capture the frames run (`crate::scope`,
    /// `crate::vectors`).
    ///
    /// Allocates, frees, locks and blocks nowhere; a full meter or scope
    /// queue is counted.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        let frames = out.len().checked_div(channels).unwrap_or(0);
        let first_frame = self.runner.next_frame();
        self.levels.fill((0.0, 0.0));
        self.scopes.commands(self.runner.engine());
        self.vectors.commands(self.runner.engine());
        let done = {
            let mut device = Device {
                sink: &mut self.sink,
                out: &mut *out,
                channels,
                at: 0,
                first: first_frame.0,
                levels: &mut self.levels,
                clipped: &mut self.clipped,
                ceiling: self.ceiling,
                ceilinged: &mut self.ceilinged,
                scopes: &mut self.scopes,
                vectors: &mut self.vectors,
            };
            self.runner.process(frames, &mut device)
        };
        if let Some(rest) = out.get_mut(done.saturating_mul(channels)..) {
            rest.fill(0.0);
        }
        // After the meters' levels: they measure the project's output.
        self.fade.apply(out, channels);
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
            .ceilinged
            .store(self.ceilinged, Ordering::Relaxed);
        self.counts
            .meter_overflows
            .store(self.meter_overflows, Ordering::Relaxed);
        self.scopes.publish(&self.counts.scopes);
        self.vectors.publish(&self.counts.vectors);
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

    /// Sets the output ceiling (output policy, ADR 0022): from the next
    /// callback every device sample, after the conversion to `f32`, is held
    /// within ±`ceiling` (a sample outside it becomes ±`ceiling`, and is
    /// counted in [`ceilinged`](Runner::ceilinged)); the meters measure the
    /// held samples. `None` (the default) holds nothing, so the device gets
    /// exactly [`device_sample`](crate::device_sample). A ceiling that is not
    /// finite or not positive is taken as 0 (silence). `signal mcp` sets
    /// its `--max-gain` here (`docs/agent-interface.md` "Safety"); no
    /// other command sets one. Not a semantic of the project: offline
    /// renders never apply it.
    pub fn set_ceiling(&mut self, ceiling: Option<f64>) {
        self.ceiling = ceiling.map(ceiling_level);
    }

    /// The output ceiling ([`set_ceiling`](Runner::set_ceiling)).
    pub fn ceiling(&self) -> Option<f32> {
        self.ceiling
    }

    /// Device samples the ceiling held, since the start.
    pub fn ceilinged(&self) -> u64 {
        self.ceilinged
    }

    /// Ramps the device output in over `frames` frames from silence, from
    /// the next callback (ADR 0036 §4: a device driver asks when its stream
    /// starts). Output policy, after the conversion and the ceiling, never a
    /// semantic of the project: offline renders never fade, and a Runner no
    /// driver asks (the null driver's) plays exactly
    /// [`device_sample`](crate::device_sample). The meters measure the
    /// samples before the fade.
    pub fn fade_in(&mut self, frames: u32) {
        self.fade.start_in(frames);
    }

    /// Ramps the device output out over `frames` frames from the level it is
    /// at, from the next callback, then plays silence
    /// ([`faded_out`](Runner::faded_out)); a device driver asks before a
    /// deliberate stop (ADR 0036 §4). The engine runs on meanwhile.
    pub fn fade_out(&mut self, frames: u32) {
        self.fade.start_out(frames);
    }

    /// Whether a [`fade_out`](Runner::fade_out) has ended: silence from here.
    pub fn faded_out(&self) -> bool {
        self.fade.is_silent()
    }
}

/// The level a ceiling of `ceiling` holds samples within: `ceiling` as
/// `f32`, at most 1 (the device's own range), and 0 for one that is not
/// finite or not positive.
pub fn ceiling_level(ceiling: f64) -> f32 {
    if ceiling.is_finite() && ceiling > 0.0 {
        ceiling.min(1.0) as f32
    } else {
        0.0
    }
}

/// `sample` held within ±`ceiling` (`None`: as it is); a held sample is
/// counted. The ceiling's whole rule, shared with whoever checks a capture
/// against a replay. Allocates nothing.
pub fn hold(sample: f32, ceiling: Option<f32>, held: &mut u64) -> f32 {
    match ceiling {
        Some(c) if sample.abs() > c => {
            *held = held.saturating_add(1);
            sample.clamp(-c, c)
        }
        _ => sample,
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
    ceiling: Option<f32>,
    ceilinged: &'a mut u64,
    scopes: &'a mut Scopes,
    vectors: &'a mut Vectors,
}

impl SinkOutput for Device<'_> {
    fn bind(&mut self, engine: &Engine) -> bool {
        self.sink.follow(engine);
        self.scopes.bind(engine);
        self.vectors.bind(engine);
        true
    }

    fn block(&mut self, engine: &Engine, frames: usize) {
        self.scopes
            .block(engine, self.first + self.at as u64, frames);
        self.vectors
            .block(engine, self.first + self.at as u64, frames);
        let gain = self.sink.gain_index();
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
                let sample = hold(
                    to_device(gain.map_or(x, |g| g * x), self.clipped),
                    self.ceiling,
                    self.ceilinged,
                );
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
/// [`Controller`](signal_engine::Controller) and the audio adapter's meters
/// and counts. It dereferences to the engine's Controller for reading only:
/// every command goes through it, and every swap through
/// [`swap`](Controller::swap) or [`send_swap`](Controller::send_swap), so the
/// vector streams know each plan the run swaps to (M2.8-9). A swap around it
/// cannot be written:
///
/// ```compile_fail,E0596
/// fn bypass(controller: &mut signal_output::Controller) -> &mut signal_engine::Controller {
///     &mut **controller
/// }
/// ```
pub struct Controller {
    controller: signal_engine::Controller,
    meters: Consumer<Meter>,
    counts: Arc<Counts>,
    scopes: ScopeControl,
    vectors: VectorControl,
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

    /// [`signal_engine::Controller::send_event`].
    pub fn send_event(
        &mut self,
        target: PortAddress,
        payload: EventPayload,
        stamp: Stamp,
    ) -> Result<CommandId, ControlError> {
        self.controller.send_event(target, payload, stamp)
    }

    /// [`signal_engine::Controller::send_suspend`].
    pub fn send_suspend(
        &mut self,
        instance: InstanceAddress,
        stamp: Stamp,
    ) -> Result<CommandId, ControlError> {
        self.controller.send_suspend(instance, stamp)
    }

    /// [`signal_engine::Controller::send_resume`].
    pub fn send_resume(
        &mut self,
        instance: InstanceAddress,
        stamp: Stamp,
    ) -> Result<CommandId, ControlError> {
        self.controller.send_resume(instance, stamp)
    }

    /// [`signal_engine::Controller::prepare_swap`], keeping `plan` for the
    /// vector streams' verdicts.
    pub fn prepare_swap(&self, plan: &ExecutionPlan) -> Result<ReadySwap, ControlError> {
        Ok(ReadySwap {
            ready: self.controller.prepare_swap(plan)?,
            plan: plan.clone(),
        })
    }

    /// [`signal_engine::Controller::prepare_swap_after`], keeping `plan`
    /// for the vector streams' verdicts: a swap to send right after
    /// `previous` (ADR 0037 §6).
    pub fn prepare_swap_after(
        &self,
        previous: &ReadySwap,
        plan: &ExecutionPlan,
    ) -> Result<ReadySwap, ControlError> {
        Ok(ReadySwap {
            ready: self.controller.prepare_swap_after(&previous.ready, plan)?,
            plan: plan.clone(),
        })
    }

    /// [`signal_engine::Controller::send_swap`]: first, room permitting,
    /// every started vector slot's verdict for the plan version the swap
    /// makes, so it is there when the swap commits; withdrawn if the engine
    /// refuses the swap, sent after it if there was no room.
    pub fn send_swap(&mut self, ready: ReadySwap, stamp: Stamp) -> Result<CommandId, ControlError> {
        let running = self.controller.status().version;
        let early = self.vectors.before_swap(running, &ready.plan);
        match self.controller.send_swap(ready.ready, stamp) {
            Ok(id) => {
                self.vectors.swapped(&ready.plan, early);
                Ok(id)
            }
            Err(error) => {
                self.vectors.refused(early);
                Err(error)
            }
        }
    }

    /// [`prepare_swap`](Controller::prepare_swap), then
    /// [`send_swap`](Controller::send_swap).
    pub fn swap(&mut self, plan: &ExecutionPlan, stamp: Stamp) -> Result<CommandId, ControlError> {
        let ready = self.prepare_swap(plan)?;
        self.send_swap(ready, stamp)
    }

    /// The runner's clipped samples, as of its last callback.
    pub fn clipped(&self) -> u64 {
        self.counts.clipped.load(Ordering::Relaxed)
    }

    /// Device samples the runner's ceiling held, as of its last callback
    /// ([`Runner::set_ceiling`]).
    pub fn ceilinged(&self) -> u64 {
        self.counts.ceilinged.load(Ordering::Relaxed)
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

    /// Starts vector slot `slot` (below [`VECTOR_SLOTS`](crate::VECTOR_SLOTS))
    /// on the `rows` rows of vector output `port`, one snapshot at every
    /// frame that is a multiple of `decimation`, from the next callback; it
    /// replaces what the slot ran. Its snapshots carry the generation
    /// returned. `decimation` must be a multiple of the port's rate divisor
    /// in `plan` (the plan the caller starts it for), when `plan` has it; in
    /// the running plan and every one swapped to later it binds only where
    /// it is ([`vector_unbound`](Controller::vector_unbound) says why not).
    pub fn vector_start(
        &mut self,
        plan: &ExecutionPlan,
        slot: usize,
        port: PortAddress,
        rows: usize,
        decimation: u64,
    ) -> Result<u32, VectorError> {
        let running = self.controller.status().version;
        self.vectors
            .start(running, plan, slot, port, rows, decimation)
    }

    /// Stops vector slot `slot`; its snapshots still queued are dropped.
    pub fn vector_stop(&mut self, slot: usize) -> Result<(), VectorError> {
        self.vectors.stop(slot)
    }

    /// Hands each vector snapshot that arrived to `snapshot`. Allocates
    /// nothing.
    pub fn poll_vectors(&mut self, snapshot: impl FnMut(&VectorSnapshot)) {
        self.vectors.poll(snapshot);
    }

    /// Whether vector slot `slot` found its port in the running plan, as of
    /// the runner's last callback.
    pub fn vector_bound(&self, slot: usize) -> bool {
        VectorControl::bound(&self.counts.vectors, slot)
    }

    /// Why started vector slot `slot` is not bound, as of the runner's last
    /// callback; `None` when it is bound or not started.
    pub fn vector_unbound(&self, slot: usize) -> Option<VectorUnbound> {
        VectorControl::unbound(&self.counts.vectors, slot)
    }

    /// Vector snapshots dropped because the vector queue was full.
    pub fn vector_overflows(&self) -> u64 {
        VectorControl::overflows(&self.counts.vectors)
    }
}

impl Deref for Controller {
    type Target = signal_engine::Controller;

    fn deref(&self) -> &signal_engine::Controller {
        &self.controller
    }
}

/// A swap prepared by [`Controller::prepare_swap`], with its plan.
pub struct ReadySwap {
    ready: signal_engine::ReadySwap,
    plan: ExecutionPlan,
}
