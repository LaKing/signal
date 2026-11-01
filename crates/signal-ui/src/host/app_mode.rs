//! Signal.app's state (app mode): power, transport, and the `status` and
//! `app` messages.

use serde_json::{Value, json};

use crate::app;

use super::failure::{Failure, fail};

/// The refusal of an app message outside app mode.
pub(super) fn app_only() -> Failure {
    fail(
        "not-allowed",
        "only Signal.app's server opens projects and plays or stops them",
    )
}

/// The refusal of what needs the app switched on.
pub(super) fn powered_off() -> Failure {
    fail("powered-off", "Signal is switched off")
}

/// The power switch as `welcome`, `app` and `power` carry it.
pub(super) fn power_json(power: app::Power) -> Value {
    json!(if power.is_on() { "on" } else { "off" })
}
