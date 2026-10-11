//! `signal devices`: the output and input devices with their supported sample
//! rates, channel counts and buffer-size ranges.

use signal_audio::{DeviceInfo, Direction, devices};

use crate::{Failure, print_stdout};

pub(crate) fn run(json: bool) -> Result<(), Failure> {
    let list = devices().map_err(|e| Failure::Device(e.to_string()))?;
    if json {
        let text =
            serde_json::to_string_pretty(&list).map_err(|e| Failure::Internal(e.to_string()))?;
        print_stdout(&text);
    } else {
        print_stdout(&describe(&list));
    }
    Ok(())
}

/// The list for people: one block per device, one line per configuration.
fn describe(list: &[DeviceInfo]) -> String {
    if list.is_empty() {
        return "no audio devices".to_owned();
    }
    let mut text = String::new();
    for device in list {
        let direction = match device.direction {
            Direction::Output => "output",
            Direction::Input => "input",
        };
        let default = if device.default { ", default" } else { "" };
        text.push_str(&format!("{} ({direction}{default})\n", device.name));
        for c in &device.configs {
            let rate = if c.min_rate == c.max_rate {
                format!("{} Hz", c.min_rate)
            } else {
                format!("{}-{} Hz", c.min_rate, c.max_rate)
            };
            let buffer = match c.buffer {
                Some((min, max)) => format!("buffer {min}-{max} frames"),
                None => "buffer range unknown".to_owned(),
            };
            text.push_str(&format!(
                "  {} channels, {rate}, {}, {buffer}\n",
                c.channels, c.sample_format
            ));
        }
    }
    text.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::describe;
    use signal_audio::{ConfigRange, DeviceInfo, Direction};

    #[test]
    fn devices_are_listed_with_rates_channels_and_buffer_ranges() {
        let list = [
            DeviceInfo {
                name: "Speakers".to_owned(),
                id: Some("coreaudio:speakers".to_owned()),
                direction: Direction::Output,
                default: true,
                configs: vec![ConfigRange {
                    channels: 2,
                    min_rate: 48_000,
                    max_rate: 48_000,
                    buffer: Some((14, 4096)),
                    sample_format: "f32".to_owned(),
                }],
            },
            DeviceInfo {
                name: "Mic".to_owned(),
                id: None,
                direction: Direction::Input,
                default: false,
                configs: vec![ConfigRange {
                    channels: 1,
                    min_rate: 8_000,
                    max_rate: 96_000,
                    buffer: None,
                    sample_format: "f32".to_owned(),
                }],
            },
        ];
        assert_eq!(
            describe(&list),
            "Speakers (output, default)\n  2 channels, 48000 Hz, f32, buffer 14-4096 frames\n\
             Mic (input)\n  1 channels, 8000-96000 Hz, f32, buffer range unknown"
        );
        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(json[0]["direction"], "output");
        assert_eq!(
            json[0]["configs"][0]["buffer"],
            serde_json::json!([14, 4096])
        );
    }
}
