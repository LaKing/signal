//! The cpal glue (ADR 0011): device listing, the output stream and the input
//! stream (metering only), and the real-time workers' lifecycle around the
//! output stream (ADR 0019 §4). The only code that calls cpal; everything it
//! decides is in [`choose_output`], [`plan_join`] (ADR 0036: what of a device
//! may be changed), [`OutputCallback`], [`InputCallback`], [`CallbackStats`]
//! and [`Scheduling`].

use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, Stream, StreamConfig, SupportedBufferSize};
use signal_engine::{Computation, Workers};
use signal_output::fade::fade_frames;
use signal_output::{Driver, DriverError, Runner};

use serde::Serialize;
use signal_output::adapter::{AdapterConfig, clock_adapter};

use signal_core::InstanceAddress;
use signal_output::inputs::{InputCounts, InputRecord, InputSource, RunInput};

use crate::callback::{CallbackControl, FeedControl, OutputCallback};
use crate::capture::{AdaptedSource, CaptureFeed, SideFeed, SideSink, capture_ring};
use crate::config::{
    AudioError, Choice, ConfigRange, DeviceInfo, Direction, choose_input, choose_output,
    fault_message, rate_moved, start_failure,
};
use crate::device_props;
use crate::input::{InputCallback, InputMeters};
use crate::join::{BufferRequest, DeviceState, FALLBACK_PERIOD, JoinPlan, plan_join};
use crate::map::ChannelMap;
use crate::rt_thread::{self, Workgroup};
use crate::scheduling::{DeviceMatch, Scheduling, TimeConstraint, WorkerPath, match_device};
use crate::secondary::{SecondaryCallback, SecondaryFeed, SecondaryState, SecondaryStats};
use crate::stats::{CallbackStats, StreamFault};

/// How long [`CoreAudioDriver::stop`] waits for the callback to hand the
/// Runner back.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the driver waits for the workers to enter or leave real-time
/// scheduling. A worker that takes longer still runs the request later.
const WORKER_TIMEOUT: Duration = Duration::from_secs(1);

/// How often the device list is read again when it changed while matching.
const MATCH_ATTEMPTS: usize = 3;

/// How long [`CoreAudioDriver::stop`] waits at least for a stream the audio
/// system stopped ([`StreamFault`]), which calls no more back (or two of
/// its longest periods, if longer: a callback under way hands the Runner
/// back).
const FAULT_STOP_TIMEOUT: Duration = Duration::from_millis(50);

fn host_error(error: impl std::fmt::Display) -> AudioError {
    AudioError::Host(error.to_string())
}

fn name_of(device: &Device) -> Result<String, AudioError> {
    Ok(device.description().map_err(host_error)?.name().to_owned())
}

fn ranges(configs: impl Iterator<Item = cpal::SupportedStreamConfigRange>) -> Vec<ConfigRange> {
    configs
        .map(|c| ConfigRange {
            channels: c.channels(),
            min_rate: c.min_sample_rate(),
            max_rate: c.max_sample_rate(),
            buffer: match *c.buffer_size() {
                SupportedBufferSize::Range { min, max } => Some((min, max)),
                SupportedBufferSize::Unknown => None,
            },
            sample_format: c.sample_format().to_string(),
        })
        .collect()
}

fn id_of(device: &Device) -> Option<String> {
    device.id().ok().map(|id| id.to_string())
}

/// The rate `device` runs at now, as cpal reads it without our `unsafe`: the
/// rate of its current stream format (`kAudioDevicePropertyStreamFormat`),
/// which follows the nominal rate Audio MIDI Setup shows. `None` when it
/// cannot be read, among them a device whose format is neither f32 nor i16
/// (cpal refuses those): a recovery takes such a device as ready.
fn current_rate(device: &Device, direction: Direction) -> Option<u32> {
    let config = match direction {
        Direction::Output => device.default_output_config(),
        Direction::Input => device.default_input_config(),
    };
    config.ok().map(|c| c.sample_rate())
}

/// `device` as plain data. `default` is the default device's id, or its name
/// when it has none.
fn info(
    device: &Device,
    direction: Direction,
    default: Option<&str>,
) -> Result<DeviceInfo, AudioError> {
    let name = name_of(device)?;
    let configs = match direction {
        Direction::Output => ranges(device.supported_output_configs().map_err(host_error)?),
        Direction::Input => ranges(device.supported_input_configs().map_err(host_error)?),
    };
    let id = id_of(device);
    let key = id.as_deref().unwrap_or(name.as_str());
    Ok(DeviceInfo {
        default: default == Some(key),
        rate: current_rate(device, direction),
        name,
        id,
        direction,
        configs,
    })
}

/// The output and input devices, with their supported configurations.
pub fn devices() -> Result<Vec<DeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let key = |d: Device| id_of(&d).or_else(|| name_of(&d).ok());
    let default_out = host.default_output_device().and_then(key);
    let default_in = host.default_input_device().and_then(key);
    let mut list = Vec::new();
    for device in host.output_devices().map_err(host_error)? {
        list.push(info(&device, Direction::Output, default_out.as_deref())?);
    }
    for device in host.input_devices().map_err(host_error)? {
        list.push(info(&device, Direction::Input, default_in.as_deref())?);
    }
    Ok(list)
}

/// Which device to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceSelector {
    /// The system's default device of the direction.
    Default,
    /// The first device of this name (`signal play --device`).
    Name(String),
    /// The device of this id ([`DeviceInfo::id`]).
    Id(String),
}

fn find(direction: Direction, selector: &DeviceSelector) -> Result<Device, AudioError> {
    let host = cpal::default_host();
    let all = || match direction {
        Direction::Output => host.output_devices().map_err(host_error),
        Direction::Input => host.input_devices().map_err(host_error),
    };
    let device = match selector {
        DeviceSelector::Default => match direction {
            Direction::Output => host.default_output_device(),
            Direction::Input => host.default_input_device(),
        },
        DeviceSelector::Name(wanted) => all()?.find(|d| name_of(d).is_ok_and(|n| n == *wanted)),
        DeviceSelector::Id(wanted) => all()?.find(|d| id_of(d).as_deref() == Some(wanted)),
    };
    let Some(device) = device else {
        return Err(AudioError::NoDevice {
            direction,
            name: match selector {
                DeviceSelector::Default => None,
                DeviceSelector::Name(s) | DeviceSelector::Id(s) => Some(s.clone()),
            },
            available: all()?.filter_map(|d| name_of(&d).ok()).collect(),
        });
    };
    Ok(device)
}

/// The output device `selector` names: its name and the rate it runs at now
/// (`None` when that cannot be read). Read again on each call, so a device
/// switched in Audio MIDI Setup, or plugged in, is seen.
pub fn output_rate(selector: &DeviceSelector) -> Result<(String, Option<u32>), AudioError> {
    let device = find(Direction::Output, selector)?;
    Ok((name_of(&device)?, current_rate(&device, Direction::Output)))
}

/// The CoreAudio object of `device`: found by its position in the system's
/// device list, which cpal lists in the same order (`match_device`), or why
/// there is none.
fn object_of(device: &Device) -> Result<u32, String> {
    let id = id_of(device).ok_or_else(|| "the device has no id".to_owned())?;
    let host = cpal::default_host();
    for _ in 0..MATCH_ATTEMPTS {
        let before = rt_thread::device_ids()?;
        let ids: Vec<Option<String>> = host
            .devices()
            .map_err(|e| e.to_string())?
            .map(|d| id_of(&d))
            .collect();
        let after = rt_thread::device_ids()?;
        match match_device(&before, &ids, &after, &id) {
            Ok(object) => return Ok(object),
            Err(DeviceMatch::Changed) => continue,
            Err(absent) => return Err(absent.to_string()),
        }
    }
    Err(DeviceMatch::Changed.to_string())
}

/// The I/O workgroup of `device`, or why there is none.
fn workgroup_of(device: &Device) -> Result<Workgroup, String> {
    rt_thread::device_workgroup(object_of(device)?)
}

/// How long [`state_of`] waits for a device that runs for this process to
/// stop (Signal's own stream, just stopped), so that it is not taken for
/// another app's.
const OWN_STOP_WAIT: Duration = Duration::from_millis(100);

/// `device`'s shared state (ADR 0036), read now; all unknown when its
/// CoreAudio object is not found. A device that runs for this process is
/// read again for up to [`OWN_STOP_WAIT`] until it no longer does (measured
/// on BlackHole 64ch, 2026-10-23: it reads stopped at once after
/// [`CoreAudioDriver::stop`]); one that still does then (Signal's input on
/// it) is taken as in use.
fn state_of(device: &Device) -> DeviceState {
    let Ok(object) = object_of(device) else {
        return DeviceState::default();
    };
    let deadline = Instant::now() + OWN_STOP_WAIT;
    loop {
        let state = device_props::state(object);
        if state.running_here != Some(true) || Instant::now() >= deadline {
            return state;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// The shared state of the device `selector` names, read now (ADR 0036):
/// whether it runs for any process, its buffer and its nominal rate.
pub fn device_state(
    direction: Direction,
    selector: &DeviceSelector,
) -> Result<DeviceState, AudioError> {
    Ok(state_of(&find(direction, selector)?))
}

/// The physical format of the device `selector` names, as cpal reads it on
/// the device object (ADR 0036's measurements); `None` when it does not
/// read.
pub fn physical_format(
    direction: Direction,
    selector: &DeviceSelector,
) -> Result<Option<device_props::PhysicalFormat>, AudioError> {
    let device = find(direction, selector)?;
    Ok(object_of(&device)
        .ok()
        .and_then(device_props::physical_format))
}

/// The frames per callback the output device `selector` names would run at
/// with the buffer `request`, decided now ([`plan_join`]): what a run sizes
/// its engine for before the driver is opened.
pub fn output_period(selector: &DeviceSelector, request: BufferRequest) -> Result<u32, AudioError> {
    let device = find(Direction::Output, selector)?;
    Ok(plan_join(request, &state_of(&device)).period)
}

/// The rate a live run on the output device `selector` names plays at (the
/// owner's decision of 2026-10-23, ADR 0036): the device's clock is the
/// run's, so its current rate, never switched; `project_rate` (the
/// project's `sample_rate`, the rate of its renders) only when the device's
/// cannot be read. With the device's name.
pub fn live_rate(
    selector: &DeviceSelector,
    project_rate: u32,
) -> Result<(String, u32), AudioError> {
    let device = find(Direction::Output, selector)?;
    let rate = state_of(&device)
        .rate
        .or_else(|| current_rate(&device, Direction::Output))
        .unwrap_or(project_rate);
    Ok((name_of(&device)?, rate))
}

/// Takes the workers out of real-time scheduling ([`rt_thread::exit`]),
/// waiting up to [`WORKER_TIMEOUT`]; a late worker still does it later, or
/// when its thread ends.
fn release_workers(workers: &Workers) {
    let generation = workers.post(|_| rt_thread::exit());
    let _ = workers.wait(generation, WORKER_TIMEOUT);
}

/// Why a start was refused: the device changed between the open, which
/// decided `open` (and sized the engine for its period), and the start,
/// which reads `now` (ADR 0036).
fn join_moved(device: &str, open: JoinPlan, now: JoinPlan) -> String {
    let what = if now.shared && !open.shared {
        "another app started playing on it, and Signal sets no buffer on a device in use".to_owned()
    } else {
        format!(
            "its buffer is now {} frames, not the {} the run was prepared for",
            now.period, open.period
        )
    };
    format!("the device '{device}' changed since the run was prepared: {what}; play again")
}

/// How long a secondary device whose stream failed waits before it is
/// opened again (ADR 0052 §6: on return its routes are rebuilt); each
/// failed open doubles it, up to [`SECONDARY_RETRY_MAX`], so a device that
/// keeps failing costs the control thread an open now and then only.
const SECONDARY_RETRY: Duration = Duration::from_secs(1);

/// The longest wait between two opens of a failing secondary device.
const SECONDARY_RETRY_MAX: Duration = Duration::from_secs(30);

/// The most [`CoreAudioDriver::stop`] lets the secondary devices play out
/// what their adapters hold (the clock's fade-out included) before their
/// streams stop: each waits its adapter's latency, its periods and its
/// stop's fade.
const SECONDARY_DRAIN_MAX: Duration = Duration::from_millis(250);

/// Nanoseconds since `epoch`, read when a callback runs: the time base the
/// clock adapter's marks and pulls share (ADR 0052 §2). Not the callback's
/// CoreAudio time stamp: an output stream's is when its buffer will be
/// played, ahead of now by that device's buffer and safety offset, so two
/// devices' stamps are offset by the difference, a bias in the linearised
/// fill and a false stop where it exceeds the stop detection's reach (the
/// owner's "scraps of sound", 2026-11-06: the clock at 128 frames, the Mac
/// Studio Speakers at their own larger buffer). `Instant::now` reads the
/// commpage clock: no allocation, lock or system call.
fn callback_time(epoch: Instant) -> Option<u64> {
    u64::try_from(epoch.elapsed().as_nanos()).ok()
}

/// What a started stream holds.
struct Running {
    stream: Stream,
    control: CallbackControl,
    workers: Workers,
    /// The clock callback's feed slots, one per secondary device.
    feeds: FeedControl,
    /// The input devices' streams (ADR 0038 §8), stopped after the output.
    inputs: Vec<Stream>,
}

/// One capture side of a run (ADR 0038 §7): its source and the device's
/// input channels it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSideSpec {
    /// The source instance.
    pub address: InstanceAddress,
    /// Its role.
    pub role: String,
    /// Its channels.
    pub width: usize,
    /// Per port, the device's input channel (from 0); `None`: 0.
    pub channels: Vec<Option<usize>>,
}

/// An input device a run captures from, and its capture sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSpec {
    /// The device.
    pub selector: DeviceSelector,
    /// Its name, as the Setup names it.
    pub name: String,
    /// Its capture sides.
    pub sides: Vec<CaptureSideSpec>,
}

/// An input device's status (ADR 0038 §10, ADR 0052 §6).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CaptureStatus {
    /// The device's name.
    pub name: String,
    /// The roles captured from it.
    pub roles: Vec<String>,
    /// `engine` (the clock device's input, a ring) or `adapted`.
    pub clock: &'static str,
    /// `locking`, `locked`, `suspect` or `absent` (adapted); `engine` reads
    /// `locked` while it runs.
    pub state: SecondaryState,
    /// Its nominal rate.
    pub rate: Option<u32>,
    /// How far its clock runs from the clock device's, ppm (adapted).
    pub drift_ppm: f64,
    /// Frames its sources read as 0 (the ring ran short, or it is gone).
    pub missing: u64,
    /// Frames its input callback could not hand on (a full ring or
    /// adapter): lost before the engine.
    pub dropped: u64,
    /// The largest |sample| its sources read since the start (a level).
    pub peak: f32,
    /// Why it is not captured, when it is not.
    pub reason: Option<String>,
}

/// An input device of a running driver.
struct Capture {
    spec: CaptureSpec,
    rate: Option<u32>,
    adapted: bool,
    stats: Arc<SecondaryStats>,
    counts: Vec<Arc<InputCounts>>,
    /// Frames the input callback could not hand on (a full ring).
    dropped: Arc<std::sync::atomic::AtomicU64>,
    reason: Option<String>,
}

/// What an opened input device gives the run.
struct OpenedCapture {
    stream: Stream,
    inputs: Vec<RunInput>,
    records: Vec<InputRecord>,
    capture: Capture,
}

/// A secondary output device of a routed run (ADR 0052 §1): a device other
/// than the clock device that roles play on through a clock adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecondarySpec {
    /// The device.
    pub selector: DeviceSelector,
    /// Its name, as the Setup names it.
    pub name: String,
    /// The roles it plays.
    pub roles: Vec<String>,
    /// The channels of the run's audio frame it carries.
    pub sources: Vec<usize>,
    /// Per source, its device channel (from 0).
    pub targets: Vec<usize>,
}

/// A secondary device's status (ADR 0052 §6), for `signal play`'s status
/// line and `devices`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SecondaryStatus {
    /// The device's name.
    pub name: String,
    /// The roles it plays.
    pub roles: Vec<String>,
    /// Always `adapted`: its clock is not the engine's.
    pub clock: &'static str,
    /// `starting`, `locking`, `locked`, `suspect` or `absent`.
    pub state: SecondaryState,
    /// Its nominal rate while its stream runs.
    pub rate: Option<u32>,
    /// The clock device's rate, when the two differ (the converter's
    /// nominal ratio is `clock_rate / rate`).
    pub clock_rate: Option<u32>,
    /// How far its clock runs from the clock device's, ppm.
    pub drift_ppm: f64,
    /// From a frame's time on the clock device to its playing on this one,
    /// ms: the adapter's latency plus the device's own.
    pub latency_ms: Option<f64>,
    /// Repeats and skips (ADR 0052 §3).
    pub slips: u64,
    /// Splices to F* after its clock jumped or ran beyond the loop (ADR
    /// 0052 §6), in its current stream.
    pub resyncs: u64,
    /// Times its routes were rebuilt after the device came back.
    pub rebuilds: u64,
    /// Why it is not playing, when it is not.
    pub reason: Option<String>,
}

impl Capture {
    fn status(&self) -> CaptureStatus {
        let now = self.stats.snapshot();
        let gone = self.stats.is_gone();
        let state = if self.reason.is_some() || gone {
            SecondaryState::Absent
        } else if self.adapted {
            now.state
        } else {
            SecondaryState::Locked
        };
        CaptureStatus {
            name: self.spec.name.clone(),
            roles: self.spec.sides.iter().map(|s| s.role.clone()).collect(),
            clock: if self.adapted { "adapted" } else { "engine" },
            state,
            rate: self.rate,
            drift_ppm: if self.adapted { now.drift_ppm } else { 0.0 },
            missing: self
                .counts
                .iter()
                .map(|c| c.missing.load(std::sync::atomic::Ordering::Relaxed))
                .sum(),
            peak: self.counts.iter().map(|c| c.peak()).fold(0.0, f32::max),
            dropped: self.dropped.load(std::sync::atomic::Ordering::Relaxed),
            reason: self
                .reason
                .clone()
                .or_else(|| gone.then(|| "the device went away or its stream failed".to_owned())),
        }
    }
}

/// One secondary device of a driver, and its stream while it plays.
struct Secondary {
    spec: SecondarySpec,
    stats: Arc<SecondaryStats>,
    stream: Option<Stream>,
    /// The open stream's rate and the device's own latency in frames.
    open: Option<(u32, Option<u32>)>,
    reason: Option<String>,
    retry_at: Option<Instant>,
    /// The wait before the next open after this one fails.
    backoff: Duration,
    /// The system's device list when it last failed: a change opens it
    /// again before its wait is over.
    listed: Option<Vec<u32>>,
    /// The open stream's period, frames.
    period: u32,
    /// Streams opened (the first included).
    opened: u64,
}

impl Secondary {
    fn new(spec: SecondarySpec) -> Secondary {
        Secondary {
            spec,
            stats: Arc::new(SecondaryStats::default()),
            stream: None,
            open: None,
            reason: None,
            retry_at: None,
            backoff: SECONDARY_RETRY,
            listed: None,
            period: 0,
            opened: 0,
        }
    }

    /// Opens and plays the device's stream at the rate it runs at, with its
    /// own buffer (Signal changes neither), its adapter's input going into
    /// the clock callback's `slot`. `clock` is the clock device's rate,
    /// period and largest callback. Control thread.
    /// Returns a notice for channels the device lacks.
    fn start(
        &mut self,
        clock: (u32, u32, u32),
        clock_id: Option<&str>,
        epoch: Instant,
        feeds: &mut FeedControl,
        slot: usize,
    ) -> Result<Option<String>, String> {
        let (clock_rate, clock_period, clock_max) = clock;
        let name = self.spec.name.clone();
        let device = find(Direction::Output, &self.spec.selector).map_err(|e| e.to_string())?;
        // A second stream on the clock device would sum roles on its
        // channels (never summed, ADR 0038 §4): the routing put it on the
        // clock, or the system default moved onto it since.
        if clock_id.is_some() && id_of(&device).as_deref() == clock_id {
            return Err(format!("{name}: it is the clock device now"));
        }
        let info = info(&device, Direction::Output, None).map_err(|e| e.to_string())?;
        let state = state_of(&device);
        let rate = state
            .rate
            .or_else(|| current_rate(&device, Direction::Output))
            .ok_or_else(|| format!("{name}: its rate cannot be read"))?;
        let period = state.buffer.unwrap_or(FALLBACK_PERIOD);
        let width = self.spec.targets.iter().map(|t| t + 1).max().unwrap_or(1);
        // The device's buffer is its own: never set, so not checked
        // against the range (as the clock device's when it is in use).
        let choice = match choose_output(&info, rate, period, width) {
            Err(AudioError::BufferSize { min, max, .. }) => {
                let mut choice = choose_output(&info, rate, period.clamp(min, max), width)
                    .map_err(|e| e.to_string())?;
                choice.buffer = period;
                choice
            }
            other => other.map_err(|e| e.to_string())?,
        };
        let channels = usize::from(choice.channels);
        let missing: Vec<String> = self
            .spec
            .targets
            .iter()
            .filter(|t| **t >= channels)
            .map(|t| (t + 1).to_string())
            .collect();
        let (sources, targets): (Vec<usize>, Vec<usize>) = self
            .spec
            .sources
            .iter()
            .zip(&self.spec.targets)
            .filter(|(_, t)| **t < channels)
            .map(|(s, t)| (*s, *t))
            .unzip();
        if sources.is_empty() {
            return Err(format!(
                "{name}: none of its channels exists (the device has {channels})"
            ));
        }
        let config = AdapterConfig::new(
            sources.len(),
            clock_rate,
            rate,
            clock_period as usize,
            period as usize,
        );
        let (input, output) = clock_adapter(config).map_err(|e| format!("{name}: {e}"))?;
        let stats = Arc::new(SecondaryStats::default());
        let max_frames = (choice.max_frames as usize).max(period as usize);
        let mut callback = SecondaryCallback::new(output, targets, max_frames, Arc::clone(&stats));
        let errors = Arc::clone(&stats);
        let stream_config = StreamConfig {
            channels: choice.channels,
            sample_rate: rate,
            buffer_size: BufferSize::Default,
        };
        let stream = device
            .build_output_stream(
                stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    callback.fill(data, channels, callback_time(epoch));
                },
                move |error: cpal::Error| {
                    errors.stream_error();
                    if matches!(
                        error.kind(),
                        cpal::ErrorKind::StreamInvalidated | cpal::ErrorKind::DeviceNotAvailable
                    ) {
                        errors.device_gone();
                    }
                },
                None,
            )
            .map_err(|e| format!("{name}: {e}"))?;
        // cpal switches the rate while it builds when it differs: a rate
        // that moved since the read would have been switched. Refuse.
        if let Some(after) = state_of(&device).rate
            && after != rate
        {
            drop(stream);
            return Err(rate_moved(&name, rate, after));
        }
        if !feeds.set(
            slot,
            Some(SecondaryFeed::new(input, sources, clock_max as usize)),
        ) {
            return Err(format!("{name}: the clock callback took no feed"));
        }
        if let Err(error) = stream.play() {
            feeds.set(slot, None);
            return Err(format!("{name}: {error}"));
        }
        let latency = object_of(&device)
            .ok()
            .and_then(device_props::output_latency);
        self.stats = stats;
        self.stream = Some(stream);
        self.open = Some((rate, latency));
        self.reason = None;
        self.retry_at = None;
        self.backoff = SECONDARY_RETRY;
        self.listed = None;
        self.period = period;
        self.opened += 1;
        Ok((!missing.is_empty()).then(|| {
            format!(
                "{name}: device channel{} {} do{} not exist (the device has {channels}); not heard there",
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", "),
                if missing.len() == 1 { "es" } else { "" }
            )
        }))
    }

    /// How long its stream should play on after the clock stopped: its
    /// adapter's latency, its periods (the stop detection) and the fade.
    fn drain(&self, clock_rate: u32) -> Duration {
        let Some((rate, _)) = self.open else {
            return Duration::ZERO;
        };
        let latency = self.stats.snapshot().latency_frames / f64::from(clock_rate.max(1));
        let periods = 3.0 * f64::from(self.period) / f64::from(rate.max(1));
        Duration::from_secs_f64((latency + periods + 0.010).max(0.0)).min(SECONDARY_DRAIN_MAX)
    }

    /// Stops and drops its stream (control thread).
    fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
        self.open = None;
    }

    fn status(&self, clock_rate: u32) -> SecondaryStatus {
        let now = self.stats.snapshot();
        let playing = self.stream.is_some() && !self.stats.is_gone();
        let rate = self.open.map(|(rate, _)| rate);
        // Before its first callback the adapter has published nothing.
        let latency_ms = self
            .open
            .filter(|_| playing && now.callbacks > 0)
            .map(|(rate, own)| {
                let adapter = now.latency_frames / f64::from(clock_rate.max(1));
                let device = f64::from(own.unwrap_or(0)) / f64::from(rate.max(1));
                (adapter + device) * 1000.0
            });
        SecondaryStatus {
            name: self.spec.name.clone(),
            roles: self.spec.roles.clone(),
            clock: "adapted",
            state: if playing {
                now.state
            } else if self.reason.is_some() || self.stats.is_gone() {
                SecondaryState::Absent
            } else {
                SecondaryState::Starting
            },
            rate,
            clock_rate: rate.filter(|r| *r != clock_rate).map(|_| clock_rate),
            drift_ppm: if playing { now.drift_ppm } else { 0.0 },
            latency_ms,
            slips: now.slips,
            resyncs: now.resyncs,
            rebuilds: self.opened.saturating_sub(1),
            reason: self.reason.clone(),
        }
    }
}

/// How a driver maps the Runner's audio frame to the device's channels.
enum Mapping {
    /// One sink of this many channels, on device channels 1 to its width.
    Identity(usize),
    /// One sink of this many channels, on these device channels (from 0).
    Targets(usize, Vec<usize>),
    /// Several play sides (ADR 0038): each channel's device channel, if any.
    Routes(Vec<Option<usize>>),
}

/// A [`Driver`] on a CoreAudio output device, through cpal.
/// [`start`](Driver::start) puts the Runner's worker threads into real-time
/// scheduling, then builds and plays the output stream, whose callback is an
/// [`OutputCallback`]; [`stop`](Driver::stop) asks it for the Runner back,
/// takes the workers out of real-time scheduling, then stops and drops the
/// stream on the calling thread.
pub struct CoreAudioDriver {
    device: Device,
    name: String,
    choice: Choice,
    /// The buffer the Setup asks for (ADR 0036).
    request: BufferRequest,
    /// How the open decided to join the device (ADR 0036); each start
    /// keeps to it or refuses.
    plan: JoinPlan,
    map: ChannelMap,
    stats: Arc<CallbackStats>,
    running: Option<Running>,
    scheduling: Option<Scheduling>,
    /// The secondary devices (ADR 0052), in their slots' order.
    secondaries: Vec<Secondary>,
    /// The time base of every callback's time ([`callback_time`]).
    epoch: Instant,
    /// The input devices (ADR 0038 §8).
    captures: Vec<CaptureSpec>,
    /// Frames of record ring per capture side, when the run records its
    /// inputs (ADR 0038 §9).
    record_frames: Option<usize>,
    /// The last start's input devices.
    captured: Vec<Capture>,
    /// The last start's records, until taken.
    records: Vec<InputRecord>,
}

impl CoreAudioDriver {
    /// Opens the output device named `name` (the default output device
    /// without one) for a sink of `sink_channels` at `sample_rate`, with
    /// `buffer` frames requested when Signal is the device's only client (a
    /// device in use keeps its own buffer, ADR 0036). `sample_rate` must be
    /// the rate the device runs at ([`live_rate`]): Signal does not resample,
    /// and [`start`](Driver::start) switches no device's rate; it refuses a
    /// device found at another.
    pub fn open(
        name: Option<&str>,
        sample_rate: u32,
        buffer: u32,
        sink_channels: usize,
    ) -> Result<CoreAudioDriver, AudioError> {
        let selector = name.map_or(DeviceSelector::Default, |n| {
            DeviceSelector::Name(n.to_owned())
        });
        CoreAudioDriver::open_selected(&selector, sample_rate, buffer, sink_channels, None)
    }

    /// [`open`](CoreAudioDriver::open) for the device `selector` names, with
    /// sink channel *i* on device channel `targets[i]` (from 0) when given
    /// ([`ChannelMap::with_targets`]), else the identity map. The device's
    /// configuration is chosen wide enough for the targets
    /// ([`ChannelMap::width_for`]).
    pub fn open_selected(
        selector: &DeviceSelector,
        sample_rate: u32,
        buffer: u32,
        sink_channels: usize,
        targets: Option<Vec<usize>>,
    ) -> Result<CoreAudioDriver, AudioError> {
        CoreAudioDriver::open_with(
            selector,
            sample_rate,
            BufferRequest::Frames(buffer),
            sink_channels,
            targets,
        )
    }

    /// [`open_selected`](CoreAudioDriver::open_selected) with the buffer
    /// `request` (ADR 0036): the frames are requested only when Signal is
    /// the device's only client; a device that runs for another app, or
    /// [`BufferRequest::Device`], keeps its own buffer. Which applies is
    /// decided here, once ([`period`](CoreAudioDriver::period),
    /// [`joined`](CoreAudioDriver::joined)); [`start`](Driver::start) keeps
    /// to it, or refuses when the device changed so that it no longer holds.
    pub fn open_with(
        selector: &DeviceSelector,
        sample_rate: u32,
        request: BufferRequest,
        sink_channels: usize,
        targets: Option<Vec<usize>>,
    ) -> Result<CoreAudioDriver, AudioError> {
        let mapping = match targets {
            Some(targets) => Mapping::Targets(sink_channels, targets),
            None => Mapping::Identity(sink_channels),
        };
        CoreAudioDriver::open_mapped(selector, sample_rate, request, mapping)
    }

    /// [`open_with`](CoreAudioDriver::open_with) for a run of several play
    /// sides (ADR 0038): channel *i* of the run's audio frame on device
    /// channel `routes[i]` (from 0), or not played
    /// ([`ChannelMap::with_routes`]); the device's configuration is chosen
    /// wide enough for the highest route.
    pub fn open_routed(
        selector: &DeviceSelector,
        sample_rate: u32,
        request: BufferRequest,
        routes: Vec<Option<usize>>,
    ) -> Result<CoreAudioDriver, AudioError> {
        CoreAudioDriver::open_mapped(selector, sample_rate, request, Mapping::Routes(routes))
    }

    fn open_mapped(
        selector: &DeviceSelector,
        sample_rate: u32,
        request: BufferRequest,
        mapping: Mapping,
    ) -> Result<CoreAudioDriver, AudioError> {
        let device = find(Direction::Output, selector)?;
        let info = info(&device, Direction::Output, None)?;
        let width = match &mapping {
            Mapping::Identity(sink) => *sink,
            Mapping::Targets(sink, targets) => ChannelMap::width_for(*sink, targets),
            Mapping::Routes(routes) => ChannelMap::width_for_routes(1, routes),
        };
        let plan = plan_join(request, &state_of(&device));
        // A buffer that is not set is not checked against the range: it is
        // the device's own (as for the input).
        let choice = match choose_output(&info, sample_rate, plan.period, width) {
            Err(AudioError::BufferSize { min, max, .. }) if plan.set_buffer.is_none() => {
                let mut choice =
                    choose_output(&info, sample_rate, plan.period.clamp(min, max), width)?;
                choice.buffer = plan.period;
                choice
            }
            other => other?,
        };
        let channels = usize::from(choice.channels);
        let map = match mapping {
            Mapping::Targets(sink, targets) => ChannelMap::with_targets(sink, channels, targets)?,
            Mapping::Identity(sink) => ChannelMap::new(sink, channels),
            // A route past the configuration chosen (the routing knew the
            // device's widest one) is not heard, as the routing refuses
            // channels the device lacks.
            Mapping::Routes(routes) => ChannelMap::with_routes(
                channels,
                routes
                    .into_iter()
                    .map(|r| r.filter(|&t| t < channels))
                    .collect(),
            )?,
        };
        Ok(CoreAudioDriver {
            device,
            name: info.name,
            map,
            choice,
            request,
            plan,
            stats: Arc::new(CallbackStats::default()),
            running: None,
            scheduling: None,
            secondaries: Vec::new(),
            epoch: Instant::now(),
            captures: Vec::new(),
            record_frames: None,
            captured: Vec::new(),
            records: Vec::new(),
        })
    }

    /// With the secondary devices `specs` (ADR 0052): each start opens
    /// their streams beside the clock device's, each stop stops them, and
    /// [`poll_secondaries`](CoreAudioDriver::poll_secondaries) rebuilds one
    /// that failed once it is back. A secondary device that cannot be opened
    /// never fails the run: its status says why it is not heard.
    pub fn with_secondaries(mut self, specs: Vec<SecondarySpec>) -> CoreAudioDriver {
        self.secondaries = specs.into_iter().map(Secondary::new).collect();
        self
    }

    /// With the input devices `specs` (ADR 0038 §7–8): each start opens
    /// their streams and feeds the run's sources from them, through a ring
    /// for the clock device's input, a clock adapter for another device's
    /// (ADR 0052 §8). With `record_frames`, each capture side's words are
    /// recorded into a ring of that many frames
    /// ([`take_input_records`](CoreAudioDriver::take_input_records)). An
    /// input that cannot be opened never fails the run: its sources read 0
    /// and its status says why.
    pub fn with_captures(
        mut self,
        specs: Vec<CaptureSpec>,
        record_frames: Option<usize>,
    ) -> CoreAudioDriver {
        self.captures = specs;
        self.record_frames = record_frames;
        self
    }

    /// [`with_captures`](CoreAudioDriver::with_captures) on a driver held
    /// by reference (control thread, before a start).
    pub fn set_captures(&mut self, specs: Vec<CaptureSpec>, record_frames: Option<usize>) {
        self.captures = specs;
        self.record_frames = record_frames;
    }

    /// The input devices' status.
    pub fn captures(&self) -> Vec<CaptureStatus> {
        self.captured.iter().map(Capture::status).collect()
    }

    /// The last start's input records (ADR 0038 §9), once.
    pub fn take_input_records(&mut self) -> Vec<InputRecord> {
        std::mem::take(&mut self.records)
    }

    /// Opens input device `spec` for the run (control thread): its stream
    /// at the rate it runs at with its own buffer, and one [`RunInput`] per
    /// capture side.
    fn open_capture(&self, spec: &CaptureSpec) -> Result<OpenedCapture, String> {
        let name = spec.name.clone();
        let device = find(Direction::Input, &spec.selector).map_err(|e| e.to_string())?;
        let info = info(&device, Direction::Input, None).map_err(|e| e.to_string())?;
        let state = state_of(&device);
        let rate = state
            .rate
            .or_else(|| current_rate(&device, Direction::Input))
            .ok_or_else(|| format!("{name}: its rate cannot be read"))?;
        let period = state.buffer.unwrap_or(FALLBACK_PERIOD);
        let choice = match choose_input(&info, rate, period) {
            Err(AudioError::BufferSize { min, max, .. }) => {
                let mut choice =
                    choose_input(&info, rate, period.clamp(min, max)).map_err(|e| e.to_string())?;
                choice.buffer = period;
                choice
            }
            other => other.map_err(|e| e.to_string())?,
        };
        let channels = usize::from(choice.channels);
        let clock_rate = self.choice.sample_rate;
        let clock_period = self.plan.period as usize;
        let clock_max = self.choice.max_frames as usize;
        // One crystal: the clock device's own input needs no converter.
        let adapted =
            id_of(&device).is_none() || id_of(&device) != id_of(&self.device) || rate != clock_rate;
        let max_frames = (choice.max_frames as usize).max(period as usize);
        let stats = Arc::new(SecondaryStats::default());
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut feeds = Vec::new();
        let mut inputs = Vec::new();
        let mut records = Vec::new();
        let mut counts = Vec::new();
        for side in &spec.sides {
            let channels_of = side
                .channels
                .iter()
                .map(|c| c.filter(|&c| c < channels))
                .collect();
            let (sink, source): (SideSink, Box<dyn InputSource>) = if adapted {
                let config = AdapterConfig::new(
                    side.width.max(1),
                    rate,
                    clock_rate,
                    period as usize,
                    clock_period,
                );
                let (input, output) = clock_adapter(config).map_err(|e| format!("{name}: {e}"))?;
                (
                    SideSink::Adapter(input),
                    Box::new(AdaptedSource::new(output, self.epoch, Arc::clone(&stats))),
                )
            } else {
                let (tx, rx) = capture_ring(side.width, period as usize, clock_period);
                (SideSink::Ring(tx), Box::new(rx))
            };
            feeds.push(SideFeed::new(
                channels_of,
                sink,
                max_frames,
                Arc::clone(&dropped),
            ));
            let (input, record) = RunInput::new(
                side.address.clone(),
                side.width,
                source,
                clock_max,
                self.record_frames,
            );
            counts.push(input.counts());
            inputs.push(input);
            records.extend(record);
        }
        let mut feed = CaptureFeed::new(feeds, self.epoch);
        let errors = Arc::clone(&stats);
        let config = StreamConfig {
            channels: choice.channels,
            sample_rate: rate,
            buffer_size: BufferSize::Default,
        };
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| feed.capture(data, channels),
                move |error: cpal::Error| {
                    errors.stream_error();
                    if matches!(
                        error.kind(),
                        cpal::ErrorKind::StreamInvalidated | cpal::ErrorKind::DeviceNotAvailable
                    ) {
                        errors.device_gone();
                    }
                },
                None,
            )
            .map_err(|e| format!("{name}: {e}"))?;
        if let Some(after) = state_of(&device).rate
            && after != rate
        {
            drop(stream);
            return Err(rate_moved(&name, rate, after));
        }
        stream.play().map_err(|e| format!("{name}: {e}"))?;
        Ok(OpenedCapture {
            stream,
            inputs,
            records,
            capture: Capture {
                spec: spec.clone(),
                rate: Some(rate),
                adapted,
                stats,
                counts,
                dropped,
                reason: None,
            },
        })
    }

    /// The secondary devices' status, in the routing's order.
    pub fn secondaries(&self) -> Vec<SecondaryStatus> {
        self.secondaries
            .iter()
            .map(|s| s.status(self.choice.sample_rate))
            .collect()
    }

    /// Control thread, now and then while running (ADR 0052 §6): a
    /// secondary device whose stream reported it gone (unplugged, its rate
    /// changed) is stopped, its feed emptied; one not playing is opened
    /// again, at most every [`SECONDARY_RETRY`], with a fresh adapter at the
    /// rate it runs at now. Returns what changed, one line each.
    pub fn poll_secondaries(&mut self) -> Vec<String> {
        let mut notices = Vec::new();
        let clock = (
            self.choice.sample_rate,
            self.plan.period,
            self.choice.max_frames,
        );
        let clock_id = id_of(&self.device);
        let Some(running) = self.running.as_mut() else {
            return notices;
        };
        running.feeds.collect();
        let now = Instant::now();
        for (slot, secondary) in self.secondaries.iter_mut().enumerate() {
            if secondary.stream.is_some() && secondary.stats.is_gone() {
                secondary.stop();
                running.feeds.set(slot, None);
                secondary.reason = Some("the device went away or its stream failed".to_owned());
                secondary.listed = rt_thread::device_ids().ok();
                secondary.retry_at = Some(now + SECONDARY_RETRY);
                notices.push(format!(
                    "{}: gone; {} not heard until it is back",
                    secondary.spec.name,
                    secondary.spec.roles.join(", ")
                ));
                continue;
            }
            // A device that failed is opened again when its wait is over,
            // or at once when the system's device list changed (plugged
            // in): one property read per poll in between, never a whole
            // open (tens of milliseconds) of a device that is not there.
            let listed_anew = || {
                let now = rt_thread::device_ids().ok();
                secondary.listed.is_some() && now != secondary.listed
            };
            if secondary.stream.is_none()
                && (secondary.retry_at.is_none_or(|at| now >= at) || listed_anew())
            {
                let before = secondary.reason.clone();
                match secondary.start(
                    clock,
                    clock_id.as_deref(),
                    self.epoch,
                    &mut running.feeds,
                    slot,
                ) {
                    Ok(missing) => {
                        notices.push(format!(
                            "{}: playing {} through a clock adapter",
                            secondary.spec.name,
                            secondary.spec.roles.join(", ")
                        ));
                        notices.extend(missing);
                    }
                    Err(reason) => {
                        if before.as_deref() != Some(reason.as_str()) {
                            notices.push(format!("{reason}; not heard"));
                        }
                        secondary.reason = Some(reason);
                        secondary.listed = rt_thread::device_ids().ok();
                        secondary.retry_at = Some(now + secondary.backoff);
                        secondary.backoff = (secondary.backoff * 2).min(SECONDARY_RETRY_MAX);
                    }
                }
            }
        }
        notices
    }

    /// The frames a callback is expected to hold, decided at the open
    /// (ADR 0036): what the engine is sized for.
    pub fn period(&self) -> u32 {
        self.plan.period
    }

    /// How the open decided to join the device, which every start keeps
    /// to: whether it was in use, and what is set.
    pub fn joined(&self) -> Option<JoinPlan> {
        Some(self.plan)
    }

    /// The device's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The channel map.
    pub fn map(&self) -> &ChannelMap {
        &self.map
    }

    /// The callback statistics.
    pub fn stats(&self) -> Arc<CallbackStats> {
        Arc::clone(&self.stats)
    }

    /// Why the audio system stopped the playing stream, in plain words, with
    /// the device's rate read now; `None` while it plays (or before a start).
    /// Control thread: it reads the device.
    pub fn fault(&self) -> Option<String> {
        let fault = self.stats.fault()?;
        let now = current_rate(&self.device, Direction::Output);
        Some(fault_message(
            &self.name,
            fault,
            self.choice.sample_rate,
            now,
        ))
    }

    /// How the last start scheduled the worker threads (`None` before one).
    pub fn scheduling(&self) -> Option<&Scheduling> {
        self.scheduling.as_ref()
    }

    /// Puts each worker into the time-constraint policy of the device's
    /// period and then into the device's I/O workgroup, or the policy alone
    /// where there is no workgroup ([`rt_thread::enter`]), and says which
    /// path each took. A worker that does not answer within
    /// [`WORKER_TIMEOUT`] is reported pending (it enters later).
    fn schedule(&self, workers: &Workers, computation: Computation) -> Scheduling {
        let count = workers.count();
        if count == 0 {
            return Scheduling::ordinary(workers.threads(), 0, "no worker threads");
        }
        let workgroup = workgroup_of(&self.device);
        let no_workgroup = workgroup.as_ref().err().cloned();
        let policy = TimeConstraint::for_period(
            self.choice.buffer,
            self.choice.sample_rate,
            rt_thread::timebase(),
            computation,
        );
        type Slots = Vec<Option<(WorkerPath, u32)>>;
        let paths: Arc<Mutex<Slots>> = Arc::new(Mutex::new(vec![None; count]));
        let slots = Arc::clone(&paths);
        // The request owns its workgroup reference until every worker has
        // run it; each joined worker retains its own.
        let generation = workers.post(move |w| {
            let path = rt_thread::enter(&policy, workgroup.as_ref().map_err(String::as_str));
            let port = rt_thread::current_port();
            let mut slots = slots.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(slot) = slots.get_mut(w) {
                *slot = Some((path, port));
            }
        });
        let _ = workers.wait(generation, WORKER_TIMEOUT);
        let paths = paths.lock().unwrap_or_else(PoisonError::into_inner);
        Scheduling {
            threads: workers.threads(),
            workers: paths
                .iter()
                .map(|p| {
                    p.as_ref().map_or_else(
                        || WorkerPath::None("pending: no answer within 1 s".to_owned()),
                        |(path, _)| path.clone(),
                    )
                })
                .collect(),
            no_workgroup,
            ports: paths
                .iter()
                .flatten()
                .filter(|(path, _)| !matches!(path, WorkerPath::None(_)))
                .map(|&(_, port)| port)
                .collect(),
        }
    }
}

impl Driver for CoreAudioDriver {
    fn sample_rate(&self) -> u32 {
        self.choice.sample_rate
    }

    fn channels(&self) -> usize {
        usize::from(self.choice.channels)
    }

    fn max_frames(&self) -> usize {
        self.choice.max_frames as usize
    }

    fn start(&mut self, runner: Runner) -> Result<(), DriverError> {
        if self.running.is_some() {
            return Err(DriverError::Running);
        }
        // ADR 0036: a device that runs for another app is joined as it is,
        // and no device's rate is switched: the run must be at its rate.
        let state = state_of(&self.device);
        let now = state
            .rate
            .or_else(|| current_rate(&self.device, Direction::Output));
        if let Some(now) = now
            && now != self.choice.sample_rate
        {
            return Err(DriverError::Device(rate_moved(
                &self.name,
                self.choice.sample_rate,
                now,
            )));
        }
        // The open's decision holds, or the start is refused: never a
        // silent change of the period the engine was sized for.
        let plan = self.plan;
        let now_plan = plan_join(self.request, &state);
        let left = plan.shared && !now_plan.shared;
        if now_plan != plan && !(left && state.buffer.is_none_or(|b| b == plan.period)) {
            return Err(DriverError::Device(join_moved(&self.name, plan, now_plan)));
        }
        // Before the first block: the workers enter real-time scheduling.
        let mut runner = runner;
        // The input devices (ADR 0038 §8), opened before the output so the
        // run's first block finds its sources fed; one that fails reads 0.
        let mut input_streams = Vec::new();
        let mut run_inputs = Vec::new();
        self.captured.clear();
        self.records.clear();
        for spec in self.captures.clone() {
            match self.open_capture(&spec) {
                Ok(opened) => {
                    input_streams.push(opened.stream);
                    run_inputs.extend(opened.inputs);
                    self.records.extend(opened.records);
                    self.captured.push(opened.capture);
                }
                Err(reason) => self.captured.push(Capture {
                    spec,
                    rate: None,
                    adapted: false,
                    stats: Arc::new(SecondaryStats::default()),
                    counts: Vec::new(),
                    dropped: Arc::default(),
                    reason: Some(reason),
                }),
            }
        }
        runner.set_inputs(run_inputs);
        let fade = fade_frames(self.choice.sample_rate);
        runner.fade_in(fade);
        let workers = runner.workers();
        self.scheduling = Some(self.schedule(&workers, runner.rt_options().computation));
        let (callback, control) = OutputCallback::new(
            runner,
            self.map.clone(),
            self.max_frames(),
            self.choice.sample_rate,
            Arc::clone(&self.stats),
        );
        let (mut callback, feeds) = callback
            .with_fade_out(fade)
            .with_feeds(self.secondaries.len());
        let config = StreamConfig {
            channels: self.choice.channels,
            sample_rate: self.choice.sample_rate,
            buffer_size: plan
                .set_buffer
                .map_or(BufferSize::Default, BufferSize::Fixed),
        };
        // A new stream: an earlier one's fault is forgotten.
        self.stats.clear_fault();
        let stats = Arc::clone(&self.stats);
        let wanted = self.choice.sample_rate;
        let before = current_rate(&self.device, Direction::Output);
        // Never shown bare (CoreAudio's "Unknown property"): with the rates.
        let failed = |error: cpal::Error| {
            let after = current_rate(&self.device, Direction::Output);
            DriverError::Device(start_failure(
                &self.name,
                wanted,
                before,
                after,
                "CoreAudio",
                &error.to_string(),
            ))
        };
        // The error callback runs on cpal's listener threads, the overload's
        // (an xrun) on the audio thread: atomics only. cpal 0.18.2 reports a
        // rate change (`StreamInvalidated`) only for a device opened by its
        // id: the system default output's stream (`DefaultOutputMonitor`)
        // has no rate listener, its AudioUnit converts the rate and plays on;
        // its loss without another default is `DeviceNotAvailable`.
        let epoch = self.epoch;
        let built = {
            self.device.build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    callback.fill_at(data, callback_time(epoch));
                },
                move |error: cpal::Error| {
                    stats.stream_error();
                    match error.kind() {
                        cpal::ErrorKind::StreamInvalidated => {
                            stats.stream_fault(StreamFault::RateChanged);
                        }
                        cpal::ErrorKind::DeviceNotAvailable => {
                            stats.stream_fault(StreamFault::Gone);
                        }
                        _ => {}
                    }
                },
                None,
            )
        };
        let stream = match built {
            Ok(stream) => stream,
            Err(error) => {
                release_workers(&workers);
                return Err(failed(error));
            }
        };
        // cpal 0.18.2 sets the nominal rate while it builds a stream when the
        // device's differs (its `set_sample_rate`): a rate that changed since
        // the check above would have been switched. Read again; refuse.
        let after = state_of(&self.device).rate;
        if let Some(after) = after
            && after != wanted
        {
            drop(stream);
            release_workers(&workers);
            return Err(DriverError::Device(rate_moved(&self.name, wanted, after)));
        }
        if let Err(error) = stream.play() {
            drop(stream);
            release_workers(&workers);
            return Err(failed(error));
        }
        self.running = Some(Running {
            stream,
            control,
            workers,
            feeds,
            inputs: input_streams,
        });
        // The secondary devices, once the clock device plays: one that
        // cannot be opened is reported in its status, not refused.
        for secondary in &mut self.secondaries {
            secondary.reason = None;
            secondary.retry_at = None;
            secondary.backoff = SECONDARY_RETRY;
            secondary.opened = 0;
        }
        let _ = self.poll_secondaries();
        Ok(())
    }

    fn stop(&mut self) -> Result<Option<Runner>, DriverError> {
        let Some(Running {
            stream,
            mut control,
            workers,
            feeds,
            inputs,
        }) = self.running.take()
        else {
            return Ok(None);
        };
        control.request_stop();
        // A stream the audio system stopped calls no more back: its Runner
        // is dropped with it, here.
        let timeout = if self.stats.fault().is_some() {
            let periods =
                2.0 * f64::from(self.choice.max_frames) / f64::from(self.choice.sample_rate.max(1));
            FAULT_STOP_TIMEOUT.max(Duration::from_secs_f64(periods))
        } else {
            STOP_TIMEOUT
        };
        let deadline = Instant::now() + timeout;
        let runner = loop {
            if let Some(runner) = control.take_runner() {
                break Some(runner);
            }
            if Instant::now() >= deadline {
                break None;
            }
            thread::sleep(Duration::from_millis(1));
        };
        let _ = stream.pause();
        // No block runs any more: the workers leave real-time scheduling.
        release_workers(&workers);
        // The callback, and the Runner if it was never handed back, are
        // dropped here, on this thread.
        drop(stream);
        drop(feeds);
        for input in inputs {
            let _ = input.pause();
        }
        // The secondary devices play out what their adapters hold (the
        // fade-out), then stop.
        let rate = self.choice.sample_rate;
        if let Some(drain) = self.secondaries.iter().map(|s| s.drain(rate)).max()
            && !drain.is_zero()
        {
            thread::sleep(drain);
        }
        for secondary in &mut self.secondaries {
            secondary.stop();
        }
        match runner {
            Some(runner) => Ok(Some(runner)),
            None => Err(DriverError::Device(
                "the device stopped calling back before handing the runner back".to_owned(),
            )),
        }
    }
}

impl Drop for CoreAudioDriver {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// A playing input stream, for metering only: its callback is an
/// [`InputCallback`], whose meters [`meters`](InputCapture::meters) hands
/// out. Input never reaches a Runner. Dropping it stops and drops the stream
/// on the dropping thread.
pub struct InputCapture {
    name: String,
    stream: Stream,
    meters: InputMeters,
    stats: Arc<CallbackStats>,
}

impl InputCapture {
    /// Opens and starts the input device `selector` names at the rate it
    /// runs at (never switched, ADR 0036; `sample_rate` only when its rate
    /// cannot be read), with every channel it offers there, requesting a buffer of
    /// `buffer` frames (the device's own when it cannot use that), with room
    /// for `capacity` meters between drains.
    pub fn open(
        selector: &DeviceSelector,
        sample_rate: u32,
        buffer: u32,
        capacity: usize,
    ) -> Result<InputCapture, AudioError> {
        InputCapture::open_with(
            selector,
            sample_rate,
            BufferRequest::Frames(buffer),
            capacity,
        )
    }

    /// [`open`](InputCapture::open) with the buffer `request` (ADR 0036): a
    /// device that runs for another app keeps its buffer.
    pub fn open_with(
        selector: &DeviceSelector,
        sample_rate: u32,
        request: BufferRequest,
        capacity: usize,
    ) -> Result<InputCapture, AudioError> {
        let device = find(Direction::Input, selector)?;
        let info = info(&device, Direction::Input, None)?;
        let state = state_of(&device);
        // Never switched (ADR 0036): metered at the rate it runs at.
        let sample_rate = state.rate.or(info.rate).unwrap_or(sample_rate);
        let plan = plan_join(request, &state);
        let fixed = |choice: Choice| match plan.set_buffer {
            Some(_) => BufferSize::Fixed(choice.buffer),
            None => BufferSize::Default,
        };
        let (choice, buffer_size) = match choose_input(&info, sample_rate, plan.period) {
            Ok(choice) => (choice, fixed(choice)),
            Err(AudioError::BufferSize { min, .. }) => {
                (choose_input(&info, sample_rate, min)?, BufferSize::Default)
            }
            Err(error) => return Err(error),
        };
        let config = StreamConfig {
            channels: choice.channels,
            sample_rate,
            buffer_size,
        };
        let (mut callback, meters) = InputCallback::new(usize::from(choice.channels), capacity);
        let stats = Arc::new(CallbackStats::default());
        let errors = Arc::clone(&stats);
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| callback.capture(data),
                move |_: cpal::Error| errors.stream_error(),
                None,
            )
            .map_err(host_error)?;
        stream.play().map_err(host_error)?;
        Ok(InputCapture {
            name: info.name,
            stream,
            meters,
            stats,
        })
    }

    /// The device's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The meters its callback sent.
    pub fn meters(&mut self) -> &mut InputMeters {
        &mut self.meters
    }

    /// Errors the input stream reported.
    pub fn stream_errors(&self) -> u64 {
        self.stats.snapshot().stream_errors
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        let _ = self.stream.pause();
    }
}
