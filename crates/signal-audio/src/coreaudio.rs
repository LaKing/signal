//! The cpal glue (ADR 0011): device listing and the output stream. The only
//! code that calls cpal; everything it decides is in [`choose_output`],
//! [`OutputCallback`] and [`CallbackStats`].

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, Stream, StreamConfig, SupportedBufferSize};
use signal_engine::{Driver, DriverError, Runner};

use crate::callback::{CallbackControl, OutputCallback};
use crate::config::{AudioError, Choice, ConfigRange, DeviceInfo, Direction, choose_output};
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

fn output_info(device: &Device, default: Option<&str>) -> Result<DeviceInfo, AudioError> {
    let name = name_of(device)?;
    let configs = device.supported_output_configs().map_err(host_error)?;
    Ok(DeviceInfo {
        default: default == Some(name.as_str()),
        name,
        direction: Direction::Output,
        configs: ranges(configs),
    })
}

/// The output and input devices, with their supported configurations.
pub fn devices() -> Result<Vec<DeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let default_out = host.default_output_device().and_then(|d| name_of(&d).ok());
    let default_in = host.default_input_device().and_then(|d| name_of(&d).ok());
    let mut list = Vec::new();
    for device in host.output_devices().map_err(host_error)? {
        list.push(output_info(&device, default_out.as_deref())?);
    }
    for device in host.input_devices().map_err(host_error)? {
        let name = name_of(&device)?;
        let configs = device.supported_input_configs().map_err(host_error)?;
        list.push(DeviceInfo {
            default: default_in.as_deref() == Some(name.as_str()),
            name,
            direction: Direction::Input,
            configs: ranges(configs),
        });
    }
    Ok(list)
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
        let host = cpal::default_host();
        let device = match name {
            None => host.default_output_device(),
            Some(wanted) => host
                .output_devices()
                .map_err(host_error)?
                .find(|d| name_of(d).is_ok_and(|n| n == wanted)),
        };
        let Some(device) = device else {
            let available = host
                .output_devices()
                .map_err(host_error)?
                .filter_map(|d| name_of(&d).ok())
                .collect();
            return Err(AudioError::NoDevice {
                name: name.map(str::to_owned),
                available,
            });
        };
        let info = output_info(&device, None)?;
        let choice = choose_output(&info, sample_rate, buffer, sink_channels)?;
        Ok(CoreAudioDriver {
            device,
            name: info.name,
            map: ChannelMap::new(sink_channels, usize::from(choice.channels)),
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
    pub fn map(&self) -> ChannelMap {
        self.map
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
            self.map,
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
