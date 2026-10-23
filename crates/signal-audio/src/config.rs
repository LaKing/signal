//! Devices as plain data, and the choice of an output configuration: the
//! project's sample rate exactly (Signal does not resample) and the requested
//! buffer.

use std::fmt;

use serde::Serialize;

use crate::stats::StreamFault;

/// Output or input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// An output device.
    Output,
    /// An input device.
    Input,
}

/// One supported stream configuration range of a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConfigRange {
    /// Channels.
    pub channels: u16,
    /// The lowest sample rate in Hz.
    pub min_rate: u32,
    /// The highest sample rate in Hz (equal to `min_rate` for a single rate).
    pub max_rate: u32,
    /// The buffer-size range in frames, if the device reports one.
    pub buffer: Option<(u32, u32)>,
    /// The sample format (`f32`, `i16`, …).
    pub sample_format: String,
}

/// A device, as `signal devices` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    /// Its name.
    pub name: String,
    /// The audio system's stable identifier (`coreaudio:<device UID>`),
    /// if it reports one. Same-named devices differ here.
    pub id: Option<String>,
    /// Output or input.
    pub direction: Direction,
    /// Whether it is the system's default device of its direction.
    pub default: bool,
    /// The sample rate it runs at now: its current stream format's, which
    /// follows the nominal rate Audio MIDI Setup shows and sets; `None` when
    /// it cannot be read (a format cpal does not read). A device opened at
    /// another rate is refused: Signal switches no device's rate (ADR 0036).
    pub rate: Option<u32>,
    /// Its supported configurations.
    pub configs: Vec<ConfigRange>,
}

/// The output configuration [`choose_output`] picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The device channels to open.
    pub channels: u16,
    /// The sample rate in Hz.
    pub sample_rate: u32,
    /// The buffer to request, in frames.
    pub buffer: u32,
    /// The most frames a callback may hold: the top of the buffer range, or
    /// the requested buffer when the device reports none.
    pub max_frames: u32,
}

/// Why no device could be opened, or listed.
#[derive(Debug, PartialEq, Eq)]
pub enum AudioError {
    /// The device does not run at the project's sample rate.
    SampleRate {
        /// The device.
        device: String,
        /// The project's rate.
        requested: u32,
        /// The supported rates, as (min, max) ranges.
        supported: Vec<(u32, u32)>,
    },
    /// The requested buffer is outside the device's range.
    BufferSize {
        /// The device.
        device: String,
        /// The requested frames.
        requested: u32,
        /// The lowest supported.
        min: u32,
        /// The highest supported.
        max: u32,
    },
    /// No device of that name or id (or no default device).
    NoDevice {
        /// Output or input.
        direction: Direction,
        /// The name or id asked for, if any.
        name: Option<String>,
        /// The devices of that direction there are.
        available: Vec<String>,
    },
    /// A channel map that does not fit the sink or the device.
    ChannelMap(String),
    /// The device offers no 32-bit float output.
    Format {
        /// The device.
        device: String,
    },
    /// The audio system reported an error.
    Host(String),
}

/// `48000` for a single rate, `8000-192000` for a range.
fn rate_range((min, max): (u32, u32)) -> String {
    if min == max {
        min.to_string()
    } else {
        format!("{min}-{max}")
    }
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::SampleRate {
                device,
                requested,
                supported,
            } => {
                let rates: Vec<String> = supported.iter().copied().map(rate_range).collect();
                write!(
                    f,
                    "the device '{device}' cannot run at {requested} Hz; it supports {} Hz. Signal does not resample: set sample_rate in project.toml to one of these",
                    rates.join(", ")
                )
            }
            AudioError::BufferSize {
                device,
                requested,
                min,
                max,
            } => write!(
                f,
                "the device '{device}' cannot use a buffer of {requested} frames; it supports {min} to {max}"
            ),
            AudioError::NoDevice {
                direction,
                name,
                available,
            } => {
                let kind = match direction {
                    Direction::Output => "output",
                    Direction::Input => "input",
                };
                match name {
                    Some(name) => write!(f, "no {kind} device is named '{name}'")?,
                    None => write!(f, "there is no default {kind} device")?,
                }
                if available.is_empty() {
                    write!(f, "; there are no {kind} devices")
                } else {
                    write!(f, "; {kind} devices: {}", available.join(", "))
                }
            }
            AudioError::ChannelMap(message) => write!(f, "the channel map: {message}"),
            AudioError::Format { device } => {
                write!(f, "the device '{device}' offers no 32-bit float output")
            }
            AudioError::Host(message) => write!(f, "the audio system: {message}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// The audio system's error text as a clause (`Unknown property` →
/// `unknown property`), never shown bare.
fn host_clause(message: &str) -> String {
    let message = message.trim();
    let mut chars = message.chars();
    let Some(first) = chars.next() else {
        return "no reason given".to_owned();
    };
    // A capitalised word only (`Unknown`), not a name (`CoreAudio`).
    let word = chars.as_str().split_whitespace().next().unwrap_or("");
    if !word.is_empty() && word.chars().all(char::is_lowercase) {
        first.to_lowercase().chain(chars).collect()
    } else {
        message.to_owned()
    }
}

/// What to do about a device that runs at `now` Hz for a run prepared at
/// `wanted` Hz: play again, at the device's rate (ADR 0036; Signal switches
/// no device's rate).
fn rate_advice(_wanted: u32, now: u32) -> String {
    format!("play again to play at {now} Hz")
}

/// Why the output stream of `device` did not start at `wanted` Hz, in plain
/// words: `system`'s error (`CoreAudio`, `Unknown property`) with the
/// device's sample rate before the attempt (`before`) and after it
/// (`after`), as read. Signal switches no device's rate (ADR 0036), so a
/// device found at another rate than `wanted` is why it did not start
/// (CoreAudio then reports, for example, "Unknown property").
pub fn start_failure(
    device: &str,
    wanted: u32,
    before: Option<u32>,
    after: Option<u32>,
    system: &str,
    error: &str,
) -> String {
    let error = host_clause(error);
    match (before, after) {
        (Some(before), _) if before != wanted => format!(
            "the device '{device}' runs at {before} Hz and did not start at {wanted} Hz ({system}: {error}); {}",
            rate_advice(wanted, before)
        ),
        (_, Some(after)) if after != wanted => format!(
            "the device '{device}' changed its sample rate to {after} Hz while starting at {wanted} Hz ({system}: {error}); {}",
            rate_advice(wanted, after)
        ),
        _ => format!("the device '{device}' did not start at {wanted} Hz ({system}: {error})"),
    }
}

/// Why a run prepared at `wanted` Hz did not start on `device`, which runs
/// at `now` Hz: its rate changed between the open and the start. Signal
/// switches no device's rate (ADR 0036, the owner's decision of 2026-10-23:
/// the device's clock is the run's); a run started again plays at `now`.
pub fn rate_moved(device: &str, wanted: u32, now: u32) -> String {
    format!(
        "the device '{device}' runs at {now} Hz, not at the {wanted} Hz this run was prepared for; Signal does not switch a device's rate: play again to play at {now} Hz"
    )
}

/// The notice of a live run at the device's rate, `plays` Hz, for a project
/// written for `written` Hz (its `sample_rate`, the rate of its renders);
/// `None` when they are the same (ADR 0036).
pub fn rate_notice(device: &str, project: &str, written: u32, plays: u32) -> Option<String> {
    (written != plays).then(|| {
        format!("'{device}' runs at {plays} Hz; {project} is written for {written} Hz and plays at {plays} Hz")
    })
}

/// Why a playing output stream of `device` at `wanted` Hz stopped, in plain
/// words, with the device's sample rate now (`now`), as read.
pub fn fault_message(device: &str, fault: StreamFault, wanted: u32, now: Option<u32>) -> String {
    match (fault, now) {
        (StreamFault::RateChanged, Some(now)) if now != wanted => format!(
            "the device '{device}' changed its sample rate to {now} Hz while playing at {wanted} Hz; {}",
            rate_advice(wanted, now)
        ),
        (StreamFault::RateChanged, _) => {
            format!("the device '{device}' changed its sample rate while playing at {wanted} Hz")
        }
        (StreamFault::Gone, _) => format!("the device '{device}' went away while playing"),
    }
}

/// The output configuration of `device` for a sink of `sink_channels` at
/// `sample_rate` with a buffer of `buffer` frames: an `f32` configuration
/// whose rate range holds the rate exactly, preferring as many channels as
/// the sink, then the fewest more, then the most. An error lists the rates or
/// buffer sizes the device supports.
pub fn choose_output(
    device: &DeviceInfo,
    sample_rate: u32,
    buffer: u32,
    sink_channels: usize,
) -> Result<Choice, AudioError> {
    let float: Vec<&ConfigRange> = device
        .configs
        .iter()
        .filter(|c| c.sample_format == "f32" && c.channels > 0)
        .collect();
    if float.is_empty() {
        return Err(AudioError::Format {
            device: device.name.clone(),
        });
    }
    let at_rate: Vec<&ConfigRange> = float
        .iter()
        .copied()
        .filter(|c| (c.min_rate..=c.max_rate).contains(&sample_rate))
        .collect();
    let width = |c: &ConfigRange| usize::from(c.channels);
    let config = at_rate
        .iter()
        .copied()
        .find(|c| width(c) == sink_channels)
        .or_else(|| {
            at_rate
                .iter()
                .copied()
                .filter(|c| width(c) > sink_channels)
                .min_by_key(|c| width(c))
        })
        .or_else(|| at_rate.iter().copied().max_by_key(|c| width(c)));
    let Some(config) = config else {
        let mut supported: Vec<(u32, u32)> =
            float.iter().map(|c| (c.min_rate, c.max_rate)).collect();
        supported.sort_unstable();
        supported.dedup();
        return Err(AudioError::SampleRate {
            device: device.name.clone(),
            requested: sample_rate,
            supported,
        });
    };
    let max_frames = match config.buffer {
        Some((min, max)) if !(min..=max).contains(&buffer) => {
            return Err(AudioError::BufferSize {
                device: device.name.clone(),
                requested: buffer,
                min,
                max,
            });
        }
        Some((_, max)) => max,
        None => buffer,
    };
    Ok(Choice {
        channels: config.channels,
        sample_rate,
        buffer,
        max_frames,
    })
}

/// The input configuration of `device` at `sample_rate` with a buffer of
/// `buffer` frames: an `f32` configuration at the rate exactly, with the most
/// channels (input is metered, every channel of it).
pub fn choose_input(
    device: &DeviceInfo,
    sample_rate: u32,
    buffer: u32,
) -> Result<Choice, AudioError> {
    choose_output(device, sample_rate, buffer, usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(channels: u16, min: u32, max: u32) -> ConfigRange {
        ConfigRange {
            channels,
            min_rate: min,
            max_rate: max,
            buffer: Some((14, 4096)),
            sample_format: "f32".to_owned(),
        }
    }

    fn device(configs: Vec<ConfigRange>) -> DeviceInfo {
        DeviceInfo {
            name: "Speakers".to_owned(),
            id: None,
            direction: Direction::Output,
            default: true,
            rate: None,
            configs,
        }
    }

    #[test]
    fn the_project_rate_and_the_sink_width() {
        let d = device(vec![
            range(2, 44_100, 44_100),
            range(2, 48_000, 48_000),
            range(4, 96_000, 96_000),
            range(2, 96_000, 96_000),
        ]);
        let choice = choose_output(&d, 96_000, 128, 2).unwrap();
        assert_eq!(
            choice,
            Choice {
                channels: 2,
                sample_rate: 96_000,
                buffer: 128,
                max_frames: 4096
            }
        );
        // A mono sink takes the narrowest wider configuration.
        assert_eq!(choose_output(&d, 96_000, 128, 1).unwrap().channels, 2);
    }

    #[test]
    fn a_rate_mismatch_lists_the_supported_rates() {
        let mut configs = vec![
            range(2, 48_000, 48_000),
            range(2, 44_100, 44_100),
            range(2, 8_000, 32_000),
        ];
        configs.push(ConfigRange {
            sample_format: "i16".to_owned(),
            ..range(2, 96_000, 96_000)
        });
        let error = choose_output(&device(configs), 96_000, 128, 2).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the device 'Speakers' cannot run at 96000 Hz; it supports 8000-32000, 44100, 48000 Hz. Signal does not resample: set sample_rate in project.toml to one of these"
        );
    }

    /// The incident of 2026-10-22: CoreAudio's bare "Unknown property"
    /// when the device ran at another rate than the project's.
    #[test]
    fn a_failed_start_says_which_rates() {
        assert_eq!(
            start_failure(
                "SSL 2+ Mk II",
                48_000,
                Some(44_100),
                Some(44_100),
                "CoreAudio",
                "Unknown property"
            ),
            "the device 'SSL 2+ Mk II' runs at 44100 Hz and did not start at 48000 Hz (CoreAudio: unknown property); play again to play at 44100 Hz"
        );
        assert_eq!(
            start_failure(
                "X",
                48_000,
                Some(48_000),
                Some(96_000),
                "CoreAudio",
                "Sample rate update timed out"
            ),
            "the device 'X' changed its sample rate to 96000 Hz while starting at 48000 Hz (CoreAudio: sample rate update timed out); play again to play at 96000 Hz"
        );
        // At the rate, or unread: the error alone, as a clause; a name keeps
        // its capitals.
        assert_eq!(
            start_failure(
                "X",
                48_000,
                None,
                None,
                "CoreAudio",
                "CoreAudio error 'who?'"
            ),
            "the device 'X' did not start at 48000 Hz (CoreAudio: CoreAudio error 'who?')"
        );
        assert_eq!(
            start_failure("X", 48_000, Some(48_000), Some(48_000), "CoreAudio", " "),
            "the device 'X' did not start at 48000 Hz (CoreAudio: no reason given)"
        );
    }

    /// ADR 0036: a run plays at the device's rate, with a notice when the
    /// project is written for another; a rate that moved is refused.
    #[test]
    fn the_device_rate_is_the_runs() {
        assert_eq!(
            rate_notice("SSL 2+ Mk II", "studio", 48_000, 44_100).as_deref(),
            Some(
                "'SSL 2+ Mk II' runs at 44100 Hz; studio is written for 48000 Hz and plays at 44100 Hz"
            )
        );
        assert_eq!(rate_notice("X", "kick", 48_000, 48_000), None);
        assert_eq!(
            rate_moved("X", 48_000, 44_100),
            "the device 'X' runs at 44100 Hz, not at the 48000 Hz this run was prepared for; Signal does not switch a device's rate: play again to play at 44100 Hz"
        );
    }

    #[test]
    fn a_stopped_stream_says_why() {
        assert_eq!(
            fault_message(
                "SSL 2+ Mk II",
                StreamFault::RateChanged,
                48_000,
                Some(44_100)
            ),
            "the device 'SSL 2+ Mk II' changed its sample rate to 44100 Hz while playing at 48000 Hz; play again to play at 44100 Hz"
        );
        assert_eq!(
            fault_message("X", StreamFault::RateChanged, 48_000, None),
            "the device 'X' changed its sample rate while playing at 48000 Hz"
        );
        assert_eq!(
            fault_message("X", StreamFault::Gone, 48_000, Some(48_000)),
            "the device 'X' went away while playing"
        );
    }

    #[test]
    fn a_buffer_outside_the_range_is_refused() {
        let error = choose_output(&device(vec![range(2, 48_000, 48_000)]), 48_000, 8, 2);
        assert_eq!(
            error,
            Err(AudioError::BufferSize {
                device: "Speakers".to_owned(),
                requested: 8,
                min: 14,
                max: 4096
            })
        );
    }

    #[test]
    fn a_device_without_float_output_is_refused() {
        let configs = vec![ConfigRange {
            sample_format: "i16".to_owned(),
            ..range(2, 48_000, 48_000)
        }];
        assert!(matches!(
            choose_output(&device(configs), 48_000, 128, 2),
            Err(AudioError::Format { .. })
        ));
    }
}
