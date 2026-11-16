//! Device slots (ADR 0038 §3, M4.1): an instance whose ports are declared
//! by its layout and whose content the machine supplies. The project names
//! a **layout** and a **role**, never hardware:
//!
//! ```toml
//! [instances]
//! cue = { device = { play = "stereo", role = "cue" } }
//! ```
//!
//! - `play`: a layout name (ADR 0038 §2). `stereo` plans as
//!   `std.audio_out` (ADR 0038 §3: "`std.audio_out` stays as it is and is
//!   the device slot `{ play = "stereo", role = "main" }`"), so its ports
//!   are `l`, `r` and `gain`, as every existing project's main output;
//!   `mono`, `quad`, `5.1` and `7.1` plan as `std.audio_out_mono`,
//!   `std.audio_out_quad`, `std.audio_out_5_1` and `std.audio_out_7_1`,
//!   their inputs the layout's channels in order. A discrete layout (a
//!   table of ports) is not built yet.
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
use signal_graph::{
    CompositeDef, ExecutionPlan, Exports, InputExport, InstanceDef, OutputExport, ParameterExport,
};
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

/// The layouts this version plans, with the module each plays as.
pub const BUILT_LAYOUTS: [(&str, &str); 5] = [
    ("mono", "std.audio_out_mono"),
    ("stereo", STEREO_SINK),
    ("quad", "std.audio_out_quad"),
    ("5.1", "std.audio_out_5_1"),
    ("7.1", "std.audio_out_7_1"),
];

/// Each play side's sink with its tap, the module it plans as inside
/// another project (ADR 0035 §3, ADR 0038 §7): `signal-std` registers the
/// same pairs (`Registry::sink_tap`), which a test checks.
pub const SINK_TAPS: [(&str, &str); 5] = [
    ("std.audio_out_mono", "std.audio_tap_mono"),
    (STEREO_SINK, "std.audio_tap"),
    ("std.audio_out_quad", "std.audio_tap_quad"),
    ("std.audio_out_5_1", "std.audio_tap_5_1"),
    ("std.audio_out_7_1", "std.audio_tap_7_1"),
];

/// The tap of play side sink `module`, if it is one.
pub fn tap_of_sink(module: &str) -> Option<&'static str> {
    SINK_TAPS
        .iter()
        .find(|(sink, _)| *sink == module)
        .map(|(_, tap)| *tap)
}

/// Whether `module` is a play side's sink (`std.audio_out` or another
/// layout's, ADR 0038 §2).
pub fn is_play_sink(module: &str) -> bool {
    BUILT_LAYOUTS.iter().any(|(_, m)| *m == module)
}

/// The layout a play side's sink `module` plays, if it is one.
pub fn layout_of_sink(module: &str) -> Option<&'static str> {
    BUILT_LAYOUTS
        .iter()
        .find(|(_, m)| *m == module)
        .map(|(layout, _)| *layout)
}

/// The module a planned stereo capture side is (ADR 0038 §7).
pub const STEREO_SOURCE: &str = "std.audio_in";

/// The module a planned mono capture side is.
pub const MONO_SOURCE: &str = "std.audio_in_mono";

/// A discrete play side's layout name (ADR 0038 §2): its ports are named
/// in [`DeviceSlot::ports`].
pub const DISCRETE: &str = "discrete";

/// The most channels a discrete layout has (ADR 0038 §2).
pub const MAX_DISCRETE: usize = 64;

/// The kind of a DMX (Art-Net) device slot (ADR 0057 §2).
pub const DMX: &str = "dmx";

/// The kind of an OSC control device slot (ADR 0057 §1).
pub const OSC: &str = "osc";

/// The kind of a MIDI control device slot (ADR 0057 §1, §3).
pub const MIDI: &str = "midi";

/// The instance inside a control slot's composite that is its source: the
/// slot `/surface` captures at `/surface/capture`.
pub const DISCRETE_SOURCE: &str = "capture";

/// A device slot as written: its role and its sides' layouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceSlot {
    /// Its kind: `audio`, or `dmx` (ADR 0057).
    pub kind: String,
    /// Its role.
    pub role: String,
    /// Its play side's layout (`stereo`, or [`DISCRETE`]), if it plays.
    pub play: Option<String>,
    /// A discrete side's ports (play or capture), in channel order; empty
    /// otherwise.
    pub ports: Vec<String>,
    /// Its capture side's layout (`mono`, `stereo`, or [`DISCRETE`]), if it
    /// captures.
    pub capture: Option<String>,
    /// ADR 0038 §3, §9: its capture side's WAV file, relative to the
    /// project folder, feeding it in offline renders.
    pub render: Option<String>,
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
    BUILT_LAYOUTS
        .iter()
        .find(|(layout, _)| *layout == play)
        .map(|(_, module)| *module)
        .ok_or_else(|| format!("`{play}` is not a layout (mono, stereo, quad, 5.1, 7.1)"))
}

/// The composite a discrete device slot `instance` plans as (ADR 0038 §3):
/// its written ports exported onto `std.device_out<N>`'s `c1…cN`, with
/// `gain`; a capture side's (`capture` true) exported out of
/// `std.device_in<N>`'s. Named `device:<instance>`, a name no file can give.
pub(crate) fn discrete_composite(
    instance: &str,
    ports: &[String],
    kind: &str,
    capture: bool,
) -> CompositeDef {
    // ADR 0057 §1: a control slot is a source, its values exported out;
    // ADR 0038 §7: so is a discrete audio capture side.
    if kind == OSC || kind == MIDI || capture {
        let source = if capture && kind != OSC && kind != MIDI {
            "std.device_in"
        } else {
            "std.control_in"
        };
        return CompositeDef {
            name: discrete_name(instance),
            instances: vec![InstanceDef {
                name: DISCRETE_SOURCE.to_owned(),
                module: format!("{source}{}", ports.len()),
                ..InstanceDef::default()
            }],
            exports: Exports {
                outputs: ports
                    .iter()
                    .enumerate()
                    .map(|(k, port)| OutputExport {
                        name: port.clone(),
                        from: format!("{DISCRETE_SOURCE}.c{}", k + 1),
                    })
                    .collect(),
                ..Exports::default()
            },
            ..CompositeDef::default()
        };
    }
    // ADR 0057 §2: a data slot's sink takes no gain.
    let data = kind == DMX;
    let module = if data {
        "std.data_out"
    } else {
        "std.device_out"
    };
    CompositeDef {
        name: discrete_name(instance),
        instances: vec![InstanceDef {
            name: DISCRETE_SINK.to_owned(),
            module: format!("{module}{}", ports.len()),
            ..InstanceDef::default()
        }],
        exports: Exports {
            inputs: ports
                .iter()
                .enumerate()
                .map(|(k, port)| InputExport {
                    name: port.clone(),
                    to: vec![format!("{DISCRETE_SINK}.c{}", k + 1)],
                })
                .collect(),
            parameters: if data {
                Vec::new()
            } else {
                vec![ParameterExport {
                    name: "gain".to_owned(),
                    to: vec![format!("{DISCRETE_SINK}.gain")],
                    ..ParameterExport::default()
                }]
            },
            ..Exports::default()
        },
        ..CompositeDef::default()
    }
}

/// The name of discrete device slot `instance`'s composite.
pub fn discrete_name(instance: &str) -> String {
    format!("device:{instance}")
}

/// The instance inside a discrete slot's composite that is its sink: the
/// slot `/drums` plays at `/drums/play`.
pub const DISCRETE_SINK: &str = "play";

/// Whether `module` names a discrete device slot's composite (also
/// qualified, inside another project: `<id>/device:<instance>`).
pub fn is_discrete_composite(module: &str) -> bool {
    module
        .rsplit('/')
        .next()
        .is_some_and(|m| m.starts_with("device:"))
}

/// Why discrete ports `ports` are not a layout, if they are not: 1 to 64
/// names, each `[a-z][a-z0-9_]*`, distinct, none `gain`.
pub(crate) fn discrete_problem(ports: &[String]) -> Option<String> {
    if ports.is_empty() || ports.len() > MAX_DISCRETE {
        return Some(format!(
            "a discrete layout has 1 to {MAX_DISCRETE} ports, not {}",
            ports.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for port in ports {
        if !crate::valid_name(port) {
            return Some(format!(
                "invalid port name `{port}`; a port is [a-z][a-z0-9_]*"
            ));
        }
        if port == "gain" {
            return Some("`gain` is the slot's parameter, not a port name".to_owned());
        }
        if !seen.insert(port.as_str()) {
            return Some(format!("port `{port}` is listed twice"));
        }
    }
    None
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
            let role = slot_of_sink(&i.address)
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
            let slot = slot_of_sink(&i.address).and_then(|name| devices.get(name));
            let role = slot.map_or_else(|| MAIN.to_owned(), |d| d.role.clone());
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
                ports: slot.map(|d| d.ports.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// Every data sink of `plan` (ADR 0057 §2: a `dmx` slot's) with its slot's
/// role and port names. In plan order.
pub fn data_sides(project: &Project, plan: &ExecutionPlan) -> Vec<signal_output::DataSide> {
    let devices = project.device_slots();
    plan.instances
        .iter()
        .filter(|i| matches!(i.descriptor.sink, Some(SinkKind::Data)))
        .map(|i| {
            let slot = slot_of_sink(&i.address).and_then(|name| devices.get(name));
            signal_output::DataSide {
                address: i.address.clone(),
                slot: slot_of_sink(&i.address).unwrap_or_default().to_owned(),
                role: slot.map_or_else(String::new, |d| d.role.clone()),
                kind: slot.map_or_else(String::new, |d| d.kind.clone()),
                ports: slot.map(|d| d.ports.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// Every control source of `plan` (ADR 0057 §1: an `osc` slot's) with its
/// slot's role and value names. In plan order.
pub fn control_sides(project: &Project, plan: &ExecutionPlan) -> Vec<signal_output::DataSide> {
    let devices = project.device_slots();
    plan.instances
        .iter()
        .filter(|i| matches!(i.descriptor.source, Some(SourceKind::Data)))
        .map(|i| {
            let slot = slot_of_sink(&i.address).and_then(|name| devices.get(name));
            signal_output::DataSide {
                address: i.address.clone(),
                slot: slot_of_sink(&i.address).unwrap_or_default().to_owned(),
                role: slot.map_or_else(String::new, |d| d.role.clone()),
                kind: slot.map_or_else(String::new, |d| d.kind.clone()),
                ports: slot.map(|d| d.ports.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// The main play side: the side of role `main`, else of the first role in
/// canonical order (ADR 0038 §4's exception); of a role with several
/// versions (§5.4), the largest, the first by address among equals. What a
/// session records, whatever the machine plays.
pub fn main_side(sides: &[PlaySide]) -> Option<&PlaySide> {
    let role = if sides.iter().any(|s| s.role == MAIN) {
        MAIN
    } else {
        sides.iter().map(|s| s.role.as_str()).min()?
    };
    sides.iter().filter(|s| s.role == role).min_by(|a, b| {
        b.width
            .cmp(&a.width)
            .then_with(|| a.address.cmp(&b.address))
    })
}

/// The device slot a sink at `address` plays for: a top-level instance
/// (`/cue` → `cue`), or a discrete slot's sink (`/drums/play` → `drums`).
pub fn slot_of_sink(address: &InstanceAddress) -> Option<&str> {
    top_level_name(address).or_else(|| {
        let (slot, sink) = address.as_str().strip_prefix('/')?.split_once('/')?;
        (sink == DISCRETE_SINK || sink == DISCRETE_SOURCE).then_some(slot)
    })
}

/// The name of a top-level instance address (`/cue` → `cue`), none for a
/// deeper one.
fn top_level_name(address: &InstanceAddress) -> Option<&str> {
    let name = address.as_str().strip_prefix('/')?;
    (!name.contains('/')).then_some(name)
}

/// A play side's layout as shown: its width's, or `discrete` (ADR 0038 §2).
pub fn side_layout(side: &PlaySide) -> String {
    if side.ports.is_empty() {
        layout_of_width(side.width)
    } else {
        DISCRETE.to_owned()
    }
}

/// The versions of a role that repeat a layout: each later sink, by
/// address, with the role and the layout (ADR 0038 §5.4: a role may have
/// several versions, one per layout; two of one layout are an error).
pub(crate) fn repeated_layouts(sides: &[PlaySide]) -> Vec<(InstanceAddress, String, String)> {
    let mut first: BTreeMap<(&str, usize, bool), &InstanceAddress> = BTreeMap::new();
    let mut ordered: Vec<&PlaySide> = sides.iter().collect();
    ordered.sort_by(|a, b| (&a.role, &a.address).cmp(&(&b.role, &b.address)));
    let mut repeated = Vec::new();
    for side in ordered {
        if first
            .insert(
                (side.role.as_str(), side.width, !side.ports.is_empty()),
                &side.address,
            )
            .is_some()
        {
            repeated.push((side.address.clone(), side.role.clone(), side_layout(side)));
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
        assert_eq!(planned_module("5.1"), Ok("std.audio_out_5_1"));
        assert_eq!(layout_of_sink("std.audio_out_mono"), Some("mono"));
        assert!(is_play_sink("std.audio_out") && !is_play_sink("std.audio_tap"));
        assert!(planned_module("hexa").unwrap_err().contains("not a layout"));
        assert_eq!(planned_source("stereo"), Ok(STEREO_SOURCE));
        assert_eq!(planned_source("mono"), Ok(MONO_SOURCE));
        assert!(planned_source("5.1").unwrap_err().contains("not built yet"));
    }
}
