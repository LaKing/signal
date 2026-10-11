//! Devices as plain data, and the choice of an output configuration: the
//! project's sample rate exactly (Signal does not resample) and the requested
//! buffer.

use std::fmt;

use serde::Serialize;

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
    /// Output or input.
    pub direction: Direction,
    /// Whether it is the system's default device of its direction.
    pub default: bool,
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
    /// No output device of that name (or no default output device).
    NoDevice {
        /// The name asked for, if any.
        name: Option<String>,
        /// The output devices there are.
        available: Vec<String>,
    },
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
            AudioError::NoDevice { name, available } => {
                match name {
                    Some(name) => write!(f, "no output device is named '{name}'")?,
                    None => f.write_str("there is no default output device")?,
                }
                if available.is_empty() {
                    f.write_str("; there are no output devices")
                } else {
                    write!(f, "; output devices: {}", available.join(", "))
                }
            }
            AudioError::Format { device } => {
                write!(f, "the device '{device}' offers no 32-bit float output")
            }
            AudioError::Host(message) => write!(f, "the audio system: {message}"),
        }
    }
}

impl std::error::Error for AudioError {}

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
            direction: Direction::Output,
            default: true,
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
