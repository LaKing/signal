//! How a run opens its output and its input from the Setup: what the
//! interface dialog's Apply plays with (M2.5), shared by Signal.app's state
//! and the UI protocol's host (`setup.apply`, M2.8-1). Control-thread code:
//! a driver is opened, handed the Runner, and stopped here; the Runner comes
//! back and is dropped here, never on the audio thread.

use signal_audio::{
    AudioError, CaptureSideSpec, CaptureSpec, CaptureStatus, ChannelMap, CoreAudioDriver,
    DeviceInfo, DeviceRef, DeviceSelector, Direction, InputCallback, InputCapture, InputMeters,
    Resolution, Scheduling, SecondarySpec, SecondaryStatus, StatsSnapshot, choose_input,
    choose_output, labels, rate_moved, rate_notice, resolve,
};
use signal_output::inputs::InputRecord;
use signal_output::{Driver, DriverError, NullDriver, Runner};
use signal_project::session::InputDevice;

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
        // ADR 0036: `buffer = "device"` stays until another number is chosen
        // (the dialog shows the default for it).
        let buffer = self.buffer.max(1);
        setup.buffer_device &= buffer == setup.buffer;
        setup.buffer = buffer;
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

    /// The frames a device callback is expected to hold (ADR 0036: the
    /// device's own buffer while it runs for another app); `None` for the
    /// null driver, whose owner chooses.
    pub fn period(&self) -> Option<u32> {
        match self {
            Output::Device(driver) => Some(driver.period()),
            Output::Null(..) => None,
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

    /// The secondary devices' status (ADR 0052; none for the null driver).
    pub fn secondaries(&self) -> Vec<SecondaryStatus> {
        match self {
            Output::Device(driver) => driver.secondaries(),
            Output::Null(..) => Vec::new(),
        }
    }

    /// Control thread, now and then while running (ADR 0052 §6): stops a
    /// secondary device that went away, rebuilds one that is back; what
    /// changed, one line each.
    pub fn poll_secondaries(&mut self) -> Vec<String> {
        match self {
            Output::Device(driver) => driver.poll_secondaries(),
            Output::Null(..) => Vec::new(),
        }
    }

    /// Captures `specs` from the next start (ADR 0038 §7–8), each capture
    /// side recorded into a ring of `record_frames` when given (a run that
    /// records a session, ADR 0038 §9: [`take_input_records`](Output::take_input_records));
    /// the null driver captures nothing (its sources read 0).
    pub fn set_captures(&mut self, specs: Vec<CaptureSpec>, record_frames: Option<usize>) {
        if let Output::Device(driver) = self {
            driver.set_captures(specs, record_frames);
        }
    }

    /// The last start's input records (ADR 0038 §9), once; none for the
    /// null driver.
    pub fn take_input_records(&mut self) -> Vec<InputRecord> {
        match self {
            Output::Device(driver) => driver.take_input_records(),
            Output::Null(..) => Vec::new(),
        }
    }

    /// The input devices' status (none for the null driver).
    pub fn captures(&self) -> Vec<CaptureStatus> {
        match self {
            Output::Device(driver) => driver.captures(),
            Output::Null(..) => Vec::new(),
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

/// The rate a live run plays at with the Setup's output (ADR 0036, the
/// owner's decision of 2026-10-23: the device's clock is the run's): the
/// rate the output runs at now, never switched, and the notice to show when
/// it is not `project_rate`, the project's `sample_rate` (the rate of its
/// renders and a preference, `project` naming it). `project_rate` when the
/// output's rate cannot be read or it is not found (opening it then says
/// why). The null backend takes a listed device's rate, as CoreAudio would.
pub fn live_rate(
    backend: &Backend,
    devices: &[DeviceInfo],
    setup: &Setup,
    project_rate: u32,
    project: &str,
) -> (u32, Option<String>) {
    let found = match backend {
        Backend::CoreAudio => {
            let selector = match &setup.output {
                Some(entry) => match selector(devices, entry, Direction::Output) {
                    Ok((selector, _)) => selector,
                    Err(_) => return (project_rate, None),
                },
                None => DeviceSelector::Default,
            };
            signal_audio::live_rate(&selector, project_rate).ok()
        }
        Backend::Null(_) => {
            let device = match &setup.output {
                Some(entry) => selector(devices, entry, Direction::Output)
                    .ok()
                    .map(|s| s.1),
                None => devices
                    .iter()
                    .find(|d| d.direction == Direction::Output && d.default),
            };
            device.map(|d| (d.name.clone(), d.rate.unwrap_or(project_rate)))
        }
    };
    match found {
        Some((name, rate)) => (rate, rate_notice(&name, project, project_rate, rate)),
        None => (project_rate, None),
    }
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
            let request = setup.buffer_request();
            let open =
                |targets| CoreAudioDriver::open_with(&selector, rate, request, width, targets);
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
            // A listed device at another rate than the run's: the run was
            // not prepared at the device's rate ([`live_rate`]); no device's
            // rate is switched (ADR 0036), as the CoreAudio driver refuses.
            let played = output.as_ref().map(|(_, d)| d).or_else(|| {
                devices
                    .iter()
                    .find(|d| d.direction == Direction::Output && d.default)
            });
            if let Some(device) = played
                && let Some(now) = device.rate
                && now != rate
            {
                return Err(rate_moved(&device.name, rate, now));
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

/// The output of a run of several play sides (ADR 0038, M4.1), and how they
/// are routed: [`open_output`] for the routing of `sides` on the Setup's
/// sound system ([`crate::devices::sound_system`], the Setup as
/// [`crate::devices::effective`] merged it), the clock device being the
/// Setup's output. The routing's notices (a role not heard, a channel
/// refused) go to `notes`; the Runner is built for
/// [`Routing::addresses`](signal_output::Routing::addresses)
/// ([`signal_output::realtime_routed`]), whose frame is
/// [`Routing::width`](signal_output::Routing::width) channels. On CoreAudio,
/// a role the Setup binds to another present output plays there through a
/// clock adapter (ADR 0052, [`CoreAudioDriver::with_secondaries`]); the null
/// driver plays the clock device's channels only.
#[allow(clippy::too_many_arguments)]
pub fn open_routed(
    backend: &Backend,
    devices: &[DeviceInfo],
    setup: &Setup,
    rate: u32,
    sides: &[signal_output::PlaySide],
    seed: u64,
    notes: &mut Vec<String>,
) -> Result<(Output, signal_output::Routing), String> {
    let clock = crate::devices::clock_device(devices, setup);
    let clock_channels = clock.map(|d| {
        d.configs
            .iter()
            .map(|c| usize::from(c.channels))
            .max()
            .unwrap_or(0)
    });
    let mut system = crate::devices::sound_system(setup, devices);
    // As one sink plays (`open_output`): a map of `main` the device cannot
    // take plays on the identity map, with a note.
    // The side that plays as main: `main`'s, else the first role when it
    // has no binding of its own (`signal_output::route`'s exception).
    let main_width = signal_project::device::main_side(sides)
        .filter(|s| s.role == signal_output::MAIN || !system.roles.contains_key(&s.role))
        .map(|s| s.width);
    if let (Some(width), Some(binding)) = (main_width, system.roles.get_mut(signal_output::MAIN))
        && binding.placement == signal_output::Placement::Clock
    {
        let limit = clock_channels.unwrap_or(usize::MAX);
        let fits = binding.channels.len() == width
            && binding.channels.iter().enumerate().all(|(i, &c)| {
                c >= 1
                    && usize::from(c) <= limit
                    && !binding.channels.iter().take(i).any(|&x| x == c)
            });
        if !fits {
            notes.push(format!(
                "output channel map {:?}: the device cannot take it for main's {width} channels; playing on the identity map",
                binding.channels
            ));
            binding.channels = (1..=width).filter_map(|c| u16::try_from(c).ok()).collect();
        }
    }
    let routing = signal_output::route(sides, &system, clock_channels);
    notes.extend(routing.notices.iter().cloned());
    let routes = routing.targets();
    let wanted = routing.device_width();
    let output = match &setup.output {
        Some(entry) => {
            let (selector, device) = selector(devices, entry, Direction::Output)?;
            Some((selector, device.clone()))
        }
        None => None,
    };
    let opened = match backend {
        Backend::CoreAudio => {
            let selector = output.map_or(DeviceSelector::Default, |(s, _)| s);
            let secondaries = secondary_specs(devices, setup, &routing, notes);
            CoreAudioDriver::open_routed(&selector, rate, setup.buffer_request(), routes)
                .map(|driver| Output::Device(Box::new(driver.with_secondaries(secondaries))))
        }
        Backend::Null(_) => {
            let played = output.as_ref().map(|(_, d)| d).or(clock);
            if let Some(device) = played
                && let Some(now) = device.rate
                && now != rate
            {
                return Err(rate_moved(&device.name, rate, now));
            }
            // The null driver plays the Runner's frame as it is: as wide
            // as the frame, the map telling where a device would play it.
            let width = routing.width().max(1);
            let channels = match &output {
                Some((_, device)) => choose_output(device, rate, setup.buffer, wanted)
                    .map(|c| usize::from(c.channels).max(width)),
                None => Ok(wanted.max(width)),
            };
            channels.and_then(|channels| {
                let map = ChannelMap::with_routes(channels, routes)?;
                Ok(Output::Null(
                    Box::new(NullDriver::new(rate, channels, seed)),
                    map,
                ))
            })
        }
    };
    opened
        .map(|output| (output, routing))
        .map_err(|error| error.to_string())
}

/// How many seconds of input a capture side's record ring holds (ADR 0038
/// §9): the control thread drains it at every status poll, far more often.
pub const INPUT_RECORD_SECONDS: usize = 10;

/// The record ring of a run at `rate` that records a session
/// ([`INPUT_RECORD_SECONDS`]), else none.
pub fn input_record_frames(rate: u32, recording: bool) -> Option<usize> {
    recording.then(|| (rate as usize).saturating_mul(INPUT_RECORD_SECONDS))
}

/// The input records of a start, each with the device and channels it
/// captured from in `specs`, as a session records them
/// ([`LiveRecorder::with_inputs`](signal_project::session::LiveRecorder::with_inputs)).
pub fn input_records(
    records: Vec<InputRecord>,
    specs: &[CaptureSpec],
) -> Vec<(InputRecord, Option<InputDevice>)> {
    records
        .into_iter()
        .map(|record| {
            let device = specs.iter().find_map(|spec| {
                spec.sides
                    .iter()
                    .find(|side| side.address == record.address)
                    .map(|side| InputDevice {
                        name: spec.name.clone(),
                        channels: side
                            .channels
                            .iter()
                            .map(|c| c.and_then(|c| u16::try_from(c + 1).ok()).unwrap_or(0))
                            .collect(),
                    })
            });
            (record, device)
        })
        .collect()
}

/// The input devices a run of `project`'s `plan` captures from (ADR 0038
/// §4, §7): its capture sides routed on the Setup's `[capture.<role>]`, the
/// Setup's `input` (else the system default input) for a binding without a
/// device and for `main` without a binding. Each device is found by
/// identity; one that cannot be is noted and its sides read 0. Nothing
/// for the null driver.
pub fn capture_specs(
    backend: &Backend,
    devices: &[DeviceInfo],
    setup: &Setup,
    sides: &[signal_output::CaptureSide],
    notes: &mut Vec<String>,
) -> Vec<CaptureSpec> {
    if sides.is_empty() {
        return Vec::new();
    }
    if matches!(backend, Backend::Null(_)) {
        notes.push("the null driver captures nothing; the project's inputs read 0".to_owned());
        return Vec::new();
    }
    // Every device a binding names, by label, with its selector.
    let mut selectors: Vec<(String, DeviceSelector)> = Vec::new();
    let mut find = |entry: &DeviceEntry, notes: &mut Vec<String>| -> Option<String> {
        match selector(devices, entry, Direction::Input) {
            Ok((selector, device)) => {
                let label = labels(devices, Direction::Input)
                    .into_iter()
                    .find(|(i, _)| devices.get(*i).is_some_and(|d| std::ptr::eq(d, device)))
                    .map_or_else(|| device.name.clone(), |(_, l)| l);
                if !selectors.iter().any(|(l, _)| *l == label) {
                    selectors.push((label.clone(), selector));
                }
                Some(label)
            }
            Err(reason) => {
                notes.push(format!("input {}: {reason}", entry.name));
                None
            }
        }
    };
    let default_input = match &setup.input {
        Some(entry) => find(entry, notes),
        None => devices
            .iter()
            .find(|d| d.direction == Direction::Input && d.default)
            .and_then(|d| {
                let entry = DeviceEntry {
                    name: d.name.clone(),
                    id: d.id.clone(),
                    signature: String::new(),
                };
                find(&entry, notes)
            }),
    };
    let mut bindings = std::collections::BTreeMap::new();
    for (role, entry) in &setup.capture {
        // A device not found keeps its role bound to it, so the role reads
        // 0 (never main's input: an unplugged mic must not become the room
        // mic on the speakers).
        let device = entry.device.as_ref().map(|device| {
            find(device, notes).unwrap_or_else(|| format!("{} (not present)", device.name))
        });
        bindings.insert(role.clone(), (device, entry.channels.clone()));
    }
    let (routed, routing_notes) =
        signal_output::route_captures(sides, &bindings, default_input.as_deref());
    notes.extend(routing_notes);
    let mut specs: Vec<CaptureSpec> = Vec::new();
    for side in routed {
        let signal_output::Captured::Device(label) = &side.from else {
            continue;
        };
        let Some((_, selector)) = selectors.iter().find(|(l, _)| l == label) else {
            continue;
        };
        let spec_side = CaptureSideSpec {
            address: side.address.clone(),
            role: side.role.clone(),
            width: side.width,
            channels: side
                .channels
                .iter()
                .map(|c| c.map(|c| usize::from(c).saturating_sub(1)))
                .collect(),
        };
        match specs.iter_mut().find(|s| s.name == *label) {
            Some(spec) => spec.sides.push(spec_side),
            None => specs.push(CaptureSpec {
                selector: selector.clone(),
                name: label.clone(),
                sides: vec![spec_side],
            }),
        }
    }
    specs
}

/// The secondary devices of `routing` (ADR 0052), each found by the Setup's
/// entry of its first role; one that cannot be opened by identity is noted
/// and left out.
fn secondary_specs(
    devices: &[DeviceInfo],
    setup: &Setup,
    routing: &signal_output::Routing,
    notes: &mut Vec<String>,
) -> Vec<SecondarySpec> {
    let mut specs = Vec::new();
    for route in routing.secondaries() {
        let entry = route
            .roles
            .iter()
            .find_map(|role| setup.play.get(role)?.device.as_ref());
        let Some(entry) = entry else {
            continue;
        };
        match selector(devices, entry, Direction::Output) {
            Ok((selector, _)) => specs.push(SecondarySpec {
                selector,
                name: route.device,
                roles: route.roles,
                sources: route.sources,
                targets: route.targets,
            }),
            Err(reason) => notes.push(format!(
                "{}: {reason}; {} not heard",
                route.device,
                route.roles.join(", ")
            )),
        }
    }
    specs
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
            InputCapture::open_with(&selector, rate, setup.buffer_request(), INPUT_METERS).map(
                |mut capture| {
                    let channels = capture.meters().channels();
                    (Input::Device(capture), channels)
                },
            )
        }
        Backend::Null(_) => {
            choose_input(&device, device.rate.unwrap_or(rate), buffer).map(|choice| {
                let channels = usize::from(choice.channels);
                let (callback, meters) = InputCallback::new(channels, INPUT_METERS);
                (Input::Null(Box::new(callback), meters), channels)
            })
        }
    };
    match opened {
        Ok(input) => Some(input),
        Err(error) => {
            notes.push(format!("no input meters: {error}"));
            None
        }
    }
}
