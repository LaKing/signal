//! The Setup and the keyboard shortcuts: devices, checks and Apply (ADR
//! 0034, 0036).

use serde_json::Value;

use crate::player::Output;

use super::failure::{Failure, fail};

/// `buffer` of a message (1 to 8192 frames), else `default`.
pub(super) fn buffer_of(message: &Value, default: u32) -> Result<u32, Failure> {
    match message.get("buffer") {
        None => Ok(default),
        // ADR 0036: the device's own buffer (`setup.apply`).
        Some(Value::String(device)) if device == "device" => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|b| (1..=8192).contains(b))
            .map(|b| b as u32)
            .ok_or_else(|| fail("bad-value", "buffer is 1 to 8192 frames")),
    }
}

/// The output device's name, for `setup.get`'s run.
pub(super) fn device_name(output: &Output) -> String {
    match output {
        Output::Device(driver) => driver.name().to_owned(),
        Output::Null(..) => "null".to_owned(),
    }
}
