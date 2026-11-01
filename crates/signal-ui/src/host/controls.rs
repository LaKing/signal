//! Control writes: a control's value written into the project's files, and
//! its corrections.

use signal_core::PortAddress;
use signal_graph::ExecutionPlan;

use crate::panel::Target;

/// The top-level instance and parameter of a control address `/<instance>.<parameter>`.
pub(super) fn control_of(port: &PortAddress) -> Option<(String, String)> {
    let instance = port.instance();
    let name = instance.as_str().strip_prefix('/')?;
    (!name.contains('/')).then(|| (name.to_owned(), port.port().to_owned()))
}

/// The base of plan parameter `port` as the plan was built (its files'
/// value, or its default).
pub(super) fn plan_base(plan: Option<&ExecutionPlan>, port: &PortAddress) -> Option<f64> {
    let instance = plan?
        .instances
        .iter()
        .find(|i| i.address == port.instance())?;
    let descriptor = instance
        .descriptor
        .parameters
        .iter()
        .find(|p| p.name == port.port())?;
    Some(
        instance
            .parameters
            .get(descriptor.name)
            .copied()
            .unwrap_or(descriptor.default),
    )
}

/// A file value as compared: its bits, and whether the file sets it.
pub(super) fn bits(base: Option<(f64, bool)>) -> Option<(u64, bool)> {
    base.map(|(value, in_file)| (value.to_bits(), in_file))
}

/// Controls written as one change ([`Host::write_controls`]).
pub(super) struct ControlWrite {
    pub(super) address: String,
    pub(super) instance: String,
    pub(super) parameter: String,
    pub(super) target: Target,
    pub(super) value: Option<f64>,
}

/// What [`Host::write_controls`] did: whether each value's live send was
/// taken, and a note on the running project.
pub(super) struct ControlsWritten {
    pub(super) sent: Vec<bool>,
    pub(super) note: Option<String>,
}
