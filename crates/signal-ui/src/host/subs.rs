//! Subscriptions: the parameter, meter, scope and vector streams and their
//! rates.

use std::time::Duration;

use serde_json::Value;
use signal_core::PortAddress;
use signal_graph::ExecutionPlan;

use super::failure::{Failure, fail};

/// The fastest parameter and meter streams.
const MAX_RATE_HZ: f64 = 60.0;

/// A message's `rate`, `default` without one: a number of at least 1.
pub(super) fn rate_of(message: &Value, default: f64) -> Result<f64, Failure> {
    let rate = match message.get("rate") {
        None => default,
        Some(rate) => rate.as_f64().unwrap_or(f64::NAN),
    };
    if rate.is_finite() && rate >= 1.0 {
        Ok(rate)
    } else {
        Err(fail("bad-rate", "rate must be a number of at least 1"))
    }
}

/// The interval of a parameter or meter stream: its `rate` (default 30)
/// from 1 to 60 per second, faster ones taken as 60.
pub(super) fn rate_interval(message: &Value) -> Result<Duration, Failure> {
    let rate = rate_of(message, 30.0)?.min(MAX_RATE_HZ);
    Duration::try_from_secs_f64(1.0 / rate).map_err(|e| fail("bad-rate", e.to_string()))
}

/// A scope's decimation at `sample_rate`: its `rate` in points per second
/// (default and at most the sample rate; at least 1).
pub(super) fn scope_decimation(message: &Value, sample_rate: u32) -> Result<u64, Failure> {
    let sample_rate = f64::from(sample_rate.max(1));
    let rate = rate_of(message, sample_rate)?.min(sample_rate);
    // From 1 to the sample rate, so this is a finite number of frames.
    Ok((sample_rate / rate).round().max(1.0) as u64)
}

/// A vector stream's decimation: its `rate` in snapshots per second
/// (default 60; at least 1, at most the port's tick rate), as a whole number
/// of the port's ticks, so every snapshot is a tick.
pub(super) fn vector_decimation(rate: f64, sample_rate: u32, divisor: u64) -> u64 {
    let divisor = divisor.max(1);
    let ticks = f64::from(sample_rate.max(1)) / divisor as f64;
    let rate = rate.min(ticks.max(1.0));
    divisor * (ticks / rate).round().max(1.0) as u64
}

/// Vector output `port` of `plan`: its elements and rate divisor
/// ([`signal_output::vector_shape`]); `None` without a plan.
pub(super) fn vector_shape(
    plan: Option<&ExecutionPlan>,
    port: &PortAddress,
) -> Option<(usize, u64)> {
    signal_output::vector_shape(plan?, port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::within;
    use serde_json::json;

    /// auditor-1-rate-panic: no client rate panics; the unsupported ones are
    /// refused with `bad-rate`.
    #[test]
    fn rates_are_checked_without_panicking() {
        fn refused<T>(result: Result<T, Failure>) -> bool {
            matches!(
                result,
                Err(Failure {
                    code: "bad-rate",
                    ..
                })
            )
        }
        for rate in [
            json!(1e-20),
            json!(1e-320),
            json!(0),
            json!(-1),
            json!(0.5),
            json!(f64::NAN),
            json!("NaN"),
            json!(null),
        ] {
            let message = json!({"rate": rate});
            assert!(refused(rate_interval(&message)), "{message}");
            assert!(refused(scope_decimation(&message, 48_000)), "{message}");
        }
        let huge = json!({"rate": 1e300});
        assert_eq!(
            rate_interval(&huge).ok(),
            Some(Duration::from_secs_f64(1.0 / 60.0))
        );
        assert_eq!(scope_decimation(&huge, 48_000).ok(), Some(1));
        let slow = json!({"rate": 1});
        assert_eq!(rate_interval(&slow).ok(), Some(Duration::from_secs(1)));
        assert_eq!(scope_decimation(&slow, 48_000).ok(), Some(48_000));
        assert_eq!(
            rate_interval(&json!({})).ok(),
            Some(Duration::from_secs_f64(1.0 / 30.0))
        );
        assert_eq!(scope_decimation(&json!({}), 48_000).ok(), Some(1));
        // A range whose ends cross (an export's targets) clamps, never panics.
        assert_eq!(within(5.0, 3.0, 1.0), 1.0);
        assert_eq!(within(-5.0, 0.0, 1.0), 0.0);
        assert_eq!(
            scope_decimation(&json!({"rate": 4800}), 48_000).ok(),
            Some(10)
        );
    }
}
