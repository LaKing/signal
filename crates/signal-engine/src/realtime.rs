//! The real-time runner (M2-3, ADR 0006, 0011): a [`Runner`] that a driver's
//! audio callback calls, and the [`Controller`] that feeds it from the control
//! thread. They share nothing but `rtrb` SPSC queues (`docs/architecture.md`,
//! "Threads"). Normative description: `docs/realtime-rules.md`, "The runner",
//! and `docs/semantics.md` §12, "Real-time runner".
//!
//! [`Runner::process`] allocates, frees, locks and blocks nowhere: every
//! queue and buffer is sized in [`realtime`], and everything that owns heap
//! memory (pushed events and addresses, replaced plans) goes back to the
//! control thread to be dropped there.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, PushError, RingBuffer};
use signal_core::{EventPayload, Frame, InstanceAddress, PortAddress, Registry};
use signal_graph::ExecutionPlan;

use crate::engine::{
    EditFull, Engine, EngineConfig, ExternalEvent, ExternalFull, InputDiscards, ParameterValue,
    PlanState, PreparedSwap, SwapTarget, prepare_swap,
};
use crate::error::{EngineError, RenderError};
use crate::fade::fade_frames;
use crate::log::{InstanceId, LogLine, LogRecord};
use crate::offline::GAIN;

/// The capacities and timing of a real-time run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RealtimeConfig {
    /// The engine's configuration. Its `queue_capacity` also bounds the
    /// commands in flight.
    pub engine: EngineConfig,
    /// The device's nominal buffer in frames.
    pub buffer_frames: u64,
    /// What [`Stamp::Default`] adds to the current frame.
    pub latency_frames: u64,
    /// The command queue's capacity, and so the most commands in flight.
    pub commands: usize,
    /// The log queue's capacity, in records.
    pub log: usize,
    /// The meter queue's capacity, in [`Meter`]s.
    pub meters: usize,
    /// The parameter queue's capacity, in parameter values.
    pub parameters: usize,
    /// The status queue's capacity.
    pub status: usize,
}

impl RealtimeConfig {
    /// The default [`buffer_frames`](RealtimeConfig::buffer_frames).
    pub const DEFAULT_BUFFER_FRAMES: u64 = 512;

    /// A configuration with default capacities and a latency of two buffers
    /// of [`DEFAULT_BUFFER_FRAMES`](RealtimeConfig::DEFAULT_BUFFER_FRAMES).
    pub fn new(sample_rate: u32, seed: u64) -> RealtimeConfig {
        RealtimeConfig::with_buffer(sample_rate, seed, RealtimeConfig::DEFAULT_BUFFER_FRAMES)
    }

    /// A configuration for a device buffer of `buffer_frames`: the latency
    /// is two buffers.
    pub fn with_buffer(sample_rate: u32, seed: u64, buffer_frames: u64) -> RealtimeConfig {
        RealtimeConfig {
            engine: EngineConfig::new(sample_rate, seed),
            buffer_frames,
            latency_frames: buffer_frames.saturating_mul(2),
            commands: 1024,
            log: 1 << 16,
            meters: 1024,
            parameters: 1024,
            status: 256,
        }
    }
}

/// How many parameter values the runner reports per callback at most. It
/// goes round the plan's parameters, so each is reported at least every
/// `parameters / PARAMETERS_PER_CALLBACK` callbacks (rounded up).
pub const PARAMETERS_PER_CALLBACK: usize = 64;

/// The id of a command sent through the [`Controller`]: 1, 2, 3, … in send
/// order. An event's id is also its sequence number.
pub type CommandId = u64;

/// When a command is requested to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// The current frame (as last reported) plus the latency.
    Default,
    /// This frame.
    At(Frame),
}

/// A command on its way to the audio thread.
#[derive(Debug)]
pub enum Command {
    /// An external event, stamped with its requested frame.
    Event(ExternalEvent),
    /// A suspend of an instance.
    Suspend(InstanceAddress),
    /// A resume of an instance.
    Resume(InstanceAddress),
    /// A prepared plan swap.
    Swap(Box<PreparedSwap>),
}

/// A command and the control side's copy of what it is, kept until it is
/// reported.
#[derive(Clone, Debug, PartialEq)]
pub enum SentCommand {
    /// An external event; its `frame` is the requested frame.
    Event(ExternalEvent),
    /// A suspend.
    Suspend(InstanceAddress),
    /// A resume.
    Resume(InstanceAddress),
    /// A swap to a plan of this [`ExecutionPlan::version`].
    Swap {
        /// The swapped-in plan's own version.
        plan_version: u64,
    },
}

/// What became of a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An event, suspend or resume was handed to the engine; it applies at
    /// the report's frame.
    Scheduled,
    /// A swap was committed at the report's frame; the engine's plan version
    /// is now `version`.
    Committed {
        /// The engine's plan version after the swap.
        version: u64,
    },
    /// The engine refused it; it has no effect.
    Refused,
}

/// A command's outcome, from the audio thread. A command whose `frame` is
/// after its `requested` frame was late: it applies on the next frame that
/// had not yet run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// The command.
    pub id: CommandId,
    /// The frame it was stamped for.
    pub requested: Frame,
    /// Its effective frame (for a refusal, the next frame when it was refused).
    pub frame: Frame,
    /// What became of it.
    pub outcome: Outcome,
}

impl Report {
    /// Whether it applies after the frame it was stamped for.
    pub fn is_late(&self) -> bool {
        self.frame > self.requested
    }
}

/// One audio-sink channel over one callback, after the f32 conversion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meter {
    /// The callback's first frame.
    pub first_frame: Frame,
    /// The callback's number of frames.
    pub frames: u64,
    /// The sink channel (its signal input, in port order).
    pub channel: usize,
    /// The largest magnitude.
    pub peak: f32,
    /// The sum of the squared samples.
    pub sum_squares: f64,
}

/// The runner's counters, cumulative since the start, and where it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunnerStatus {
    /// The next frame the runner will run.
    pub next_frame: Frame,
    /// The engine's plan version.
    pub version: u64,
    /// Callbacks run.
    pub callbacks: u64,
    /// Samples clamped to [−1, 1] (or non-finite, written as 0).
    pub clipped: u64,
    /// Commands that applied after the frame they were stamped for.
    pub late: u64,
    /// Commands the engine refused (a full engine queue, a refused commit).
    pub refused: u64,
    /// Log records dropped because the log queue was full.
    pub log_overflows: u64,
    /// Meters dropped because the meter queue was full.
    pub meter_overflows: u64,
    /// Parameter values dropped because the parameter queue was full.
    pub parameter_overflows: u64,
    /// Statuses not sent because the status queue was full (or returns were
    /// held back).
    pub status_overflows: u64,
    /// Returned items held on the audio thread because the return queue was
    /// full. The in-flight limit keeps this at 0.
    pub return_stalls: u64,
    /// The engine's [`log_overflows`](Engine::log_overflows).
    pub engine_log_overflows: u64,
    /// The engine's [`input_overflows`](Engine::input_overflows).
    pub input_overflows: u64,
    /// The engine's [`input_discards`](Engine::input_discards).
    pub input_discards: InputDiscards,
}

/// What [`Controller::poll`] hands out. Every method has an empty default.
pub trait ControlSink {
    /// A command's outcome, with the control side's copy of the command.
    fn report(&mut self, _report: &Report, _command: &SentCommand) {}
    /// A log record of a frame, with addresses.
    fn log(&mut self, _line: LogLine<'_>) {}
    /// A meter of one callback and channel.
    fn meter(&mut self, _meter: &Meter) {}
    /// A parameter's base and effective value at the end of a callback
    /// (observation only: it never feeds back into the run).
    fn parameter(&mut self, _port: &PortAddress, _value: ParameterValue) {}
    /// Called last: every command whose effective frame is before `next_frame`
    /// has been reported (if a status reached the control thread).
    fn frontier(&mut self, _next_frame: Frame) {}
}

/// Discards everything.
impl ControlSink for () {}

/// Why a [`Controller`] refused something, or [`realtime`] a plan.
#[derive(Debug)]
pub enum ControlError {
    /// The engine refused the plan.
    Engine(EngineError),
    /// The plan does not have exactly one audio sink, or a swap changes it:
    /// a [`RenderError`] that says how.
    Sink(RenderError),
    /// The command queue is full, or as many commands as the queues can
    /// return are in flight; the command is handed back.
    Full(Command),
    /// A swap was refused at its commit, so the Controller's mirror no longer
    /// matches the engine: no further swap is sent.
    Desynchronised,
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::Engine(error) => write!(f, "{error}"),
            ControlError::Sink(error) => write!(f, "{error}"),
            ControlError::Full(_) => f.write_str("the command queue is full"),
            ControlError::Desynchronised => {
                f.write_str("a swap was refused at its commit; no further swap is sent")
            }
        }
    }
}

impl std::error::Error for ControlError {}

/// A command in the queue.
#[derive(Debug)]
struct Queued {
    id: CommandId,
    requested: Frame,
    command: Command,
}

/// What the audio thread sends back: reports, and what is dropped on the
/// control thread.
#[derive(Debug)]
enum Back {
    Report(Report),
    Event(ExternalEvent),
    Address(InstanceAddress),
    Swap(Box<PreparedSwap>),
}

/// A swap prepared on the control thread by [`Controller::prepare_swap`],
/// not yet sent.
pub struct ReadySwap {
    /// The [`Controller`] that prepared it ([`Controller::send_swap`] sends
    /// it through that one only).
    controller: u64,
    swap: Box<PreparedSwap>,
    mirror: PlanState,
    plan_version: u64,
    target_version: u64,
}

/// A swap waiting for its frame.
#[derive(Debug)]
struct PendingSwap {
    id: CommandId,
    requested: Frame,
    frame: Frame,
    swap: Box<PreparedSwap>,
}

/// Builds a real-time run of `plan`: the [`Runner`] for the audio thread and
/// the [`Controller`] for the control thread. The plan must have exactly one
/// audio sink with at least one signal input, as for [`render`](crate::render).
pub fn realtime(
    plan: &ExecutionPlan,
    registry: &Registry,
    config: RealtimeConfig,
) -> Result<(Runner, Controller), ControlError> {
    let engine =
        Engine::new(plan, registry, config.engine, Vec::new()).map_err(ControlError::Engine)?;
    let mirror = PlanState::mirror(plan, registry, config.engine).map_err(ControlError::Engine)?;
    let sinks = engine.audio_sinks();
    let sink = match sinks.as_slice() {
        [sink] => *sink,
        [] => return Err(ControlError::Sink(RenderError::NoAudioSink)),
        several => {
            let instances = several
                .iter()
                .filter_map(|&s| engine.instance_address(s).cloned())
                .collect();
            return Err(ControlError::Sink(RenderError::SeveralAudioSinks {
                instances,
            }));
        }
    };
    let address = engine
        .instance_address(sink)
        .cloned()
        .ok_or(ControlError::Sink(RenderError::NoAudioSink))?;
    let width = engine.instance_inputs(sink).len();
    if width == 0 {
        return Err(ControlError::Sink(RenderError::AudioChannels {
            instance: address,
            channels: width,
        }));
    }
    let gain = PortAddress::new(&address, GAIN)
        .ok()
        .filter(|port| engine.parameter(port).is_some());

    // Every command returns one report and one item to drop: the return
    // queue holds twice the commands in flight.
    let limit = config.commands.min(config.engine.queue_capacity).max(1);
    let (command_tx, command_rx) = RingBuffer::new(limit);
    let (back_tx, back_rx) = RingBuffer::new(2 * limit);
    let (log_tx, log_rx) = RingBuffer::new(config.log.max(1));
    let (meter_tx, meter_rx) = RingBuffer::new(config.meters.max(1));
    let (parameter_tx, parameter_rx) = RingBuffer::new(config.parameters.max(1));
    let (status_tx, status_rx) = RingBuffer::new(config.status.max(1));
    let target = engine.swap_target();

    let runner = Runner {
        engine,
        sink,
        gain,
        commands: command_rx,
        back: back_tx,
        log: log_tx,
        meters: meter_tx,
        parameters: parameter_tx,
        next_parameter: 0,
        status: status_tx,
        pending: VecDeque::with_capacity(limit),
        held: VecDeque::with_capacity(2 * limit),
        levels: vec![(0.0, 0.0); width],
        last_event: Frame::ZERO,
        last_edit: Frame::ZERO,
        last_swap: None,
        counts: RunnerStatus::default(),
    };
    let controller = Controller {
        commands: command_tx,
        back: back_rx,
        log: log_rx,
        meters: meter_rx,
        parameters: parameter_rx,
        status: status_rx,
        registry: registry.clone(),
        target,
        mirrors: VecDeque::from([(0, mirror)]),
        sink: (address, width),
        latency: config.latency_frames,
        fade: fade_frames(config.engine.sample_rate),
        next_id: 1,
        sent: BTreeMap::new(),
        outstanding: 0,
        limit,
        latest: RunnerStatus::default(),
        command_overflows: 0,
        desynchronised: false,
        id: NEXT_CONTROLLER.fetch_add(1, Ordering::Relaxed),
    };
    Ok((runner, controller))
}

/// The audio-thread half of a real-time run: owns the [`Engine`] and is
/// moved into a [`Driver`](crate::Driver), whose callback calls
/// [`process`](Runner::process).
pub struct Runner {
    engine: Engine,
    sink: InstanceId,
    gain: Option<PortAddress>,
    commands: Consumer<Queued>,
    back: Producer<Back>,
    log: Producer<(u64, LogRecord)>,
    meters: Producer<Meter>,
    /// (plan version, parameter index, value).
    parameters: Producer<(u64, usize, ParameterValue)>,
    /// The parameter the next callback reports first.
    next_parameter: usize,
    status: Producer<RunnerStatus>,
    /// Swaps waiting for their frame, in frame order.
    pending: VecDeque<PendingSwap>,
    /// Returned items the return queue had no room for (never dropped here).
    held: VecDeque<Back>,
    /// Per sink channel: (peak, sum of squares) of the current callback.
    levels: Vec<(f32, f64)>,
    last_event: Frame,
    last_edit: Frame,
    last_swap: Option<Frame>,
    counts: RunnerStatus,
}

impl Runner {
    /// Runs one callback: `out` holds `out.len() / channels` frames of
    /// `channels` interleaved device channels. In order:
    /// 1. takes the queued commands, each at its effective frame: events,
    ///    suspends and resumes go to the engine, swaps wait for their frame;
    /// 2. runs the frames one at a time, committing a swap at its frame;
    ///    writes each frame's audio-sink channels (times the sink's `gain`)
    ///    clamped to [−1, 1] as f32 to the same device channels, extra device
    ///    channels 0; and pushes the frame's log records;
    /// 3. pushes a meter per sink channel, the base and effective value of
    ///    up to [`PARAMETERS_PER_CALLBACK`] parameters (going round the plan's
    ///    parameters), returns used input for dropping, and pushes the status.
    ///
    /// Allocates, frees, locks and blocks nowhere; a full log, meter or
    /// status queue is counted. A trailing partial frame in `out` is zeroed.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        self.collect();
        let first_frame = self.engine.next_frame();
        self.levels.fill((0.0, 0.0));
        let mut frames = 0u64;
        if channels == 0 {
            out.fill(0.0);
        } else {
            let mut rows = out.chunks_exact_mut(channels);
            for row in &mut rows {
                self.frame(row);
                frames += 1;
            }
            rows.into_remainder().fill(0.0);
        }
        self.finish(first_frame, frames);
    }

    /// The next frame it will run.
    pub fn next_frame(&self) -> Frame {
        self.engine.next_frame()
    }

    /// Its counters now (what the next status will say).
    pub fn status(&self) -> RunnerStatus {
        RunnerStatus {
            next_frame: self.engine.next_frame(),
            version: self.engine.version(),
            ..self.counts
        }
    }

    /// Takes the queued commands, unless returned items are still held.
    fn collect(&mut self) {
        while let Some(item) = self.held.pop_front() {
            if let Err(PushError::Full(item)) = self.back.push(item) {
                self.held.push_front(item);
                return;
            }
        }
        while let Ok(queued) = self.commands.pop() {
            self.take(queued);
        }
    }

    /// One command, at its effective frame: the latest of its stamp, the next
    /// frame, and the last of its kind (`docs/session-format.md` §4).
    fn take(&mut self, queued: Queued) {
        let Queued {
            id,
            requested,
            command,
        } = queued;
        let next = self.engine.next_frame();
        match command {
            Command::Event(event) => {
                let frame = requested.max(next).max(self.last_event);
                match self.engine.push_external(event) {
                    Ok(()) => {
                        self.last_event = frame;
                        self.report(id, requested, frame, Outcome::Scheduled);
                    }
                    Err(ExternalFull(event)) => {
                        self.report(id, requested, next, Outcome::Refused);
                        self.send_back(Back::Event(event));
                    }
                }
            }
            Command::Suspend(address) => {
                let pushed = self.engine.push_suspend(address, requested);
                self.edit_taken(id, requested, next, pushed);
            }
            Command::Resume(address) => {
                let pushed = self.engine.push_resume(address, requested);
                self.edit_taken(id, requested, next, pushed);
            }
            Command::Swap(swap) => {
                let frame = match self.last_swap {
                    Some(last) => requested.max(next).max(Frame(last.0.saturating_add(1))),
                    None => requested.max(next),
                };
                if self.pending.len() < self.pending.capacity() {
                    self.last_swap = Some(frame);
                    self.pending.push_back(PendingSwap {
                        id,
                        requested,
                        frame,
                        swap,
                    });
                } else {
                    self.report(id, requested, next, Outcome::Refused);
                    self.send_back(Back::Swap(swap));
                }
            }
        }
    }

    /// A suspend or resume handed to the engine: its effective frame is the
    /// latest of its stamp, the next frame and the last edit's.
    fn edit_taken(
        &mut self,
        id: CommandId,
        requested: Frame,
        next: Frame,
        pushed: Result<(), EditFull>,
    ) {
        match pushed {
            Ok(()) => {
                let frame = requested.max(next).max(self.last_edit);
                self.last_edit = frame;
                self.report(id, requested, frame, Outcome::Scheduled);
            }
            Err(EditFull(address)) => {
                self.report(id, requested, next, Outcome::Refused);
                self.send_back(Back::Address(address));
            }
        }
    }

    /// One frame: the swap due now, the frame, its audio, its log.
    fn frame(&mut self, row: &mut [f32]) {
        let next = self.engine.next_frame();
        if self.pending.front().is_some_and(|p| p.frame <= next)
            && let Some(mut pending) = self.pending.pop_front()
        {
            match self.engine.commit_swap(&mut pending.swap, next) {
                Ok(()) => {
                    if let Some(sink) = self.engine.first_audio_sink() {
                        self.sink = sink;
                    }
                    let version = self.engine.version();
                    let outcome = Outcome::Committed { version };
                    self.report(pending.id, pending.requested, next, outcome);
                }
                Err(_) => self.report(pending.id, pending.requested, next, Outcome::Refused),
            }
            self.send_back(Back::Swap(pending.swap));
        }

        self.engine.run_frame();
        row.fill(0.0);
        let gain = self
            .gain
            .as_ref()
            .and_then(|port| self.engine.parameter(port))
            .map(|g| g.effective);
        let inputs = self.engine.instance_inputs(self.sink);
        for (channel, &x) in inputs.iter().enumerate() {
            let sample = to_device(gain.map_or(x, |g| g * x), &mut self.counts.clipped);
            if let Some(level) = self.levels.get_mut(channel) {
                level.0 = level.0.max(sample.abs());
                level.1 += f64::from(sample) * f64::from(sample);
            }
            if let Some(out) = row.get_mut(channel) {
                *out = sample;
            }
        }

        let version = self.engine.version();
        let (log, overflows) = (&mut self.log, &mut self.counts.log_overflows);
        self.engine.drain_log(&mut |record| {
            if log.push((version, *record)).is_err() {
                *overflows = overflows.saturating_add(1);
            }
        });
    }

    /// After the frames: meters, used input back, then the status, last, so
    /// that every report it covers precedes it.
    fn finish(&mut self, first_frame: Frame, frames: u64) {
        if frames > 0 {
            for (channel, &(peak, sum_squares)) in self.levels.iter().enumerate() {
                let meter = Meter {
                    first_frame,
                    frames,
                    channel,
                    peak,
                    sum_squares,
                };
                if self.meters.push(meter).is_err() {
                    self.counts.meter_overflows = self.counts.meter_overflows.saturating_add(1);
                }
            }
        }
        self.push_parameters();
        let Runner {
            engine,
            back,
            held,
            counts,
            ..
        } = self;
        for event in engine.reclaim_externals() {
            send_back(back, held, &mut counts.return_stalls, Back::Event(event));
        }
        for address in engine.reclaim_edits() {
            send_back(
                back,
                held,
                &mut counts.return_stalls,
                Back::Address(address),
            );
        }
        self.counts.callbacks = self.counts.callbacks.saturating_add(1);
        self.counts.engine_log_overflows = self.engine.log_overflows();
        self.counts.input_overflows = self.engine.input_overflows();
        self.counts.input_discards = self.engine.input_discards();
        // A held report would be missing behind this status's frontier.
        if !self.held.is_empty() || self.status.push(self.status()).is_err() {
            self.counts.status_overflows = self.counts.status_overflows.saturating_add(1);
        }
    }

    /// Pushes the next parameters' values, for observation.
    fn push_parameters(&mut self) {
        let count = self.engine.parameter_count();
        let version = self.engine.version();
        for _ in 0..count.min(PARAMETERS_PER_CALLBACK) {
            if self.next_parameter >= count {
                self.next_parameter = 0;
            }
            let index = self.next_parameter;
            self.next_parameter += 1;
            if let Some(value) = self.engine.parameter_at(index)
                && self.parameters.push((version, index, value)).is_err()
            {
                self.counts.parameter_overflows = self.counts.parameter_overflows.saturating_add(1);
                return;
            }
        }
    }

    fn report(&mut self, id: CommandId, requested: Frame, frame: Frame, outcome: Outcome) {
        if outcome == Outcome::Refused {
            self.counts.refused = self.counts.refused.saturating_add(1);
        } else if frame > requested {
            self.counts.late = self.counts.late.saturating_add(1);
        }
        let report = Report {
            id,
            requested,
            frame,
            outcome,
        };
        self.send_back(Back::Report(report));
    }

    fn send_back(&mut self, item: Back) {
        send_back(
            &mut self.back,
            &mut self.held,
            &mut self.counts.return_stalls,
            item,
        );
    }
}

/// Pushes `item` to the return queue, or holds it. Never drops it: that
/// would free on the audio thread.
fn send_back(back: &mut Producer<Back>, held: &mut VecDeque<Back>, stalls: &mut u64, item: Back) {
    let item = if held.is_empty() {
        match back.push(item) {
            Ok(()) => return,
            Err(PushError::Full(item)) => item,
        }
    } else {
        item
    };
    *stalls = stalls.saturating_add(1);
    if held.len() < held.capacity() {
        held.push_back(item);
    } else {
        // Unreachable: at most two items per command in flight exist, and
        // the hold has room for all of them. Leaking neither frees nor blocks.
        mem::forget(item);
    }
}

/// A sample for the device: clamped to [−1, 1], non-finite as 0; either
/// counts as clipped.
fn to_device(x: f64, clipped: &mut u64) -> f32 {
    let clamped = if x.is_finite() {
        x.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    if clamped != x {
        *clipped = clipped.saturating_add(1);
    }
    clamped as f32
}

/// The control-thread half of a real-time run: sends commands to the
/// [`Runner`], prepares swaps, and drains what it sends back
/// ([`poll`](Controller::poll)). Allocates (it builds plans) but performs no
/// file I/O.
///
/// It keeps a structural mirror of each plan version whose log records may
/// still arrive, and retires older mirrors only when a log record of a newer
/// version is drained; if log overflow drops every record of a version, the
/// older mirror stays until a later record arrives (at most one per swap
/// sent).
pub struct Controller {
    commands: Producer<Queued>,
    back: Consumer<Back>,
    log: Consumer<(u64, LogRecord)>,
    meters: Consumer<Meter>,
    parameters: Consumer<(u64, usize, ParameterValue)>,
    status: Consumer<RunnerStatus>,
    registry: Registry,
    /// The engine and the version the next swap is prepared at.
    target: SwapTarget,
    /// Structural mirrors by engine plan version: the newest is the plan the
    /// next swap replaces; older ones read log records not yet drained.
    mirrors: VecDeque<(u64, PlanState)>,
    sink: (InstanceAddress, usize),
    latency: u64,
    fade: u64,
    next_id: CommandId,
    sent: BTreeMap<CommandId, SentCommand>,
    /// Commands whose returned item has not been dropped yet.
    outstanding: usize,
    limit: usize,
    latest: RunnerStatus,
    command_overflows: u64,
    desynchronised: bool,
    /// Tells this Controller's [`ReadySwap`]s from another's.
    id: u64,
}

/// The id of the next [`Controller`] built (bookkeeping, not semantics).
static NEXT_CONTROLLER: AtomicU64 = AtomicU64::new(1);

impl Controller {
    /// Sends an event to `target`, stamped `stamp`; its sequence number is
    /// the returned id.
    pub fn send_event(
        &mut self,
        target: PortAddress,
        payload: EventPayload,
        stamp: Stamp,
    ) -> Result<CommandId, ControlError> {
        let event = ExternalEvent {
            frame: self.frame_of(stamp),
            sequence: self.next_id,
            target,
            payload,
        };
        let sent = SentCommand::Event(event.clone());
        self.send(event.frame, Command::Event(event), sent)
    }

    /// Sends a suspend of `instance` (ADR 0016 §1a), stamped `stamp`.
    pub fn send_suspend(
        &mut self,
        instance: InstanceAddress,
        stamp: Stamp,
    ) -> Result<CommandId, ControlError> {
        let sent = SentCommand::Suspend(instance.clone());
        self.send(self.frame_of(stamp), Command::Suspend(instance), sent)
    }

    /// Sends a resume of `instance`, stamped `stamp`.
    pub fn send_resume(
        &mut self,
        instance: InstanceAddress,
        stamp: Stamp,
    ) -> Result<CommandId, ControlError> {
        let sent = SentCommand::Resume(instance.clone());
        self.send(self.frame_of(stamp), Command::Resume(instance), sent)
    }

    /// Prepares a swap to `plan` here (it allocates) and sends it, stamped
    /// `stamp`. It commits at its effective frame; its report says so. The
    /// plan must keep the audio sink: one, at the same address, as wide.
    /// [`prepare_swap`](Controller::prepare_swap), then
    /// [`send_swap`](Controller::send_swap).
    pub fn swap(&mut self, plan: &ExecutionPlan, stamp: Stamp) -> Result<CommandId, ControlError> {
        let ready = self.prepare_swap(plan)?;
        self.send_swap(ready, stamp)
    }

    /// Prepares a swap to `plan` (it allocates) without sending anything:
    /// every refusal of [`swap`](Controller::swap) but a full queue happens
    /// here, so a caller can prepare before sending commands the swap
    /// completes (a removal's suspends).
    pub fn prepare_swap(&self, plan: &ExecutionPlan) -> Result<ReadySwap, ControlError> {
        if self.desynchronised {
            return Err(ControlError::Desynchronised);
        }
        let mirror = PlanState::mirror(plan, &self.registry, self.target.config)
            .map_err(ControlError::Engine)?;
        if mirror.audio_sink() != Some((&self.sink.0, self.sink.1)) {
            let after = mirror
                .audio_sink()
                .map(|(a, _)| a.clone())
                .into_iter()
                .collect();
            return Err(ControlError::Sink(RenderError::SinkChanged {
                before: self.sink.0.clone(),
                after,
                channels: self.sink.1,
            }));
        }
        let old = self
            .mirrors
            .back()
            .map(|(_, state)| state)
            .ok_or(ControlError::Desynchronised)?;
        let swap = prepare_swap(old, self.target, &[], plan, &self.registry)
            .map_err(ControlError::Engine)?;
        Ok(ReadySwap {
            swap: Box::new(swap),
            mirror,
            plan_version: plan.version,
            target_version: self.target.version,
            controller: self.id,
        })
    }

    /// Sends a swap from [`prepare_swap`](Controller::prepare_swap), stamped
    /// `stamp`. Refused, before anything is queued, if another Controller
    /// prepared it or another swap was sent since it was prepared
    /// ([`EngineError::StaleSwap`]), or as any command is.
    pub fn send_swap(&mut self, ready: ReadySwap, stamp: Stamp) -> Result<CommandId, ControlError> {
        if self.desynchronised {
            return Err(ControlError::Desynchronised);
        }
        if ready.controller != self.id || ready.target_version != self.target.version {
            return Err(ControlError::Engine(EngineError::StaleSwap));
        }
        let sent = SentCommand::Swap {
            plan_version: ready.plan_version,
        };
        let id = self.send(self.frame_of(stamp), Command::Swap(ready.swap), sent)?;
        self.target.version = self.target.version.saturating_add(1);
        self.mirrors.push_back((self.target.version, ready.mirror));
        Ok(id)
    }

    fn send(
        &mut self,
        requested: Frame,
        command: Command,
        sent: SentCommand,
    ) -> Result<CommandId, ControlError> {
        if self.outstanding >= self.limit {
            self.command_overflows = self.command_overflows.saturating_add(1);
            return Err(ControlError::Full(command));
        }
        let id = self.next_id;
        let queued = Queued {
            id,
            requested,
            command,
        };
        if let Err(PushError::Full(queued)) = self.commands.push(queued) {
            self.command_overflows = self.command_overflows.saturating_add(1);
            return Err(ControlError::Full(queued.command));
        }
        self.next_id = id.saturating_add(1);
        self.outstanding += 1;
        self.sent.insert(id, sent);
        Ok(id)
    }

    /// Drains everything the runner sent, without allocating: reads the
    /// newest status, hands each report to `sink` with its command, drops
    /// returned items and replaced plans, hands each log record and parameter
    /// value (read with the plan that wrote it) and meter to `sink`, and calls
    /// [`frontier`](ControlSink::frontier) with the status's next frame.
    pub fn poll(&mut self, sink: &mut impl ControlSink) {
        while let Ok(status) = self.status.pop() {
            self.latest = status;
        }
        let frontier = self.latest.next_frame;
        while let Ok(item) = self.back.pop() {
            match item {
                Back::Report(report) => {
                    if let Some(command) = self.sent.remove(&report.id) {
                        if report.outcome == Outcome::Refused
                            && matches!(command, SentCommand::Swap { .. })
                        {
                            self.desynchronised = true;
                        }
                        sink.report(&report, &command);
                    }
                }
                // Dropped here, on the control thread.
                Back::Event(event) => self.dropped(event),
                Back::Address(address) => self.dropped(address),
                Back::Swap(swap) => self.dropped(swap),
            }
        }
        while let Ok((version, record)) = self.log.pop() {
            // The log is FIFO: no later record names an older plan.
            while self.mirrors.len() > 1 && self.mirrors.front().is_some_and(|m| m.0 < version) {
                self.mirrors.pop_front();
            }
            let mirror = self.mirrors.iter().find(|m| m.0 == version);
            if let Some(line) = mirror.and_then(|(_, state)| state.line(&record)) {
                sink.log(line);
            }
        }
        while let Ok(meter) = self.meters.pop() {
            sink.meter(&meter);
        }
        // Values of a plan whose mirror is retired are dropped: a newer
        // plan's values follow.
        while let Ok((version, index, value)) = self.parameters.pop() {
            let mirror = self.mirrors.iter().find(|m| m.0 == version);
            if let Some(port) = mirror.and_then(|(_, state)| state.parameter_port(index)) {
                sink.parameter(port, value);
            }
        }
        sink.frontier(frontier);
    }

    /// The runner's last status that reached this thread.
    pub fn status(&self) -> RunnerStatus {
        self.latest
    }

    /// The frame [`Stamp::Default`] stands for now: the last reported next
    /// frame plus the latency.
    pub fn default_frame(&self) -> Frame {
        Frame(self.latest.next_frame.0.saturating_add(self.latency))
    }

    /// The latency in frames.
    pub fn latency_frames(&self) -> u64 {
        self.latency
    }

    /// The frame to swap out an instance suspended at `suspend`
    /// ([`Engine::removal_swap_frame`]).
    pub fn removal_swap_frame(&self, suspend: Frame) -> Frame {
        Frame(suspend.0.saturating_add(self.fade))
    }

    /// Commands refused because the queue was full or too many were in
    /// flight.
    pub fn command_overflows(&self) -> u64 {
        self.command_overflows
    }

    /// Commands sent whose returned item has not been dropped yet.
    pub fn in_flight(&self) -> usize {
        self.outstanding
    }

    /// How many more commands can be sent now without being refused as
    /// [`Full`](ControlError::Full): the in-flight limit less those in
    /// flight. A batch that must go out whole checks this first.
    pub fn available(&self) -> usize {
        self.limit.saturating_sub(self.outstanding)
    }

    /// Whether a swap was refused at its commit (no further swap is sent).
    pub fn is_desynchronised(&self) -> bool {
        self.desynchronised
    }

    /// Drops what a command left behind; the command is no longer in flight.
    fn dropped<T>(&mut self, item: T) {
        drop(item);
        self.outstanding = self.outstanding.saturating_sub(1);
    }

    fn frame_of(&self, stamp: Stamp) -> Frame {
        match stamp {
            Stamp::Default => self.default_frame(),
            Stamp::At(frame) => frame,
        }
    }
}

#[cfg(test)]
mod tests {
    //! What the public API cannot reach: a swap refused at its commit, and a
    //! full return queue (both ruled out by construction).

    use signal_core::{
        DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
        PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor, SinkKind,
    };
    use signal_graph::{CompositeDef, ConnectionDef, Exports, InstanceDef, build_plan};

    use super::*;

    const fn port(name: &'static str, direction: Direction) -> PortDescriptor {
        PortDescriptor {
            name,
            direction,
            shape: PortShape::Signal,
            rate: RateDivisor::ONE,
            kind: PortKind::Audio,
            unit: None,
            range: None,
        }
    }

    const HALF_PORTS: &[PortDescriptor] = &[port("out", Direction::Out)];
    const SINK_PORTS: &[PortDescriptor] = &[port("l", Direction::In)];

    /// `out` = 0.5.
    #[derive(Default)]
    struct Half;

    impl DirectPrimitive for Half {
        fn describe() -> ModuleDescriptor {
            ModuleDescriptor {
                type_name: "unit.half",
                doc: "0.5",
                ports: HALF_PORTS,
                parameters: &[],
                feedthrough: Feedthrough::Direct,
                sink: None,
            }
        }
        fn prepare(&mut self, _ctx: &PrepareContext) {}
        fn reset(&mut self) {}
        fn process(&mut self, _inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
            outputs.signal(0).fill(0.5);
        }
    }

    /// An audio sink with one input.
    #[derive(Default)]
    struct Sink;

    impl DirectPrimitive for Sink {
        fn describe() -> ModuleDescriptor {
            ModuleDescriptor {
                type_name: "unit.sink",
                doc: "sink",
                ports: SINK_PORTS,
                parameters: &[],
                feedthrough: Feedthrough::Direct,
                sink: Some(SinkKind::Audio),
            }
        }
        fn prepare(&mut self, _ctx: &PrepareContext) {}
        fn reset(&mut self) {}
        fn process(&mut self, _inputs: &Inputs<'_>, _outputs: &mut Outputs<'_>) {}
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry
            .register(ModuleType {
                descriptor: Half::describe(),
                create: || Primitive::Direct(Box::new(Half)),
            })
            .unwrap();
        registry
            .register(ModuleType {
                descriptor: Sink::describe(),
                create: || Primitive::Direct(Box::new(Sink)),
            })
            .unwrap();
        registry
    }

    fn plan(version: u64) -> ExecutionPlan {
        let instance = |name: &str, module: &str| InstanceDef {
            name: name.to_owned(),
            module: module.to_owned(),
            parameters: Default::default(),
        };
        let root = CompositeDef {
            name: "project".to_owned(),
            instances: vec![instance("half", "unit.half"), instance("out", "unit.sink")],
            connections: vec![ConnectionDef {
                from: "half.out".to_owned(),
                to: "out.l".to_owned(),
                scale: None,
            }],
            exports: Exports::default(),
        };
        build_plan(&root, &[], &registry(), version).unwrap()
    }

    #[derive(Default)]
    struct Reports(Vec<Report>);

    impl ControlSink for Reports {
        fn report(&mut self, report: &Report, _command: &SentCommand) {
            self.0.push(*report);
        }
    }

    /// A swap the engine refuses at its commit is reported refused, its box
    /// comes back, and the Controller sends no further swap.
    #[test]
    fn a_refused_commit_desynchronises_the_controller() {
        let (mut runner, mut controller) =
            realtime(&plan(0), &registry(), RealtimeConfig::new(1_000, 1)).unwrap();
        // Prepared at a version the engine is not at: refused as stale.
        controller.target.version += 1;
        controller.swap(&plan(1), Stamp::At(Frame(2))).unwrap();
        let mut out = [0.0f32; 8];
        runner.process(&mut out, 1);
        assert_eq!(out, [0.5; 8], "the old plan keeps running");
        let mut reports = Reports::default();
        controller.poll(&mut reports);
        assert_eq!(reports.0.len(), 1);
        assert_eq!(reports.0[0].outcome, Outcome::Refused);
        assert!(controller.is_desynchronised());
        assert_eq!(controller.status().refused, 1);
        assert_eq!(controller.in_flight(), 0, "the refused swap came back");
        assert!(matches!(
            controller.swap(&plan(2), Stamp::Default),
            Err(ControlError::Desynchronised)
        ));
    }

    /// Out-of-range samples are clamped, non-finite ones written as 0; all
    /// are counted.
    #[test]
    fn non_finite_samples_are_written_as_zero_and_counted() {
        let mut clipped = 0;
        assert_eq!(to_device(0.5, &mut clipped), 0.5);
        assert_eq!(to_device(1.5, &mut clipped), 1.0);
        assert_eq!(to_device(-1.5, &mut clipped), -1.0);
        assert_eq!(to_device(f64::INFINITY, &mut clipped), 0.0);
        assert_eq!(to_device(f64::NEG_INFINITY, &mut clipped), 0.0);
        assert_eq!(to_device(f64::NAN, &mut clipped), 0.0);
        assert_eq!(clipped, 5);
    }

    /// With the in-flight limit lifted, a full return queue holds items on
    /// the audio thread (counted, never dropped) and withholds the status
    /// until they are sent; nothing is lost.
    #[test]
    fn a_full_return_queue_holds_items_and_loses_nothing() {
        let config = RealtimeConfig {
            commands: 1,
            ..RealtimeConfig::new(1_000, 1)
        };
        let (mut runner, mut controller) = realtime(&plan(0), &registry(), config).unwrap();
        controller.limit = 10;
        let target: PortAddress = "/half.out".parse().unwrap();
        let mut out = [0.0f32; 4];
        let mut sent = Vec::new();
        for _ in 0..3 {
            // Unrouted events: the engine takes them and counts them as
            // discards, which is all this test needs.
            let id = controller
                .send_event(target.clone(), EventPayload::Trigger, Stamp::At(Frame(0)))
                .unwrap();
            sent.push(id);
            runner.process(&mut out, 1);
        }
        assert!(runner.status().return_stalls > 0);
        assert!(!runner.held.is_empty());
        let mut reports = Reports::default();
        controller.poll(&mut reports);
        runner.process(&mut out, 1);
        controller.poll(&mut reports);
        runner.process(&mut out, 1);
        controller.poll(&mut reports);
        assert!(runner.held.is_empty());
        let ids: Vec<CommandId> = reports.0.iter().map(|r| r.id).collect();
        assert_eq!(ids, sent);
        assert_eq!(controller.in_flight(), 0);
        assert!(runner.status().status_overflows > 0);
    }
}
