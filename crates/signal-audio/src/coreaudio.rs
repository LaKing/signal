//! The cpal glue (ADR 0011): device listing, the output stream and the input
//! stream (metering only). The only code that calls cpal; everything it
//! decides is in [`choose_output`], [`OutputCallback`], [`InputCallback`] and
//! [`CallbackStats`].

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, Stream, StreamConfig, SupportedBufferSize};
use signal_engine::{Driver, DriverError, Runner};

use crate::callback::{CallbackControl, OutputCallback};
use crate::config::{
    AudioError, Choice, ConfigRange, DeviceInfo, Direction, choose_input, choose_output,
};
use crate::input::{InputCallback, InputMeters};
use crate::map::ChannelMap;
use crate::stats::CallbackStats;

/// How long [`CoreAudioDriver::stop`] waits for the callback to hand the
/// Runner back.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

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

/// A [`Driver`] on a CoreAudio output device, through cpal.
/// [`start`](Driver::start) builds and plays the output stream, whose callback
/// is an [`OutputCallback`]; [`stop`](Driver::stop) asks it for the Runner
/// back, then stops and drops the stream on the calling thread.
pub struct CoreAudioDriver {
    device: Device,
    name: String,
    choice: Choice,
    map: ChannelMap,
    stats: Arc<CallbackStats>,
    running: Option<(Stream, CallbackControl)>,
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
        let stats = Arc::clone(&self.stats);
        let stream = self
            .device
            .build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| callback.fill(data),
                move |_: cpal::Error| stats.stream_error(),
                None,
            )
            .map_err(|e| DriverError::Device(e.to_string()))?;
        stream
            .play()
            .map_err(|e| DriverError::Device(e.to_string()))?;
        self.running = Some((stream, control));
        Ok(())
    }

    fn stop(&mut self) -> Result<Option<Runner>, DriverError> {
        let Some((stream, mut control)) = self.running.take() else {
            return Ok(None);
        };
        control.request_stop();
        let deadline = Instant::now() + STOP_TIMEOUT;
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
