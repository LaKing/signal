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

use crate::callback::{CallbackControl, OutputCallback};
use crate::config::{
    AudioError, Choice, ConfigRange, DeviceInfo, Direction, choose_input, choose_output,
    fault_message, rate_moved, start_failure,
};
use crate::device_props;
use crate::input::{InputCallback, InputMeters};
use crate::join::{BufferRequest, DeviceState, JoinPlan, plan_join};
use crate::map::ChannelMap;
use crate::rt_thread::{self, Workgroup};
use crate::scheduling::{DeviceMatch, Scheduling, TimeConstraint, WorkerPath, match_device};
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

/// What a started stream holds.
struct Running {
    stream: Stream,
    control: CallbackControl,
    workers: Workers,
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
        let device = find(Direction::Output, selector)?;
        let info = info(&device, Direction::Output, None)?;
        let width = targets
            .as_deref()
            .map_or(sink_channels, |t| ChannelMap::width_for(sink_channels, t));
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
        let map = match targets {
            Some(targets) => ChannelMap::with_targets(sink_channels, channels, targets)?,
            None => ChannelMap::new(sink_channels, channels),
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
        })
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
        let mut callback = callback.with_fade_out(fade);
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
        let built = {
            self.device.build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| callback.fill(data),
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
        });
        Ok(())
    }

    fn stop(&mut self) -> Result<Option<Runner>, DriverError> {
        let Some(Running {
            stream,
            mut control,
            workers,
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
