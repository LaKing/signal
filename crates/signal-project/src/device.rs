//! Device slots (ADR 0038 §3, M4.1): an instance whose ports are declared
//! by its layout and whose content the machine supplies. The project names
//! a **layout** and a **role**, never hardware:
//!
//! ```toml
//! [instances]
//! cue = { device = { play = "stereo", role = "cue" } }
//! ```
//!
//! - `play`: a layout name (ADR 0038 §2). This version plans `stereo`,
//!   which is `std.audio_out` (ADR 0038 §3: "`std.audio_out` stays as it is
//!   and is the device slot `{ play = "stereo", role = "main" }`"), so a
//!   device slot's ports are `l`, `r` and `gain`, as every existing
//!   project's main output. The other layouts are reported as not built
//!   yet.
//! - `capture`: a layout name for the input side (ADR 0038 §7). This
//!   version plans `stereo` as `std.audio_in` (outputs `l`, `r`) and `mono`
//!   as `std.audio_in_mono` (output `m`), sources whose frames the driver
//!   writes; a slot with both `play` and `capture` (one box) is not built
//!   yet.
//! - `role`: what it is for, `[a-z][a-z0-9_]*`, default `main`.
//! - `kind`: `"audio"`, the default and the only kind built.
//! - Device slots are top-level instances of `project.toml`: a composite
//!   names none.
//!
//! The plan knows sinks, never roles (hard rule 1): [`play_sides`] hands
//! the role of each audio sink beside the plan to the output adapter
//! (`signal_output::route`). The same project text gives a byte-identical
//! plan on every machine, with any Setup.

use std::collections::BTreeMap;

use signal_core::{Direction, InstanceAddress, PortShape, SinkKind, SourceKind};
use signal_graph::ExecutionPlan;
use signal_output::{CaptureSide, MAIN, PlaySide};

use crate::Project;

/// The module a planned stereo play side is (ADR 0038 §3).
pub const STEREO_SINK: &str = "std.audio_out";

/// The layouts of ADR 0038 §2, by name, with their channels in canonical
/// order.
pub const LAYOUTS: [(&str, &[&str]); 5] = [
    ("mono", &["m"]),
    ("stereo", &["l", "r"]),
    ("quad", &["l", "r", "lb", "rb"]),
    ("5.1", &["l", "r", "c", "lfe", "ls", "rs"]),
    ("7.1", &["l", "r", "c", "lfe", "lb", "rb", "ls", "rs"]),
];

/// The layouts this version plans.
pub const BUILT_LAYOUTS: [&str; 1] = ["stereo"];

/// The module a planned stereo capture side is (ADR 0038 §7).
pub const STEREO_SOURCE: &str = "std.audio_in";

/// The module a planned mono capture side is.
pub const MONO_SOURCE: &str = "std.audio_in_mono";

/// A device slot as written: its role and its sides' layouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceSlot {
    /// Its role.
    pub role: String,
    /// Its play side's layout (`stereo`), if it plays.
    pub play: Option<String>,
    /// Its capture side's layout (`mono`, `stereo`), if it captures.
    pub capture: Option<String>,
}

/// Whether `role` is a role's name: `[a-z][a-z0-9_]*`.
pub fn valid_role(role: &str) -> bool {
    crate::valid_name(role)
}

/// The channel names of layout `name`, if it is one.
pub fn layout_channels(name: &str) -> Option<&'static [&'static str]> {
    LAYOUTS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// The layout a play side of `width` channels is shown as: `mono`,
/// `stereo`, …, else `<n> ch`.
pub fn layout_of_width(width: usize) -> String {
    LAYOUTS
        .iter()
        .find(|(_, c)| c.len() == width)
        .map_or_else(|| format!("{width} ch"), |(n, _)| (*n).to_owned())
}

/// The module a device slot's `play` side plans as, or why it does not.
pub(crate) fn planned_module(play: &str) -> Result<&'static str, String> {
    match play {
        "stereo" => Ok(STEREO_SINK),
        other if layout_channels(other).is_some() => Err(format!(
            "the layout `{other}` is not built yet; this version plays `stereo` device slots (ADR 0038, M4.1)"
        )),
        other => Err(format!(
            "`{other}` is not a layout (mono, stereo, quad, 5.1, 7.1)"
        )),
    }
}

/// The module a device slot's `capture` side plans as, or why it does not.
pub(crate) fn planned_source(capture: &str) -> Result<&'static str, String> {
    match capture {
        "stereo" => Ok(STEREO_SOURCE),
        "mono" => Ok(MONO_SOURCE),
        other if layout_channels(other).is_some() => Err(format!(
            "the capture layout `{other}` is not built yet; this version captures `mono` and `stereo` (ADR 0038)"
        )),
        other => Err(format!(
            "`{other}` is not a layout (mono, stereo, quad, 5.1, 7.1)"
        )),
    }
}

/// Every audio source of `plan` with its role (ADR 0038 §7): the device
/// slot's written role, `main` for any other. In plan order.
pub fn capture_sides(project: &Project, plan: &ExecutionPlan) -> Vec<CaptureSide> {
    let devices = project.device_slots();
    plan.instances
        .iter()
        .filter(|i| matches!(i.descriptor.source, Some(SourceKind::Audio)))
        .map(|i| {
            let role = top_level_name(&i.address)
                .and_then(|name| devices.get(name))
                .map_or_else(|| MAIN.to_owned(), |d| d.role.clone());
            let width = i
                .descriptor
                .ports
                .iter()
                .filter(|p| p.direction == Direction::Out && p.shape == PortShape::Signal)
                .count();
            CaptureSide {
                address: i.address.clone(),
                role,
                width,
            }
        })
        .collect()
}

/// Every audio sink of `plan` with its role (ADR 0038 §3): a device slot's
/// written role, `main` for any other (`std.audio_out`). In plan order;
/// `signal_output::route` puts them in canonical order.
pub fn play_sides(project: &Project, plan: &ExecutionPlan) -> Vec<PlaySide> {
    let devices = project.device_slots();
    plan.instances
        .iter()
        .filter(|i| matches!(i.descriptor.sink, Some(SinkKind::Audio)))
        .map(|i| {
            let role = top_level_name(&i.address)
                .and_then(|name| devices.get(name))
                .map_or_else(|| MAIN.to_owned(), |d| d.role.clone());
            let width = i
                .descriptor
                .ports
                .iter()
                .filter(|p| p.direction == Direction::In && p.shape == PortShape::Signal)
                .count();
            PlaySide {
                address: i.address.clone(),
                role,
                width,
            }
        })
        .collect()
}

/// The main play side: the side of role `main`, else the first in canonical
/// order (ADR 0038 §4's exception), if the plan has any.
pub fn main_side(sides: &[PlaySide]) -> Option<&PlaySide> {
    sides.iter().find(|s| s.role == MAIN).or_else(|| {
        sides
            .iter()
            .min_by(|a, b| (&a.role, &a.address).cmp(&(&b.role, &b.address)))
    })
}

/// The name of a top-level instance address (`/cue` → `cue`), none for a
/// deeper one.
fn top_level_name(address: &InstanceAddress) -> Option<&str> {
    let name = address.as_str().strip_prefix('/')?;
    (!name.contains('/')).then_some(name)
}

/// The roles played by more than one sink: each later sink, by address,
/// with the role (ADR 0038 §5.4: versions of a role are not built, and two
/// of one layout are always an error).
pub(crate) fn repeated_roles(sides: &[PlaySide]) -> Vec<(InstanceAddress, String)> {
    let mut first: BTreeMap<&str, &InstanceAddress> = BTreeMap::new();
    let mut ordered: Vec<&PlaySide> = sides.iter().collect();
    ordered.sort_by(|a, b| (&a.role, &a.address).cmp(&(&b.role, &b.address)));
    let mut repeated = Vec::new();
    for side in ordered {
        if first.insert(side.role.as_str(), &side.address).is_some() {
            repeated.push((side.address.clone(), side.role.clone()));
        }
    }
    repeated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_name_their_channels_in_canonical_order() {
        assert_eq!(layout_channels("stereo"), Some(&["l", "r"][..]));
        assert_eq!(layout_channels("5.1").map(<[_]>::len), Some(6));
        assert_eq!(layout_of_width(2), "stereo");
        assert_eq!(layout_of_width(3), "3 ch");
        assert_eq!(planned_module("stereo"), Ok(STEREO_SINK));
        assert!(planned_module("5.1").unwrap_err().contains("not built yet"));
        assert!(planned_module("hexa").unwrap_err().contains("not a layout"));
        assert_eq!(planned_source("stereo"), Ok(STEREO_SOURCE));
        assert_eq!(planned_source("mono"), Ok(MONO_SOURCE));
        assert!(planned_source("5.1").unwrap_err().contains("not built yet"));
    }
}
