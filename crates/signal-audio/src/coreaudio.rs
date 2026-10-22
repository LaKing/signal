//! The cpal glue (ADR 0011): device listing, the output stream and the input
//! stream (metering only), and the real-time workers' lifecycle around the
//! output stream (ADR 0019 §4). The only code that calls cpal; everything it
//! decides is in [`choose_output`], [`OutputCallback`], [`InputCallback`],
//! [`CallbackStats`] and [`Scheduling`].

use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, Stream, StreamConfig, SupportedBufferSize};
use signal_engine::{Computation, Workers};
use signal_output::{Driver, DriverError, Runner};

use crate::callback::{CallbackControl, OutputCallback};
use crate::config::{
    AudioError, Choice, ConfigRange, DeviceInfo, Direction, choose_input, choose_output,
    fault_message, start_failure,
};
use crate::input::{InputCallback, InputMeters};
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

/// How long [`switch_rate`] waits for the device to report the new rate.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(2);

/// How long [`switch_rate`] waits for the rate after the probe stream
/// failed (the switch may still have been made before the failure).
const SWITCH_FAILED_TIMEOUT: Duration = Duration::from_millis(500);

/// How long [`switch_rate`] lets the device settle at its new rate, so that
/// the change's late notifications reach no stream that plays.
const SWITCH_SETTLE: Duration = Duration::from_millis(250);

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
/// (cpal refuses those): such a device is not switched before its stream
/// is built, and a recovery takes it as ready.
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

/// Switches `device` to `config`'s rate before the stream that plays is
/// built: cpal sets the device's nominal rate while it builds a stream, and a
/// stream built across the change is stopped by the change's own
/// notification (measured on BlackHole 64ch, 2026-10-22: cpal reported
/// `StreamInvalidated`, "Device sample rate changed", and the stream never
/// called back). A stream built and dropped unplayed makes the switch; the
/// device then reports the rate, and settles. The probe's error is the
/// switch's only when the device did not reach the rate.
fn switch_rate(device: &Device, config: StreamConfig) -> Result<(), cpal::Error> {
    // The probe stream is dropped at once; only its error is kept.
    let probe = device
        .build_output_stream(
            config,
            |data: &mut [f32], _: &cpal::OutputCallbackInfo| data.fill(0.0),
            |_: cpal::Error| {},
            None,
        )
        .map(drop);
    let deadline = Instant::now()
        + if probe.is_ok() {
            SWITCH_TIMEOUT
        } else {
            SWITCH_FAILED_TIMEOUT
        };
    while current_rate(device, Direction::Output) != Some(config.sample_rate) {
        if Instant::now() >= deadline {
            // Not switched: the probe's error, or the device's rate says why.
            return probe;
        }
        thread::sleep(Duration::from_millis(10));
    }
    thread::sleep(SWITCH_SETTLE);
    Ok(())
}

/// The I/O workgroup of `device`: its CoreAudio object is found by its
/// position in the system's device list, which cpal lists in the same order
/// (`match_device`), or why there is none.
fn workgroup_of(device: &Device) -> Result<Workgroup, String> {
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
            Ok(object) => return rt_thread::device_workgroup(object),
            Err(DeviceMatch::Changed) => continue,
            Err(absent) => return Err(absent.to_string()),
        }
    }
    Err(DeviceMatch::Changed.to_string())
}

/// Takes the workers out of real-time scheduling ([`rt_thread::exit`]),
/// waiting up to [`WORKER_TIMEOUT`]; a late worker still does it later, or
/// when its thread ends.
fn release_workers(workers: &Workers) {
    let generation = workers.post(|_| rt_thread::exit());
    let _ = workers.wait(generation, WORKER_TIMEOUT);
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
    map: ChannelMap,
    stats: Arc<CallbackStats>,
    running: Option<Running>,
    scheduling: Option<Scheduling>,
}

impl CoreAudioDriver {
    /// Opens the output device named `name` (the default output device
    /// without one) for a sink of `sink_channels` at `sample_rate`, with a
    /// fixed buffer of `buffer` frames requested. The device must support the
    /// rate exactly: Signal does not resample.
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
        let device = find(Direction::Output, selector)?;
        let info = info(&device, Direction::Output, None)?;
        let width = targets
            .as_deref()
            .map_or(sink_channels, |t| ChannelMap::width_for(sink_channels, t));
        let choice = choose_output(&info, sample_rate, buffer, width)?;
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
            stats: Arc::new(CallbackStats::default()),
            running: None,
            scheduling: None,
        })
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
        // Before the first block: the workers enter real-time scheduling.
        let workers = runner.workers();
        self.scheduling = Some(self.schedule(&workers, runner.rt_options().computation));
        let (mut callback, control) = OutputCallback::new(
            runner,
            self.map.clone(),
            self.max_frames(),
            self.choice.sample_rate,
            Arc::clone(&self.stats),
        );
        let config = StreamConfig {
            channels: self.choice.channels,
            sample_rate: self.choice.sample_rate,
            buffer_size: BufferSize::Fixed(self.choice.buffer),
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
        let switched = match before {
            Some(rate) if rate != wanted => switch_rate(&self.device, config),
            _ => Ok(()),
        };
        // The error callback runs on cpal's listener threads, the overload's
        // (an xrun) on the audio thread: atomics only. cpal 0.18.2 reports a
        // rate change (`StreamInvalidated`) only for a device opened by its
        // id: the system default output's stream (`DefaultOutputMonitor`)
        // has no rate listener, its AudioUnit converts the rate and plays on;
        // its loss without another default is `DeviceNotAvailable`.
        let built = switched.and_then(|()| {
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
        });
        let stream = match built {
            Ok(stream) => stream,
            Err(error) => {
                release_workers(&workers);
                return Err(failed(error));
            }
        };
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
    /// Opens and starts the input device `selector` names at `sample_rate`
    /// exactly, with every channel it offers there, requesting a buffer of
    /// `buffer` frames (the device's own when it cannot use that), with room
    /// for `capacity` meters between drains.
    pub fn open(
        selector: &DeviceSelector,
        sample_rate: u32,
        buffer: u32,
        capacity: usize,
    ) -> Result<InputCapture, AudioError> {
        let device = find(Direction::Input, selector)?;
        let info = info(&device, Direction::Input, None)?;
        let (choice, buffer_size) = match choose_input(&info, sample_rate, buffer) {
            Ok(choice) => (choice, BufferSize::Fixed(choice.buffer)),
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
