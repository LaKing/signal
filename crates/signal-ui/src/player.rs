//! How a run opens its output and its input from the Setup: what the
//! interface dialog's Apply plays with (M2.5), shared by Signal.app's state
//! and the UI protocol's host (`setup.apply`, M2.8-1). Control-thread code:
//! a driver is opened, handed the Runner, and stopped here; the Runner comes
//! back and is dropped here, never on the audio thread.

use signal_audio::{
    AudioError, ChannelMap, CoreAudioDriver, DeviceInfo, DeviceRef, DeviceSelector, Direction,
    InputCallback, InputCapture, InputMeters, Resolution, Scheduling, StatsSnapshot, choose_input,
    choose_output, resolve, start_failure,
};
use signal_output::{Driver, DriverError, NullDriver, Runner};

use crate::setup::{DeviceEntry, Setup};

/// Room for input meters between two control steps.
pub const INPUT_METERS: usize = 4096;

/// What Apply does with the Setup's output or input device.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Selection {
    /// Keeps the Setup's device: the dialog could not find it, and no other
    /// was chosen (a missing saved device is never replaced unasked).
    #[default]
    Keep,
    /// No device: the system's default output, or no input.
    Unset,
    /// This device.
    Device(DeviceRef),
}

impl Selection {
    /// Takes the selection into the Setup's entry.
    pub fn apply(self, saved: &mut Option<DeviceEntry>) {
        match self {
            Selection::Keep => {}
            Selection::Unset => *saved = None,
            Selection::Device(device) => *saved = Some(DeviceEntry::from_ref(device)),
        }
    }
}

/// The interface settings the dialog applies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interface {
    /// The output device.
    pub output: Selection,
    /// The input device, metered only.
    pub input: Selection,
    /// The buffer in frames.
    pub buffer: u32,
    /// "CPU workers": threads per block, the audio thread included; `None`
    /// is auto.
    pub cpu_workers: Option<u32>,
    /// The device channel (from 1) of each sink channel; identity without.
    pub output_channels: Option<Vec<u16>>,
}

impl Interface {
    /// Takes the settings into `setup`, as Apply does.
    pub fn apply(self, setup: &mut Setup) {
        self.output.apply(&mut setup.output);
        self.input.apply(&mut setup.input);
        setup.buffer = self.buffer.max(1);
        setup.cpu_workers = self.cpu_workers.map(|n| n.max(1));
        setup.output_channels = self.output_channels;
    }
}

/// Where audio goes.
#[derive(Clone, Debug)]
pub enum Backend {
    /// CoreAudio, through `signal-audio`.
    CoreAudio,
    /// No device: the null driver, run by its owner, and this device list.
    /// For tests and scripting.
    Null(Vec<DeviceInfo>),
}

/// The output a run plays on.
pub enum Output {
    // Boxed, as the null driver: both are large (the CoreAudio driver keeps
    // its workers' scheduling, M2.7-1 its ports too).
    /// The audio device.
    Device(Box<CoreAudioDriver>),
    /// The null driver, with the map a device would play (the null driver
    /// plays the identity).
    Null(Box<NullDriver>, ChannelMap),
}

impl Output {
    /// Hands the Runner to the driver.
    pub fn start(&mut self, runner: Runner) -> Result<(), DriverError> {
        match self {
            Output::Device(driver) => driver.start(runner),
            Output::Null(driver, _) => driver.start(runner),
        }
    }

    /// Stops the driver; the Runner comes back to the caller's thread.
    pub fn stop(&mut self) -> Result<Option<Runner>, DriverError> {
        match self {
            Output::Device(driver) => driver.stop(),
            Output::Null(driver, _) => driver.stop(),
        }
    }

    /// The device channel (from 0) sink channel `sink` plays on, if any.
    pub fn target(&self, sink: usize) -> Option<usize> {
        match self {
            Output::Device(driver) => driver.map().target(sink),
            Output::Null(_, map) => map.target(sink),
        }
    }

    /// The device's callback statistics (none for the null driver).
    pub fn stats(&self) -> StatsSnapshot {
        match self {
            Output::Device(driver) => driver.stats().snapshot(),
            Output::Null(..) => StatsSnapshot::default(),
        }
    }

    /// How the device's workers are scheduled (none for the null driver).
    pub fn scheduling(&self) -> Option<&Scheduling> {
        match self {
            Output::Device(driver) => driver.scheduling(),
            Output::Null(..) => None,
        }
    }

    /// Why the audio system stopped the playing stream (its rate changed,
    /// it went away), in plain words; `None` while it plays, and for the
    /// null driver.
    pub fn fault(&self) -> Option<String> {
        match self {
            Output::Device(driver) => driver.fault(),
            Output::Null(..) => None,
        }
    }

    /// The null driver, if this is one.
    pub fn null(&mut self) -> Option<&mut NullDriver> {
        match self {
            Output::Null(driver, _) => Some(driver),
            Output::Device(_) => None,
        }
    }
}

/// The input a run meters.
pub enum Input {
    /// The input device.
    Device(InputCapture),
    /// No device: the callback body, fed by its owner, and its meters.
    Null(Box<InputCallback>, InputMeters),
}

impl Input {
    /// Its meters.
    pub fn meters(&mut self) -> &mut InputMeters {
        match self {
            Input::Device(capture) => capture.meters(),
            Input::Null(_, meters) => meters,
        }
    }

    /// The device's stream errors.
    pub fn stream_errors(&self) -> u64 {
        match self {
            Input::Device(capture) => capture.stream_errors(),
            Input::Null(..) => 0,
        }
    }

    /// Null input only: one input callback of `data` (interleaved frames).
    pub fn push(&mut self, data: &[f32]) -> Result<(), String> {
        match self {
            Input::Null(callback, _) => {
                callback.capture(data);
                Ok(())
            }
            Input::Device(_) => Err("not a null input".to_owned()),
        }
    }
}

/// Where `entry` is among `devices` of `direction`: the device to open, or
/// why none is.
pub fn selector<'a>(
    devices: &'a [DeviceInfo],
    entry: &DeviceEntry,
    direction: Direction,
) -> Result<(DeviceSelector, &'a DeviceInfo), String> {
    match resolve(devices, direction, &entry.to_ref()) {
        Resolution::Found(index) => {
            let Some(device) = devices.get(index) else {
                return Err("the device list changed".to_owned());
            };
            match &device.id {
                Some(id) => Ok((DeviceSelector::Id(id.clone()), device)),
                // Opening by name takes the first of that name: only when the
                // name is unique.
                None if devices.iter().any(|d| {
                    d.direction == direction && d.name == device.name && !std::ptr::eq(d, device)
                }) =>
                {
                    Err(format!(
                        "the {} '{}' has no id and shares its name; it cannot be opened",
                        match direction {
                            Direction::Output => "output",
                            Direction::Input => "input",
                        },
                        device.name
                    ))
                }
                None => Ok((DeviceSelector::Name(device.name.clone()), device)),
            }
        }
        Resolution::Ambiguous { reason, .. } | Resolution::Missing { reason, .. } => Err(reason),
    }
}

/// The Setup's output in `devices` as it is now: its id (or name) and the
/// rate it runs at (`None` unread); the system's default output without a
/// saved device. `None` when it is not there. What a run stopped by the
/// device waits to change (`docs/ui-protocol.md`, "Stopped by the device").
pub fn output_state(devices: &[DeviceInfo], setup: &Setup) -> Option<(String, Option<u32>)> {
    let device = match &setup.output {
        Some(entry) => selector(devices, entry, Direction::Output).ok()?.1,
        None => devices
            .iter()
            .find(|d| d.direction == Direction::Output && d.default)?,
    };
    let key = device.id.clone().unwrap_or_else(|| device.name.clone());
    Some((key, device.rate))
}

/// Opens the Setup's output for `width` sink channels at `rate`, with its
/// buffer and channel map (a map the device cannot take falls back to the
/// identity, with a note).
pub fn open_output(
    backend: &Backend,
    devices: &[DeviceInfo],
    setup: &Setup,
    rate: u32,
    width: usize,
    seed: u64,
    notes: &mut Vec<String>,
) -> Result<Output, String> {
    let buffer = setup.buffer;
    let output = match &setup.output {
        Some(entry) => {
            let (selector, device) = selector(devices, entry, Direction::Output)?;
            Some((selector, device.clone()))
        }
        None => None,
    };
    let targets: Option<Vec<usize>> = setup.output_channels.as_ref().map(|c| {
        c.iter()
            .map(|&c| usize::from(c).saturating_sub(1))
            .collect()
    });
    let map_note = |error: AudioError, channels: &[u16]| {
        format!("output channel map {channels:?}: {error}; playing on the identity map",)
    };
    let opened = match backend {
        Backend::CoreAudio => {
            let selector = output.map_or(DeviceSelector::Default, |(s, _)| s);
            let open =
                |targets| CoreAudioDriver::open_selected(&selector, rate, buffer, width, targets);
            match open(targets.clone()) {
                Err(error @ AudioError::ChannelMap(_)) => {
                    let channels = setup.output_channels.clone().unwrap_or_default();
                    notes.push(map_note(error, &channels));
                    open(None)
                }
                other => other,
            }
            .map(|driver| Output::Device(Box::new(driver)))
        }
        Backend::Null(_) => {
            // A listed device running at another rate stands for one that
            // cannot be switched (the CoreAudio driver switches it, which
            // can fail): deterministic, for the tests.
            let played = output.as_ref().map(|(_, d)| d).or_else(|| {
                devices
                    .iter()
                    .find(|d| d.direction == Direction::Output && d.default)
            });
            if let Some(device) = played
                && let Some(now) = device.rate
                && now != rate
            {
                return Err(start_failure(
                    &device.name,
                    rate,
                    Some(now),
                    Some(now),
                    "null driver",
                    "a listed device keeps its rate",
                ));
            }
            // As `CoreAudioDriver::open_selected` chooses, on the listed
            // device (the default output is taken to be that wide).
            let wanted = targets
                .as_deref()
                .map_or(width, |t| ChannelMap::width_for(width, t));
            let channels = match &output {
                Some((_, device)) => {
                    choose_output(device, rate, buffer, wanted).map(|c| usize::from(c.channels))
                }
                None => Ok(wanted),
            };
            channels.map(|channels| {
                let map = match (targets, &setup.output_channels) {
                    (Some(targets), Some(saved)) => {
                        ChannelMap::with_targets(width, channels, targets).unwrap_or_else(|error| {
                            notes.push(map_note(error, saved));
                            ChannelMap::new(width, channels)
                        })
                    }
                    _ => ChannelMap::new(width, channels),
                };
                Output::Null(Box::new(NullDriver::new(rate, channels, seed)), map)
            })
        }
    };
    opened.map_err(|error| error.to_string())
}

/// The Setup's input, metered only, and its channels; a note when it cannot
/// be used (playing goes on without it).
pub fn open_input(
    backend: &Backend,
    devices: &[DeviceInfo],
    setup: &Setup,
    rate: u32,
    notes: &mut Vec<String>,
) -> Option<(Input, usize)> {
    let entry = setup.input.as_ref()?;
    let buffer = setup.buffer;
    let (selector, device) = match selector(devices, entry, Direction::Input) {
        Ok((selector, device)) => (selector, device.clone()),
        Err(reason) => {
            notes.push(format!("no input meters: {reason}"));
            return None;
        }
    };
    let opened = match backend {
        Backend::CoreAudio => {
            InputCapture::open(&selector, rate, buffer, INPUT_METERS).map(|mut capture| {
                let channels = capture.meters().channels();
                (Input::Device(capture), channels)
            })
        }
        Backend::Null(_) => choose_input(&device, rate, buffer).map(|choice| {
            let channels = usize::from(choice.channels);
            let (callback, meters) = InputCallback::new(channels, INPUT_METERS);
            (Input::Null(Box::new(callback), meters), channels)
        }),
    };
    match opened {
        Ok(input) => Some(input),
        Err(error) => {
            notes.push(format!("no input meters: {error}"));
            None
        }
    }
}
