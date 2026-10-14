//! `signal devices`: the output and input devices with their supported sample
//! rates, channel counts and buffer-size ranges; with `--compute` (M2.7-2)
//! the compute devices: the CPU's performance levels and, in a `--features
//! gpu` build, every GPU adapter. The JSON is documented in
//! `docs/benchmarks.md`, "`signal devices --compute`".

use clap::Args;
use serde_json::{Value, json};
use signal_audio::{DeviceInfo, Direction, devices};
use signal_engine::{cpu_levels, cpu_lines, performance_cores};

use crate::{Failure, print_stdout};

/// Arguments of `signal devices`.
#[derive(Args)]
pub(crate) struct DevicesArgs {
    /// List the compute devices instead: CPU performance levels and GPU
    /// adapters (the adapters in a `--features gpu` build only).
    #[arg(long)]
    compute: bool,
}

/// Why the default build lists no GPU adapter.
#[cfg(not(feature = "gpu"))]
const NOT_BUILT: &str = "this build has no GPU support (AC2b): build signal with --features gpu";

pub(crate) fn run(args: &DevicesArgs, json: bool) -> Result<(), Failure> {
    if args.compute {
        if json {
            let text = serde_json::to_string_pretty(&compute_json())
                .map_err(|e| Failure::Internal(e.to_string()))?;
            print_stdout(&text);
        } else {
            print_stdout(&compute_lines().join("\n"));
        }
        return Ok(());
    }
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

/// The compute devices for people (the lines Signal.app's interface setup
/// shows too).
fn compute_lines() -> Vec<String> {
    let (levels, source) = cpu_levels();
    let mut lines = cpu_lines(&levels, source);
    #[cfg(feature = "gpu")]
    lines.extend(signal_gpu::inventory().lines());
    #[cfg(not(feature = "gpu"))]
    lines.extend(["GPU:".to_owned(), format!("  not listed: {NOT_BUILT}")]);
    lines
}

/// `signal devices --compute --json`.
fn compute_json() -> Value {
    let (levels, source) = cpu_levels();
    let levels: Vec<Value> = levels
        .iter()
        .map(|l| {
            json!({
                "index": l.index,
                "name": l.name,
                "physical": l.physical,
                "logical": l.logical,
            })
        })
        .collect();
    let cpu = json!({
        "source": source.name(),
        "performance_cores": performance_cores().0,
        "levels": levels,
    });
    json!({ "cpu": cpu, "gpu": gpu_json() })
}

#[cfg(feature = "gpu")]
fn gpu_json() -> Value {
    let inventory = signal_gpu::inventory();
    let adapters: Vec<Value> = inventory
        .adapters
        .iter()
        .map(|a| {
            let l = &a.limits;
            let (x, y, z) = l.max_compute_workgroup_size;
            json!({
                "id": a.id,
                "index": a.index,
                "name": a.name,
                "backend": a.backend,
                "type": a.device_type,
                "vendor": a.vendor,
                "device_id": a.device_id,
                "driver": a.driver,
                "driver_info": a.driver_info,
                "limits": {
                    "max_compute_workgroups_per_dimension": l.max_compute_workgroups_per_dimension,
                    "max_compute_invocations_per_workgroup": l.max_compute_invocations_per_workgroup,
                    "max_compute_workgroup_size": [x, y, z],
                    "max_compute_workgroup_storage_size": l.max_compute_workgroup_storage_size,
                    "max_storage_buffer_binding_size": l.max_storage_buffer_binding_size,
                    "max_buffer_size": l.max_buffer_size,
                },
            })
        })
        .collect();
    json!({
        "built": true,
        "available": !adapters.is_empty(),
        "reason": inventory.error,
        "adapters": adapters,
    })
}

#[cfg(not(feature = "gpu"))]
fn gpu_json() -> Value {
    json!({ "built": false, "available": false, "reason": NOT_BUILT, "adapters": [] })
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
