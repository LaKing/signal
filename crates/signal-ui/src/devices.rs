//! The machine's sound system (ADR 0038 §4, §6, M4.1): where each role of
//! a project plays on this machine, and the writes that bind a role to
//! device channels. Control-thread code, outside the host's state: the
//! host (and Signal.app's [`crate::app::start`]) calls in.
//!
//! - **Where a binding lives:** the Setup's `[play.<role>]` (and
//!   `output_channels`, `main`'s short form) for every project ("all
//!   projects", scope `machine`), or the project folder's
//!   `devices.local.toml` for this project on this machine (scope
//!   `project`), merged over the Setup key by key. Neither is a project
//!   file: renders never read them and the plan never changes with them.
//! - **[`sound_system`]** resolves the merged bindings against the device
//!   inventory into a [`SoundSystem`] for `signal_output::route`: a role on
//!   the clock device, on another device (another clock, played through a
//!   clock adapter, ADR 0052), or on one that is absent.
//! - **[`route`] and [`unroute`]** are the protocol's `device.route` and
//!   `device.unroute` (`docs/ui-protocol.md`): they change the Setup in
//!   memory (the caller saves it and plays again) or write
//!   `devices.local.toml` (atomically, comments kept; the first write
//!   inside a git work tree appends `devices.local.toml` to the project
//!   folder's `.gitignore`, ADR 0038 §6).
//! - **Which folder:** a folder project's own; a `.signal` project's state
//!   folder outside it (`crate::local_state`, ADR 0038 §6's amendment of
//!   2026-11-01), which the host passes as `dir`.
//! - Routing `main` to another device moves the clock: `[output]` becomes
//!   that device (one clocked device in v1, ADR 0038 §8).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use signal_audio::{DeviceInfo, Direction, Resolution, labels, resolve};
use signal_output::{MAIN, Placement, RoleBinding, SoundSystem};
use signal_project::device::valid_role;

use crate::setup::{DeviceEntry, PlayEntry, Setup};

/// The machine file of a project folder (ADR 0038 §6): this project's
/// routing on this machine. Never a project file.
pub const LOCAL_FILE: &str = "devices.local.toml";

/// The line `.gitignore` gains (ADR 0038 §6).
const GITIGNORE_LINE: &str = "devices.local.toml";

/// `devices.local.toml` of the project folder `dir`, as read: its
/// `[play.<role>]` tables and the problems found (never fatal). A missing
/// file is empty.
pub fn read_local(dir: &Path) -> (BTreeMap<String, LocalPlay>, Vec<String>) {
    let path = dir.join(LOCAL_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (BTreeMap::new(), Vec::new());
        }
        Err(error) => return (BTreeMap::new(), vec![format!("{LOCAL_FILE}: {error}")]),
    };
    parse_local(&text)
}

/// One role's override in `devices.local.toml`: either key may be absent
/// (merged over the Setup's key by key).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalPlay {
    /// Its device channels, from 1.
    pub channels: Option<Vec<u16>>,
    /// Its device.
    pub device: Option<DeviceEntry>,
}

/// [`read_local`] of the text.
pub fn parse_local(text: &str) -> (BTreeMap<String, LocalPlay>, Vec<String>) {
    let mut found = BTreeMap::new();
    let mut problems = Vec::new();
    let table = match text.parse::<toml::Table>() {
        Ok(table) => table,
        Err(error) => {
            problems.push(format!(
                "{LOCAL_FILE}: not valid TOML ({})",
                error.to_string().trim()
            ));
            return (found, problems);
        }
    };
    let Some(play) = table.get("play") else {
        return (found, problems);
    };
    let Some(play) = play.as_table() else {
        problems.push(format!("{LOCAL_FILE}: play must be tables [play.<role>]"));
        return (found, problems);
    };
    for (role, entry) in play {
        let Some(entry) = entry.as_table().filter(|_| valid_role(role)) else {
            problems.push(format!(
                "{LOCAL_FILE}: play.{role} is not a role's table; ignored"
            ));
            continue;
        };
        let mut local = LocalPlay::default();
        for (key, value) in entry {
            match key.as_str() {
                "channels" => match value.clone().try_into::<Vec<u16>>() {
                    Ok(channels) if !channels.is_empty() && !channels.contains(&0) => {
                        local.channels = Some(channels);
                    }
                    _ => problems.push(format!(
                        "{LOCAL_FILE}: play.{role}.channels: device channels from 1; ignored"
                    )),
                },
                "device" => match value.clone().try_into::<DeviceEntry>() {
                    Ok(device) if !device.name.is_empty() => local.device = Some(device),
                    _ => problems.push(format!(
                        "{LOCAL_FILE}: play.{role}.device: a device table (name, id, signature); ignored"
                    )),
                },
                other => problems.push(format!(
                    "{LOCAL_FILE}: play.{role}: unknown key '{other}'; ignored"
                )),
            }
        }
        found.insert(role.clone(), local);
    }
    (found, problems)
}

/// The Setup with `devices.local.toml`'s overrides merged over its
/// `[play.<role>]` key by key (ADR 0038 §6: local, then the Setup, then the
/// defaults). An override of `main`'s channels replaces `output_channels`.
pub fn effective(setup: &Setup, local: &BTreeMap<String, LocalPlay>) -> Setup {
    let mut merged = setup.clone();
    for (role, over) in local {
        let base = merged.play.get(role).cloned().or_else(|| {
            (role == MAIN).then(|| PlayEntry {
                channels: setup.output_channels.clone().unwrap_or_default(),
                device: None,
            })
        });
        let entry = PlayEntry {
            channels: over
                .channels
                .clone()
                .or_else(|| base.as_ref().map(|b| b.channels.clone()))
                .unwrap_or_default(),
            device: over
                .device
                .clone()
                .or_else(|| base.as_ref().and_then(|b| b.device.clone())),
        };
        if entry.channels.is_empty() {
            continue;
        }
        merged.play.insert(role.clone(), entry);
    }
    // `main` plays on the clock device: a device named for it is the clock
    // (ADR 0038 §8, one clocked device), here as for the machine's routes.
    if let Some(device) = merged.play.get_mut(MAIN).and_then(|e| e.device.take()) {
        merged.output = Some(device);
    }
    merged
}

/// The Setup merged with the project folder `dir`'s `devices.local.toml`,
/// and what did not read in it.
pub fn effective_setup(setup: &Setup, dir: &Path) -> (Setup, Vec<String>) {
    let (local, mut problems) = read_local(dir);
    let merged = effective(setup, &local);
    for role in local.keys() {
        if !merged.play.contains_key(role) && !(role == MAIN && merged.output_channels.is_some()) {
            problems.push(format!(
                "{LOCAL_FILE}: play.{role} has no channels here or in the Setup; ignored"
            ));
        }
    }
    (merged, problems)
}

/// Whether the run must route several play sides (or a role the Setup
/// describes) rather than play its one sink as before.
pub fn routed(setup: &Setup, sides: usize) -> bool {
    sides > 1 || !setup.play.is_empty()
}

/// The clock device of `setup` in `devices`: the Setup's output, else the
/// system's default output.
pub fn clock_device<'a>(devices: &'a [DeviceInfo], setup: &Setup) -> Option<&'a DeviceInfo> {
    match &setup.output {
        Some(entry) => match resolve(devices, Direction::Output, &entry.to_ref()) {
            Resolution::Found(index) => devices.get(index),
            _ => None,
        },
        None => devices
            .iter()
            .find(|d| d.direction == Direction::Output && d.default),
    }
}

/// The sound system `setup` describes, its devices found in `devices`
/// (ADR 0038 §4, §8): `main` from `[play.main]`, else `output_channels`
/// (absent: channels 1 to its width, `signal_output::route`'s default);
/// every other role from `[play.<role>]`. A role naming no device, or the
/// clock device, plays on the clock device; another present output is
/// another clock, played through a clock adapter (ADR 0052); one not found
/// is absent.
pub fn sound_system(setup: &Setup, devices: &[DeviceInfo]) -> SoundSystem {
    let clock = clock_device(devices, setup);
    let mut roles = BTreeMap::new();
    if !setup.play.contains_key(MAIN)
        && let Some(channels) = &setup.output_channels
    {
        roles.insert(MAIN.to_owned(), RoleBinding::clock(channels.clone()));
    }
    for (role, entry) in &setup.play {
        let placement = match &entry.device {
            // `main` is on the clock device by definition ([`effective`]
            // makes a device named for it the clock).
            _ if role == MAIN => Placement::Clock,
            None => Placement::Clock,
            Some(device) => match resolve(devices, Direction::Output, &device.to_ref()) {
                Resolution::Found(index) => {
                    let found = devices.get(index);
                    let same = match (found, clock) {
                        (Some(a), Some(b)) => a.name == b.name && a.id == b.id,
                        _ => false,
                    };
                    if same {
                        Placement::Clock
                    } else {
                        // By its label, which tells same-named devices
                        // apart: two identical cards are two secondaries.
                        let label = labels(devices, Direction::Output)
                            .into_iter()
                            .find(|(i, _)| *i == index)
                            .map_or_else(|| device.name.clone(), |(_, label)| label);
                        Placement::Other(label)
                    }
                }
                Resolution::Missing { reason, .. } | Resolution::Ambiguous { reason, .. } => {
                    Placement::Missing(device.name.clone(), reason)
                }
            },
        };
        roles.insert(
            role.clone(),
            RoleBinding {
                placement,
                channels: entry.channels.clone(),
            },
        );
    }
    SoundSystem { roles }
}

/// `setup.get`'s `play` (additive): every role the Setup binds, `main`'s
/// short form included, `{role: {channels, device}}`.
pub fn play_json(setup: &Setup) -> Value {
    let mut roles = serde_json::Map::new();
    if let Some(channels) = &setup.output_channels
        && !setup.play.contains_key(MAIN)
    {
        roles.insert(
            MAIN.to_owned(),
            json!({"channels": channels, "device": Value::Null, "short": true}),
        );
    }
    for (role, entry) in &setup.play {
        roles.insert(
            role.clone(),
            json!({"channels": entry.channels, "device": entry.device}),
        );
    }
    Value::Object(roles)
}

/// `setup.get`'s `local` (additive): `devices.local.toml`'s overrides of
/// the project folder `dir`, `{play: {role: {channels, device}}, problems}`.
pub fn local_json(dir: &Path) -> Value {
    let (local, problems) = read_local(dir);
    let play: serde_json::Map<String, Value> = local
        .iter()
        .map(|(role, l)| {
            (
                role.clone(),
                json!({"channels": l.channels, "device": l.device}),
            )
        })
        .collect();
    json!({"file": LOCAL_FILE, "play": play, "problems": problems})
}

/// Where a binding is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The Setup: every project on this machine.
    Machine,
    /// `devices.local.toml`: this project on this machine.
    Project,
}

/// A refused route: an error code of `docs/ui-protocol.md` and the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// The code (`bad-message`, `bad-value`, `write-failed`).
    pub code: &'static str,
    /// Why.
    pub message: String,
}

fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused {
        code,
        message: message.into(),
    }
}

/// What a route or an unroute did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Routed {
    /// The role.
    pub role: String,
    /// Where it was written.
    pub scope: Scope,
    /// The Setup changed (the caller saves it and plays again).
    pub setup_changed: bool,
    /// `devices.local.toml` was written (the caller plays again).
    pub local_written: bool,
    /// What to tell (a `.gitignore` line written).
    pub notes: Vec<String>,
}

fn scope_of(message: &Value) -> Result<Scope, Refused> {
    match message.get("scope").and_then(Value::as_str) {
        None | Some("machine") => Ok(Scope::Machine),
        Some("project") => Ok(Scope::Project),
        Some(other) => Err(refused(
            "bad-value",
            format!("scope is \"machine\" or \"project\", not \"{other}\""),
        )),
    }
}

fn role_of(message: &Value) -> Result<String, Refused> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| refused("bad-message", "a route needs `role`"))?;
    if !valid_role(role) {
        return Err(refused(
            "bad-value",
            format!("`{role}` is not a role: [a-z][a-z0-9_]*"),
        ));
    }
    Ok(role.to_owned())
}

fn channels_of(message: &Value) -> Result<Vec<u16>, Refused> {
    let list = message
        .get("channels")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            refused(
                "bad-message",
                "a route needs `channels`, a list of device channels from 1",
            )
        })?;
    let channels = list
        .iter()
        .map(|c| {
            c.as_u64()
                .and_then(|c| u16::try_from(c).ok())
                .filter(|&c| c > 0)
                .ok_or_else(|| {
                    refused(
                        "bad-value",
                        format!("'{c}' is not a device channel (1, 2, …)"),
                    )
                })
        })
        .collect::<Result<Vec<u16>, Refused>>()?;
    if channels.is_empty() {
        return Err(refused("bad-value", "channels is empty"));
    }
    if (0..channels.len()).any(|i| {
        channels
            .iter()
            .skip(i + 1)
            .any(|c| channels.get(i) == Some(c))
    }) {
        return Err(refused("bad-value", "a device channel is listed twice"));
    }
    Ok(channels)
}

fn device_of(message: &Value) -> Result<Option<DeviceEntry>, Refused> {
    match message.get("device") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value::<DeviceEntry>(value.clone())
            .ok()
            .filter(|d| !d.name.is_empty())
            .map(Some)
            .ok_or_else(|| refused("bad-value", "device is {name, id, signature} or null")),
    }
}

/// `device.route` (ADR 0038 §10, `docs/ui-protocol.md`): binds `role` to
/// `channels` (from 1, in the play side's port order) of `device` (absent
/// or null: the clock device), for every project (`scope: "machine"`, the
/// default: the Setup, changed in memory) or this one (`"project"`:
/// `devices.local.toml` in `dir`, written now). `main` on the machine
/// scope writes its short form, `output_channels`; with a device other
/// than the clock device, that device becomes the Setup's output (the
/// clock).
pub fn route(message: &Value, setup: &mut Setup, dir: Option<&Path>) -> Result<Routed, Refused> {
    let role = role_of(message)?;
    let channels = channels_of(message)?;
    let device = device_of(message)?;
    let scope = scope_of(message)?;
    let mut notes = Vec::new();
    // ADR 0038 §4, §7: a capture role's input channels, the Setup's
    // `[capture.<role>]` (its device: the Setup's input without one).
    match message.get("side").and_then(Value::as_str) {
        None | Some("play") => {}
        Some("capture") => {
            if scope == Scope::Project {
                return Err(refused(
                    "not-built",
                    "a capture role is bound in the Setup (scope \"machine\"); devices.local.toml binds play roles only for now",
                ));
            }
            setup
                .capture
                .insert(role.clone(), PlayEntry { channels, device });
            return Ok(Routed {
                role,
                scope,
                setup_changed: true,
                local_written: false,
                notes,
            });
        }
        Some(other) => {
            return Err(refused(
                "bad-value",
                format!("side is \"play\" or \"capture\", not \"{other}\""),
            ));
        }
    }
    match scope {
        Scope::Machine => {
            if role == MAIN {
                if let Some(device) = device
                    && setup
                        .output
                        .as_ref()
                        .is_none_or(|o| !same_device(o, &device))
                {
                    setup.output = Some(device);
                }
                setup.output_channels = Some(channels);
                setup.play.remove(MAIN);
            } else {
                // A device equal to the clock device is written as none,
                // so the binding follows the clock.
                let device =
                    device.filter(|d| setup.output.as_ref().is_none_or(|o| !same_device(o, d)));
                setup
                    .play
                    .insert(role.clone(), PlayEntry { channels, device });
            }
            Ok(Routed {
                role,
                scope,
                setup_changed: true,
                local_written: false,
                notes,
            })
        }
        Scope::Project => {
            let dir = dir.ok_or_else(|| refused("no-project", "open a project folder first"))?;
            write_local(dir, &role, Some((&channels, device.as_ref())), &mut notes)?;
            Ok(Routed {
                role,
                scope,
                setup_changed: false,
                local_written: true,
                notes,
            })
        }
    }
}

/// `device.unroute`: removes `role`'s binding from the Setup (`main`: its
/// `output_channels`, so it plays on channels 1 to its width) or from
/// `devices.local.toml`.
pub fn unroute(message: &Value, setup: &mut Setup, dir: Option<&Path>) -> Result<Routed, Refused> {
    let role = role_of(message)?;
    let scope = scope_of(message)?;
    let mut notes = Vec::new();
    match scope {
        Scope::Machine => {
            let removed = setup.play.remove(&role).is_some();
            let short = role == MAIN && setup.output_channels.take().is_some();
            Ok(Routed {
                role,
                scope,
                setup_changed: removed || short,
                local_written: false,
                notes,
            })
        }
        Scope::Project => {
            let dir = dir.ok_or_else(|| refused("no-project", "open a project folder first"))?;
            write_local(dir, &role, None, &mut notes)?;
            Ok(Routed {
                role,
                scope,
                setup_changed: false,
                local_written: true,
                notes,
            })
        }
    }
}

fn same_device(a: &DeviceEntry, b: &DeviceEntry) -> bool {
    match (&a.id, &b.id) {
        (Some(x), Some(y)) => x == y,
        _ => a.name == b.name && a.signature == b.signature,
    }
}

/// Writes (or, with `None`, removes) `role`'s table in `dir`'s
/// `devices.local.toml`, keeping everything else (comments, other roles,
/// the plane's places); atomic. The first write inside a git work tree
/// appends the file to the project folder's `.gitignore`.
fn write_local(
    dir: &Path,
    role: &str,
    binding: Option<(&[u16], Option<&DeviceEntry>)>,
    notes: &mut Vec<String>,
) -> Result<(), Refused> {
    let path = dir.join(LOCAL_FILE);
    let failed = |e: String| refused("write-failed", format!("{LOCAL_FILE}: {e}"));
    let before = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(failed(error.to_string())),
    };
    if before.is_none() && binding.is_none() {
        return Ok(());
    }
    let mut doc = before
        .as_deref()
        .unwrap_or("")
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| failed(format!("not valid TOML ({})", e.to_string().trim())))?;
    let play = doc
        .entry("play")
        .or_insert_with(|| {
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            toml_edit::Item::Table(table)
        })
        .as_table_mut()
        .ok_or_else(|| failed("play is not a table".to_owned()))?;
    match binding {
        None => {
            play.remove(role);
        }
        Some((channels, device)) => {
            let mut table = toml_edit::Table::new();
            let mut list = toml_edit::Array::new();
            for &c in channels {
                list.push(i64::from(c));
            }
            table.insert("channels", toml_edit::value(list));
            if let Some(device) = device {
                let mut inline = toml_edit::InlineTable::new();
                inline.insert("name", device.name.as_str().into());
                if let Some(id) = &device.id {
                    inline.insert("id", id.as_str().into());
                }
                inline.insert("signature", device.signature.as_str().into());
                table.insert("device", toml_edit::value(inline));
            }
            play.insert(role, toml_edit::Item::Table(table));
        }
    }
    let text = doc.to_string();
    signal_project::transaction::write_atomic(&path, text.as_bytes())
        .map_err(|e| failed(e.to_string()))?;
    if before.is_none()
        && let Some(note) = ignore_in_git(dir).map_err(failed)?
    {
        notes.push(note);
    }
    Ok(())
}

/// ADR 0038 §6: inside a git work tree (a `.git` found upward from `dir`),
/// appends `devices.local.toml` to `dir`'s `.gitignore` unless that exact
/// line is there; the note to show when it did.
fn ignore_in_git(dir: &Path) -> Result<Option<String>, String> {
    // A `.signal` project's state folder (`crate::local_state`) is no
    // project folder: only a folder holding `project.toml` gains the line.
    if !dir.join(signal_project::PROJECT_FILE).is_file() || !inside_git(dir) {
        return Ok(None);
    }
    let path = dir.join(".gitignore");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!(".gitignore: {error}")),
    };
    if text.lines().any(|l| l.trim_end() == GITIGNORE_LINE) {
        return Ok(None);
    }
    let mut next = text;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(GITIGNORE_LINE);
    next.push('\n');
    signal_project::transaction::write_atomic(&path, next.as_bytes())
        .map_err(|e| format!(".gitignore: {e}"))?;
    Ok(Some(format!(
        "{LOCAL_FILE} holds this machine's routing; it was added to the project folder's .gitignore"
    )))
}

fn inside_git(dir: &Path) -> bool {
    let mut at: Option<PathBuf> = fs::canonicalize(dir).ok();
    while let Some(here) = at {
        if here.join(".git").exists() {
            return true;
        }
        at = here.parent().map(Path::to_path_buf);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use signal_audio::ConfigRange;

    fn output(name: &str, id: &str, channels: u16, default: bool) -> DeviceInfo {
        DeviceInfo {
            name: name.to_owned(),
            id: Some(id.to_owned()),
            direction: Direction::Output,
            default,
            rate: Some(48_000),
            configs: vec![ConfigRange {
                channels,
                min_rate: 44_100,
                max_rate: 192_000,
                buffer: Some((15, 4096)),
                sample_format: "f32".to_owned(),
            }],
        }
    }

    fn ssl() -> DeviceInfo {
        output("SSL 2+ MKII", "coreaudio:ssl", 4, false)
    }

    fn entry(device: &DeviceInfo) -> DeviceEntry {
        DeviceEntry {
            name: device.name.clone(),
            id: device.id.clone(),
            signature: String::new(),
        }
    }

    #[test]
    fn roles_on_the_clock_another_device_or_an_absent_one() {
        let speakers = output("MacBook Pro Speakers", "coreaudio:spk", 2, true);
        let devices = vec![speakers.clone(), ssl()];
        let mut setup = Setup {
            output: Some(entry(&ssl())),
            output_channels: Some(vec![1, 2]),
            ..Setup::default()
        };
        setup.play.insert(
            "cue".to_owned(),
            PlayEntry {
                channels: vec![3, 4],
                device: None,
            },
        );
        setup.play.insert(
            "alt".to_owned(),
            PlayEntry {
                channels: vec![1, 2],
                device: Some(entry(&speakers)),
            },
        );
        setup.play.insert(
            "here".to_owned(),
            PlayEntry {
                channels: vec![3, 4],
                device: Some(entry(&ssl())),
            },
        );
        setup.play.insert(
            "gone".to_owned(),
            PlayEntry {
                channels: vec![1, 2],
                device: Some(DeviceEntry {
                    name: "BlackHole".to_owned(),
                    id: Some("x".to_owned()),
                    signature: String::new(),
                }),
            },
        );
        let system = sound_system(&setup, &devices);
        assert_eq!(system.roles["main"], RoleBinding::clock(vec![1, 2]));
        assert_eq!(system.roles["cue"], RoleBinding::clock(vec![3, 4]));
        assert_eq!(system.roles["here"].placement, Placement::Clock);
        assert_eq!(
            system.roles["alt"].placement,
            Placement::Other("MacBook Pro Speakers".to_owned())
        );
        assert!(matches!(
            system.roles["gone"].placement,
            Placement::Missing(..)
        ));
    }

    /// ADR 0038 §7: `side: capture` binds a capture role in the Setup's
    /// `[capture.<role>]`, its device as given; the project scope is
    /// refused for it.
    #[test]
    fn a_capture_route_writes_the_setup_capture_table() {
        let mut setup = Setup {
            output: Some(entry(&ssl())),
            ..Setup::default()
        };
        let at2020 = json!({"name": "AT2020USB+", "id": "coreaudio:at", "signature": ""});
        let routed = route(
            &json!({"side": "capture", "role": "input", "channels": [1], "device": at2020}),
            &mut setup,
            None,
        )
        .unwrap();
        assert!(routed.setup_changed);
        assert_eq!(setup.capture["input"].channels, vec![1]);
        assert_eq!(
            setup.capture["input"]
                .device
                .as_ref()
                .map(|d| d.name.as_str()),
            Some("AT2020USB+")
        );
        assert!(setup.play.is_empty());
        let refused = route(
            &json!({"side": "capture", "role": "input", "channels": [1], "scope": "project"}),
            &mut setup,
            None,
        )
        .unwrap_err();
        assert_eq!(refused.code, "not-built");
        assert!(
            route(
                &json!({"side": "middle", "role": "x", "channels": [1]}),
                &mut setup,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn a_route_on_the_machine_writes_the_setup_main_by_its_short_form() {
        let mut setup = Setup {
            output: Some(entry(&ssl())),
            ..Setup::default()
        };
        let routed = route(
            &json!({"role": "cue", "channels": [3, 4]}),
            &mut setup,
            None,
        )
        .unwrap();
        assert!(routed.setup_changed);
        assert_eq!(
            setup.play["cue"],
            PlayEntry {
                channels: vec![3, 4],
                device: None
            }
        );
        // The clock device named is written as none.
        route(
            &json!({"role": "cue", "channels": [3, 4], "device": {"name": "SSL 2+ MKII", "id": "coreaudio:ssl", "signature": ""}}),
            &mut setup,
            None,
        )
        .unwrap();
        assert_eq!(setup.play["cue"].device, None);
        route(
            &json!({"role": "main", "channels": [1, 2]}),
            &mut setup,
            None,
        )
        .unwrap();
        assert_eq!(setup.output_channels, Some(vec![1, 2]));
        assert!(!setup.play.contains_key("main"));
        let text = setup.to_toml();
        assert!(text.contains("[play.cue]\nchannels = [3, 4]"), "{text}");
        let (read, problems) = Setup::parse(&text, Path::new("setup.toml"));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(read, setup);
        unroute(&json!({"role": "cue"}), &mut setup, None).unwrap();
        assert!(setup.play.is_empty());
        for bad in [
            json!({"role": "Cue", "channels": [3]}),
            json!({"role": "cue", "channels": []}),
            json!({"role": "cue", "channels": [0]}),
            json!({"role": "cue", "channels": [3, 3]}),
            json!({"role": "cue", "channels": [3], "scope": "galaxy"}),
            json!({"channels": [3]}),
        ] {
            assert!(route(&bad, &mut setup, None).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_route_for_this_project_writes_devices_local_toml_over_the_setup() {
        let dir = std::env::temp_dir().join(format!("signal-devices-local-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(LOCAL_FILE),
            "# mine\n[plane.\"id:x\"]\nat = [1, 2]\n",
        )
        .unwrap();
        let mut setup = Setup::default();
        setup.play.insert(
            "cue".to_owned(),
            PlayEntry {
                channels: vec![3, 4],
                device: None,
            },
        );
        let routed = route(
            &json!({"role": "cue", "channels": [4, 3], "scope": "project"}),
            &mut setup,
            Some(&dir),
        )
        .unwrap();
        assert!(routed.local_written && !routed.setup_changed);
        let text = fs::read_to_string(dir.join(LOCAL_FILE)).unwrap();
        assert!(
            text.starts_with("# mine\n[plane.\"id:x\"]\nat = [1, 2]\n"),
            "{text}"
        );
        assert!(text.contains("[play.cue]\nchannels = [4, 3]\n"), "{text}");
        let (merged, problems) = effective_setup(&setup, &dir);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(merged.play["cue"].channels, vec![4, 3]);
        // The Setup itself is untouched.
        assert_eq!(setup.play["cue"].channels, vec![3, 4]);
        unroute(
            &json!({"role": "cue", "scope": "project"}),
            &mut setup,
            Some(&dir),
        )
        .unwrap();
        let (merged, _) = effective_setup(&setup, &dir);
        assert_eq!(merged.play["cue"].channels, vec![3, 4]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_local_override_of_main_replaces_output_channels() {
        let setup = Setup {
            output_channels: Some(vec![1, 2]),
            ..Setup::default()
        };
        let (local, problems) =
            parse_local("[play.main]\nchannels = [3, 4]\n[play.Bad]\nchannels = [1]\n");
        assert_eq!(problems.len(), 1);
        let merged = effective(&setup, &local);
        let system = sound_system(&merged, &[]);
        assert_eq!(system.roles["main"].channels, vec![3, 4]);
        // A device named for main is the clock (one clocked device).
        let (local, _) =
            parse_local("[play.main]\ndevice = { name = \"Speakers\", id = \"coreaudio:spk\" }\n");
        let merged = effective(&setup, &local);
        assert_eq!(
            merged.output.as_ref().map(|d| d.name.as_str()),
            Some("Speakers")
        );
        assert_eq!(
            merged.play["main"],
            PlayEntry {
                channels: vec![1, 2],
                device: None
            }
        );
        assert_eq!(
            sound_system(&merged, &[]).roles["main"].placement,
            Placement::Clock
        );
    }

    #[test]
    fn the_gitignore_line_is_written_once_inside_a_work_tree_only() {
        let base = std::env::temp_dir().join(format!("signal-devices-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let project = base.join("song");
        fs::create_dir_all(base.join(".git")).unwrap();
        fs::create_dir_all(&project).unwrap();
        // A state folder outside any project (a `.signal` project's,
        // `crate::local_state`) inside the work tree gains no line.
        let state = base.join("Local/0123456789abcdef0123456789abcdef");
        fs::create_dir_all(&state).unwrap();
        let mut notes = Vec::new();
        write_local(&state, "cue", Some((&[3, 4], None)), &mut notes).unwrap();
        assert!(notes.is_empty() && !state.join(".gitignore").exists());
        fs::write(project.join("project.toml"), "").unwrap();
        write_local(&project, "cue", Some((&[3, 4], None)), &mut notes).unwrap();
        assert_eq!(notes.len(), 1);
        write_local(&project, "monitor", Some((&[3, 4], None)), &mut notes).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            fs::read_to_string(project.join(".gitignore")).unwrap(),
            "devices.local.toml\n"
        );
        let _ = fs::remove_dir_all(&base);
    }
}
