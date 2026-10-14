//! The interface setup dialog's draft, without a window: what is chosen
//! until Apply. A chosen device is kept by its identity ([`DeviceRef`]),
//! never by its place in the list, and found again after every refresh. A
//! saved device the list does not have is *unresolved*: it is shown as such,
//! Apply keeps it as saved, and only an explicit choice replaces it.

use signal_audio::{DeviceInfo, DeviceRef, Direction, Resolution, resolve};
pub use signal_ui::setup::{Choice, capabilities, channels, found, rate_check};

use crate::setup::DeviceEntry;
use crate::state::{Interface, Selection, View};

/// What the dialog edits until Apply.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// The output.
    pub output: Choice,
    /// The input, metered only.
    pub input: Choice,
    /// The buffer in frames.
    pub buffer: u32,
    /// "CPU workers": threads per block, the audio thread included; `None`
    /// is auto (one per performance core).
    pub cpu_workers: Option<u32>,
    /// The output channel map as typed: `1, 2`, or empty for the identity.
    pub channels: String,
}

fn choice(saved: &Option<DeviceEntry>, devices: &[DeviceInfo], direction: Direction) -> Choice {
    match saved {
        None => Choice::Unset,
        Some(entry) => found(devices, direction, &entry.to_ref()),
    }
}

impl Draft {
    /// The draft of the Setup in `view`, its devices found in its list.
    pub fn new(view: &View) -> Draft {
        let setup = view.setup.clone().unwrap_or_default();
        let devices = &view.devices.devices;
        Draft {
            output: choice(&setup.output, devices, Direction::Output),
            input: choice(&setup.input, devices, Direction::Input),
            buffer: setup.buffer,
            cpu_workers: setup.cpu_workers,
            channels: setup
                .output_channels
                .map(|c| c.iter().map(u16::to_string).collect::<Vec<_>>().join(", "))
                .unwrap_or_default(),
        }
    }

    /// Finds the chosen devices again in a refreshed list; one that is gone
    /// or no longer told apart becomes unresolved.
    pub fn refreshed(&mut self, devices: &[DeviceInfo]) {
        for (choice, direction) in [
            (&mut self.output, Direction::Output),
            (&mut self.input, Direction::Input),
        ] {
            if let Choice::Device(device) = choice {
                *choice = found(devices, direction, device);
            }
        }
    }

    fn choice(&self, direction: Direction) -> &Choice {
        match direction {
            Direction::Output => &self.output,
            Direction::Input => &self.input,
        }
    }

    /// The chosen device's place in `devices`, if a device is chosen there.
    pub fn index(&self, direction: Direction, devices: &[DeviceInfo]) -> Option<usize> {
        match self.choice(direction) {
            Choice::Device(device) => match resolve(devices, direction, device) {
                Resolution::Found(index) => Some(index),
                _ => None,
            },
            _ => None,
        }
    }

    /// An explicit choice: the device at `index` in `devices`, or none (the
    /// system default output, no input).
    pub fn select(&mut self, direction: Direction, index: Option<usize>, devices: &[DeviceInfo]) {
        let choice = match index.and_then(|i| devices.get(i)) {
            Some(device) => Choice::Device(DeviceRef::of(device)),
            None => Choice::Unset,
        };
        match direction {
            Direction::Output => self.output = choice,
            Direction::Input => self.input = choice,
        }
    }

    /// The output whose capabilities the dialog shows: the chosen device,
    /// or for the system default the listed default output.
    pub fn output_device<'a>(&self, devices: &'a [DeviceInfo]) -> Option<&'a DeviceInfo> {
        match &self.output {
            Choice::Device(_) => self
                .index(Direction::Output, devices)
                .and_then(|i| devices.get(i)),
            Choice::Unset => devices
                .iter()
                .find(|d| d.direction == Direction::Output && d.default),
            Choice::Unresolved(_) => None,
        }
    }

    /// The settings Apply sends; an error when the channel map does not read.
    pub fn interface(&self) -> Result<Interface, String> {
        let selection = |choice: &Choice| match choice {
            Choice::Unresolved(_) => Selection::Keep,
            Choice::Unset => Selection::Unset,
            Choice::Device(device) => Selection::Device(device.clone()),
        };
        Ok(Interface {
            output: selection(&self.output),
            input: selection(&self.input),
            buffer: self.buffer.max(1),
            cpu_workers: self.cpu_workers.map(|n| n.max(1)),
            output_channels: channels(&self.channels)?,
        })
    }
}
