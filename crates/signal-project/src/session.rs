//! Session directories (`docs/session-format.md`): the initial project, every
//! plan version swapped in, and every command with its effective frame, as
//! text. [`SessionRecorder`] writes one; [`LiveRecorder`] feeds it from a
//! real-time [`Controller`](signal_output::Controller), with the outcome of
//! every latent module's due block (M2.7-4); [`Session::load`] reads and
//! validates one; [`replay`] renders it offline through the same engine API a
//! live driver uses (ADR 0006, V10), exactly as it was played, and
//! [`replay_with`] can render it ideally instead. Control-thread code: it
//! allocates and does I/O.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use signal_core::{
    Curve, Direction, EventPayload, Frame, InstanceAddress, LossCause, ModuleDescriptor,
    PortAddress, PortShape, Registry, SinkKind, fnv1a64, fnv1a64_words,
};
use signal_engine::{
    BoundaryLine, BoundaryOutcome, ControlSink, Engine, EngineConfig, EngineError, ExternalEvent,
    InputDiscards, Outcome, Provenance, Report, RunnerStatus, Scripted, SentCommand,
};
use signal_graph::ExecutionPlan;
use signal_output::{
    Conversion, Render, RenderError, VectorTap, VectorTrack, render_blocks_tapped,
};

use crate::{Diagnostic, LoadError, Project, Severity, SourceFile};

/// The session format version this build reads and writes.
pub const FORMAT_VERSION: i64 = 1;

/// The session file in a session directory.
pub const SESSION_FILE: &str = "session.toml";

/// The command log in a session directory.
pub const EDITS_FILE: &str = "edits.jsonl";

/// The folder of plan versions (`projects/v<N>/`) in a session directory.
pub const PROJECTS_DIR: &str = "projects";

/// The boundary records in a session directory (M2.7-4).
pub const BOUNDARIES_FILE: &str = "boundaries.jsonl";

/// The delivered results of the boundary records, when recorded (M2.7-4).
pub const BOUNDARY_DATA_FILE: &str = "boundary-data.f32";

/// What a recorded run did at the due blocks of its latent modules
/// (`docs/session-format.md` §4a, M2.7-4).
#[derive(Clone, Debug, PartialEq)]
pub struct Boundaries {
    /// Every due block's outcome reached the recorder: an exact replay is
    /// possible.
    pub complete: bool,
    /// Delivered results were recorded with their data
    /// (`--record-boundary-data`).
    pub data: bool,
    /// Delivered outcomes recorded.
    pub delivered: u64,
    /// Missed outcomes recorded.
    pub missed: u64,
    /// When incomplete: outcomes the run had that the recorder did not get.
    pub lost: Option<u64>,
    /// When incomplete: why.
    pub reason: Option<String>,
    /// The outcomes, by (frame, instance).
    pub records: Vec<BoundaryRecord>,
}

/// One due block's outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryRecord {
    /// The due frame.
    pub frame: Frame,
    /// The latent instance.
    pub instance: InstanceAddress,
    /// Delivered (with its words, if recorded) or missed.
    pub outcome: BoundaryKind,
}

/// A due result's recorded outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum BoundaryKind {
    /// Delivered.
    Delivered {
        /// The result's words, when the session records data.
        words: Option<Vec<f32>>,
        /// The FNV-1a 64 of the result's words
        /// ([`fnv1a64_words`](signal_core::fnv1a64_words), M2.7-8): always
        /// recorded now; `None` in a session written before hashes.
        fnv1a64: Option<u64>,
    },
    /// Missed: the module played silence for that block; with the cause of
    /// a lost result, when the run recorded one (M2.7-5).
    Missed(Option<LossCause>),
}

/// The machine a session was recorded on (`[machine]` of `session.toml`,
/// M2.7-8): with `[provenance]`'s build, what an exact replay that must
/// recompute results compares before it starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    /// The CPU architecture (`std::env::consts::ARCH`, e.g. `aarch64`).
    pub cpu_arch: String,
    /// Every compute device the run used (a GPU adapter), in the order the
    /// driver lists them; none without one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<Device>,
}

impl Machine {
    /// This machine, with `devices`.
    pub fn here(devices: Vec<Device>) -> Machine {
        Machine {
            cpu_arch: std::env::consts::ARCH.to_owned(),
            devices,
        }
    }
}

/// A compute device a run used: a GPU adapter as its backend describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// The backend (e.g. `Metal`).
    pub backend: String,
    /// The adapter's name.
    pub name: String,
    /// The adapter's PCI vendor id (0 where the backend has none).
    pub vendor: u32,
    /// The adapter's PCI device id (0 where the backend has none).
    pub device_id: u32,
    /// The driver's name.
    pub driver: String,
    /// The driver's version or other information.
    pub driver_info: String,
}

/// The build and machine a replay runs on (M2.7-8), compared with the
/// recording's before an exact replay that must recompute results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Here {
    /// The replaying build, as a render manifest records it.
    pub build: Provenance,
    /// The replaying machine, with the devices the replay will use.
    pub machine: Machine,
}

/// Whether any plan has a module with a declared latency.
fn latent(plans: &[&ExecutionPlan]) -> bool {
    plans
        .iter()
        .any(|p| p.instances.iter().any(|i| i.descriptor.latency.is_some()))
}

/// The line of `boundaries.jsonl` of `record` (without the newline).
fn boundary_line(record: &BoundaryRecord) -> String {
    let head = format!(
        r#"{{"frame":{},"instance":{},"outcome":"#,
        record.frame.0,
        json_string(&record.instance.to_string())
    );
    match &record.outcome {
        BoundaryKind::Missed(None) => format!(r#"{head}"missed"}}"#),
        BoundaryKind::Missed(Some(cause)) => {
            format!(r#"{head}"missed","cause":"{}"}}"#, cause.name())
        }
        BoundaryKind::Delivered { words, fnv1a64 } => {
            let mut line = format!(r#"{head}"delivered""#);
            if let Some(words) = words {
                line.push_str(&format!(r#","words":{}"#, words.len()));
            }
            if let Some(hash) = fnv1a64 {
                line.push_str(&format!(r#","fnv1a64":"{}""#, hex(*hash)));
            }
            line.push('}');
            line
        }
    }
}

/// A line of `boundaries.jsonl` as read.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundaryLineIn {
    frame: u64,
    instance: String,
    outcome: String,
    #[serde(default)]
    words: Option<u64>,
    #[serde(default)]
    cause: Option<String>,
    #[serde(default)]
    fnv1a64: Option<String>,
}

/// `[boundaries]` of `session.toml`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundariesToml {
    complete: bool,
    data: bool,
    delivered: u64,
    missed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lost: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_fnv1a64: Option<String>,
}

/// A suspend or a resume (ADR 0016 §1a).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// The instance's time stops.
    Suspend,
    /// The instance's time runs again.
    Resume,
}

/// One recorded command.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// An external event: `set`, `ramp` or `trigger`. Its sequence number is
    /// the edit's `seq`.
    Event {
        /// An event input or parameter of the plan running at the frame.
        target: PortAddress,
        /// The event.
        payload: EventPayload,
    },
    /// A suspend or resume of an instance.
    Edit {
        /// Which.
        kind: EditKind,
        /// The instance.
        address: InstanceAddress,
    },
    /// A swap to plan version `version` (`projects/v<version>/`).
    Swap {
        /// The version swapped in.
        version: u64,
    },
}

/// A command and its effective frame: one line of `edits.jsonl`.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    /// The effective frame: the frame from which the command applies.
    pub frame: Frame,
    /// Unique in the session; the external event's sequence number.
    pub seq: u64,
    /// The command.
    pub command: Command,
}

impl Edit {
    /// The line of `edits.jsonl` (without the newline): fields in the order
    /// `frame, seq, kind, ...`, numbers in the shortest form that reads back.
    pub fn to_line(&self) -> String {
        let head = format!(r#"{{"frame":{},"seq":{},"kind":"#, self.frame.0, self.seq);
        let string = |s: &dyn fmt::Display| json_string(&s.to_string());
        match &self.command {
            Command::Event { target, payload } => match payload {
                EventPayload::Set(value) => format!(
                    r#"{head}"set","target":{},"value":{}}}"#,
                    string(target),
                    json_number(*value)
                ),
                EventPayload::Ramp { to, frames, curve } => format!(
                    r#"{head}"ramp","target":{},"to":{},"frames":{frames},"curve":{}}}"#,
                    string(target),
                    json_number(*to),
                    json_string(curve_name(*curve))
                ),
                EventPayload::Trigger => {
                    format!(r#"{head}"trigger","target":{}}}"#, string(target))
                }
            },
            Command::Edit { kind, address } => {
                let kind = match kind {
                    EditKind::Suspend => "suspend",
                    EditKind::Resume => "resume",
                };
                format!(r#"{head}"{kind}","address":{}}}"#, string(address))
            }
            Command::Swap { version } => format!(r#"{head}"swap","version":{version}}}"#),
        }
    }
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

fn json_number(value: f64) -> String {
    serde_json::to_string(&value).unwrap_or_default()
}

fn curve_name(curve: Curve) -> &'static str {
    match curve {
        Curve::Linear => "linear",
        _ => "unknown",
    }
}

/// A line of `edits.jsonl` as read.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Line {
    Set {
        frame: u64,
        seq: u64,
        target: String,
        value: f64,
    },
    Ramp {
        frame: u64,
        seq: u64,
        target: String,
        to: f64,
        frames: u64,
        curve: String,
    },
    Trigger {
        frame: u64,
        seq: u64,
        target: String,
    },
    Suspend {
        frame: u64,
        seq: u64,
        address: String,
    },
    Resume {
        frame: u64,
        seq: u64,
        address: String,
    },
    Swap {
        frame: u64,
        seq: u64,
        version: u64,
    },
}

impl Line {
    fn edit(self) -> Result<Edit, String> {
        let port = |target: String| {
            target
                .parse::<PortAddress>()
                .map_err(|e| format!("target `{target}`: {e}"))
        };
        let instance = |address: String| {
            address
                .parse::<InstanceAddress>()
                .map_err(|e| format!("address `{address}`: {e}"))
        };
        let (frame, seq, command) = match self {
            Line::Set {
                frame,
                seq,
                target,
                value,
            } => (
                frame,
                seq,
                Command::Event {
                    target: port(target)?,
                    payload: EventPayload::Set(value),
                },
            ),
            Line::Ramp {
                frame,
                seq,
                target,
                to,
                frames,
                curve,
            } => {
                if curve != "linear" {
                    return Err(format!("unknown curve `{curve}`; expected \"linear\""));
                }
                (
                    frame,
                    seq,
                    Command::Event {
                        target: port(target)?,
                        payload: EventPayload::Ramp {
                            to,
                            frames,
                            curve: Curve::Linear,
                        },
                    },
                )
            }
            Line::Trigger { frame, seq, target } => (
                frame,
                seq,
                Command::Event {
                    target: port(target)?,
                    payload: EventPayload::Trigger,
                },
            ),
            Line::Suspend {
                frame,
                seq,
                address,
            } => (
                frame,
                seq,
                Command::Edit {
                    kind: EditKind::Suspend,
                    address: instance(address)?,
                },
            ),
            Line::Resume {
                frame,
                seq,
                address,
            } => (
                frame,
                seq,
                Command::Edit {
                    kind: EditKind::Resume,
                    address: instance(address)?,
                },
            ),
            Line::Swap {
                frame,
                seq,
                version,
            } => (frame, seq, Command::Swap { version }),
        };
        Ok(Edit {
            frame: Frame(frame),
            seq,
            command,
        })
    }
}

/// `session.toml` as written.
#[derive(Serialize)]
struct SessionOut<'a> {
    format_version: i64,
    sample_rate: u32,
    seed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    frames: Option<u64>,
    provenance: &'a Provenance,
    machine: &'a Machine,
    #[serde(skip_serializing_if = "Option::is_none")]
    boundaries: Option<BoundariesToml>,
    versions: Vec<VersionOut>,
}

#[derive(Serialize)]
struct VersionOut {
    version: u64,
    files: BTreeMap<String, String>,
}

/// `session.toml` as read.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionIn {
    format_version: toml::Spanned<i64>,
    sample_rate: toml::Spanned<u32>,
    seed: u64,
    #[serde(default)]
    frames: Option<u64>,
    provenance: Provenance,
    #[serde(default)]
    machine: Option<Machine>,
    #[serde(default)]
    boundaries: Option<toml::Spanned<BoundariesToml>>,
    versions: Vec<toml::Spanned<VersionIn>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionIn {
    version: u64,
    files: BTreeMap<String, String>,
}

fn hex(hash: u64) -> String {
    format!("{hash:016x}")
}

/// The FNV-1a 64 of each file, by path.
fn file_hashes(files: &[SourceFile]) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|f| (f.path.clone(), hex(fnv1a64(f.text.as_bytes()))))
        .collect()
}

/// `projects/v<version>`, relative to the session directory.
fn version_dir(version: u64) -> String {
    format!("{PROJECTS_DIR}/v{version}")
}

fn error(file: &str, line: usize, column: usize, message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        file: file.to_owned(),
        line,
        column,
        message,
    }
}

/// A diagnostic of version `version`'s project, with its path relative to the
/// session directory.
fn rebased(version: u64, diagnostic: Diagnostic) -> Diagnostic {
    Diagnostic {
        file: format!("{}/{}", version_dir(version), diagnostic.file),
        ..diagnostic
    }
}

/// An instance's state as the engine keeps it across edits and swaps.
#[derive(Clone, Copy)]
struct Slot {
    type_name: &'static str,
    descriptor: ModuleDescriptor,
    suspended: bool,
    edited_at: Option<Frame>,
}

/// What the engine would do with each command: mirrors its rules for
/// discarding input (`Engine::input_discards`, `docs/semantics.md` §12), so
/// that a session holds only commands that take effect.
struct State {
    version: u64,
    instances: BTreeMap<InstanceAddress, Slot>,
}

impl State {
    fn new(plan: &ExecutionPlan, version: u64) -> State {
        let mut state = State {
            version,
            instances: BTreeMap::new(),
        };
        state.swap(plan, version);
        state
    }

    /// A swap: survivors (same address and type) keep their suspension and
    /// last edit; added instances start running.
    fn swap(&mut self, plan: &ExecutionPlan, version: u64) {
        let instances = plan
            .instances
            .iter()
            .map(|i| {
                let slot = match self.instances.get(&i.address) {
                    Some(old) if old.type_name == i.type_name => *old,
                    _ => Slot {
                        type_name: i.type_name,
                        descriptor: i.descriptor,
                        suspended: false,
                        edited_at: None,
                    },
                };
                (i.address.clone(), slot)
            })
            .collect();
        self.instances = instances;
        self.version = version;
    }

    /// Why the engine would discard `command` at `frame`, if it would.
    fn check(&self, frame: Frame, command: &Command) -> Result<(), String> {
        match command {
            Command::Event { target, payload } => {
                let name = target.port();
                let routed = self.instances.get(&target.instance()).is_some_and(|slot| {
                    slot.descriptor.parameters.iter().any(|p| p.name == name)
                        || slot.descriptor.ports.iter().any(|p| {
                            p.name == name
                                && p.direction == Direction::In
                                && p.shape == PortShape::Events
                        })
                });
                if !routed {
                    return Err(format!(
                        "{target} is not an event input or parameter of version {} at frame {}",
                        self.version, frame.0
                    ));
                }
                let finite = match payload {
                    EventPayload::Set(value) | EventPayload::Ramp { to: value, .. } => {
                        value.is_finite()
                    }
                    EventPayload::Trigger => true,
                };
                if !finite {
                    return Err(format!("the value sent to {target} is not finite"));
                }
                Ok(())
            }
            Command::Edit { kind, address } => {
                let Some(slot) = self.instances.get(address) else {
                    return Err(format!(
                        "{address} is not an instance of version {} at frame {}",
                        self.version, frame.0
                    ));
                };
                if slot.edited_at == Some(frame) {
                    return Err(format!(
                        "{address} was already suspended or resumed at frame {}; an instance takes one edit per frame",
                        frame.0
                    ));
                }
                match (kind, slot.suspended) {
                    (EditKind::Suspend, true) => Err(format!("{address} is already suspended")),
                    (EditKind::Resume, false) => Err(format!("{address} is not suspended")),
                    _ => Ok(()),
                }
            }
            Command::Swap { .. } => Ok(()),
        }
    }

    /// Applies an edit that [`check`](State::check) accepted.
    fn apply(&mut self, frame: Frame, command: &Command) {
        if let Command::Edit { kind, address } = command
            && let Some(slot) = self.instances.get_mut(address)
        {
            slot.suspended = *kind == EditKind::Suspend;
            slot.edited_at = Some(frame);
        }
    }
}

/// The edits at or after the session's recorded end `frames`: every command
/// of a session that records its end lies before it.
fn end_errors(edits: &[(usize, Edit)], frames: Option<u64>) -> Vec<Diagnostic> {
    let Some(frames) = frames else {
        return Vec::new();
    };
    edits
        .iter()
        .filter(|(_, edit)| edit.frame.0 >= frames)
        .map(|(line, edit)| {
            error(
                EDITS_FILE,
                *line,
                1,
                format!(
                    "seq {} at frame {} is at or after the session's end (frames = {frames} in session.toml)",
                    edit.seq, edit.frame.0
                ),
            )
        })
        .collect()
}

/// Every rule of `edits.jsonl` (`docs/session-format.md`) that `edits`, with
/// their line numbers, break against the plans of `plans` (index = version).
fn validate(edits: &[(usize, Edit)], plans: &[ExecutionPlan]) -> Vec<Diagnostic> {
    let at = |line: usize, message: String| error(EDITS_FILE, line, 1, message);
    let mut diagnostics = Vec::new();
    let mut seqs = BTreeSet::new();
    let mut previous: Option<(u64, u64)> = None;
    let mut next_version = 1u64;
    for (line, edit) in edits {
        let key = (edit.frame.0, edit.seq);
        if let Some(previous) = previous
            && key <= previous
        {
            diagnostics.push(at(
                *line,
                format!(
                    "(frame {}, seq {}) does not come after the previous line's (frame {}, seq {}); lines are ordered by frame, then seq",
                    key.0, key.1, previous.0, previous.1
                ),
            ));
        }
        previous = Some(previous.map_or(key, |p| p.max(key)));
        if !seqs.insert(edit.seq) {
            diagnostics.push(at(
                *line,
                format!("seq {} is used twice; seq is unique in a session", edit.seq),
            ));
        }
        if let Command::Swap { version } = edit.command {
            if version != next_version {
                diagnostics.push(at(
                    *line,
                    format!("swap to version {version}; the next version is {next_version}"),
                ));
            } else if usize::try_from(version).map_or(true, |v| v >= plans.len()) {
                diagnostics.push(at(
                    *line,
                    format!(
                        "swap to version {version}, which session.toml does not list (versions 0 to {})",
                        plans.len().saturating_sub(1)
                    ),
                ));
            }
            next_version = next_version.max(version).saturating_add(1);
        }
    }
    if diagnostics.is_empty() && usize::try_from(next_version).ok() != Some(plans.len()) {
        diagnostics.push(error(
            SESSION_FILE,
            1,
            1,
            format!(
                "session.toml lists {} versions but edits.jsonl swaps to versions 1 to {}",
                plans.len(),
                next_version - 1
            ),
        ));
    }
    if !diagnostics.is_empty() {
        return diagnostics;
    }

    // Each frame's swap applies before the frame's other commands, whatever
    // their seq (the engine commits it first).
    let Some(first) = plans.first() else {
        return diagnostics;
    };
    let mut state = State::new(first, 0);
    let mut start = 0;
    while let Some((_, head)) = edits.get(start) {
        let frame = head.frame;
        let end = start + edits[start..].partition_point(|(_, e)| e.frame == frame);
        let group = &edits[start..end];
        let mut swapped = false;
        for (line, edit) in group {
            if let Command::Swap { version } = edit.command {
                if swapped {
                    diagnostics.push(at(
                        *line,
                        format!("a second swap at frame {}; at most one per frame", frame.0),
                    ));
                } else if let Some(plan) = usize::try_from(version).ok().and_then(|v| plans.get(v))
                {
                    state.swap(plan, version);
                }
                swapped = true;
            }
        }
        for (line, edit) in group {
            match state.check(frame, &edit.command) {
                Ok(()) => state.apply(frame, &edit.command),
                Err(message) => diagnostics.push(at(*line, message)),
            }
        }
        start = end;
    }
    diagnostics
}

/// The audio sink of `project`'s `plan`: its address and number of signal inputs, or the
/// diagnostics that say why there is not exactly one.
pub fn sink_of(
    project: &Project,
    plan: &ExecutionPlan,
) -> Result<(InstanceAddress, usize), Vec<Diagnostic>> {
    let sinks: Vec<_> = plan
        .instances
        .iter()
        .filter(|i| matches!(i.descriptor.sink, Some(SinkKind::Audio)))
        .collect();
    match sinks.as_slice() {
        [sink] => {
            let inputs = sink
                .descriptor
                .ports
                .iter()
                .filter(|p| p.direction == Direction::In && p.shape == PortShape::Signal)
                .count();
            Ok((sink.address.clone(), inputs))
        }
        [] => Err(vec![project.instances_error(
            "the project has no audio sink (an instance of `std.audio_out`); a session needs exactly one"
                .to_owned(),
        )]),
        several => Err(several
            .iter()
            .map(|sink| {
                project.instance_error(
                    &sink.address,
                    format!(
                        "{}: the project has {} audio sinks; a session needs exactly one",
                        sink.address,
                        several.len()
                    ),
                )
            })
            .collect()),
    }
}

/// Why a [`SessionRecorder`] refused a command or could not write.
#[derive(Debug)]
pub enum SessionError {
    /// The command would not take effect, or breaks a session rule.
    Refused(String),
    /// A project or the recorded session is invalid.
    Invalid(Vec<Diagnostic>),
    /// Writing the directory failed.
    Io {
        /// The path.
        path: PathBuf,
        /// The error.
        error: io::Error,
    },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Refused(message) => f.write_str(message),
            SessionError::Invalid(diagnostics) => {
                for (i, diagnostic) in diagnostics.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{diagnostic}")?;
                }
                Ok(())
            }
            SessionError::Io { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for SessionError {}

/// A suspend or resume reserved by [`SessionRecorder::edit`], to push with
/// `Engine::push_suspend`/`push_resume` and record with
/// [`SessionRecorder::applied_edit`] once the engine has accepted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingEdit {
    /// Suspend or resume.
    pub kind: EditKind,
    /// The instance.
    pub address: InstanceAddress,
    /// The effective frame: push the edit stamped for it.
    pub frame: Frame,
    /// Its seq in the session.
    pub seq: u64,
}

/// Records a session as it is played (control-thread code; it allocates).
///
/// Recording is two-phase, so the session holds exactly what the engine
/// accepted. [`event`](SessionRecorder::event) and
/// [`edit`](SessionRecorder::edit) compute the effective frame by the
/// engine's rule, reserve a seq and check the command against the version
/// running now; the caller pushes what they return and, if the engine accepts
/// it, calls [`applied_event`](SessionRecorder::applied_event) or
/// [`applied_edit`](SessionRecorder::applied_edit). A push the engine hands
/// back is not recorded (its seq stays unused). Record a swap
/// ([`applied_swap`](SessionRecorder::applied_swap)) before input meant for
/// the instances it adds.
pub struct SessionRecorder {
    sample_rate: u32,
    seed: u64,
    provenance: Provenance,
    /// The machine (M2.7-8): this one, with the devices the driver sets
    /// ([`set_devices`](SessionRecorder::set_devices)).
    machine: Machine,
    /// The files and plan of every version.
    versions: Vec<(Vec<SourceFile>, ExecutionPlan)>,
    edits: Vec<Edit>,
    next_seq: u64,
    last_event: Frame,
    last_edit: Frame,
    last_swap: Option<Frame>,
    state: State,
    /// The recorded end ([`set_end`](SessionRecorder::set_end)).
    end: Option<u64>,
    /// The boundary records ([`set_boundaries`](SessionRecorder::set_boundaries)).
    boundaries: Option<Boundaries>,
}

impl SessionRecorder {
    /// A recorder for an engine built from `initial` (version 0) with this
    /// sample rate and seed. `provenance` is the build's, as a render
    /// manifest records it; its file hashes are set to `initial`'s.
    pub fn new(
        initial: &Project,
        registry: &Registry,
        sample_rate: u32,
        seed: u64,
        mut provenance: Provenance,
    ) -> Result<SessionRecorder, SessionError> {
        provenance.project_file_hashes = file_hashes(&initial.files);
        if i64::try_from(seed).is_err() {
            return Err(SessionError::Refused(format!(
                "seed {seed} does not fit a session file (at most 2^63 - 1)"
            )));
        }
        let mut plan = initial.plan(registry).map_err(SessionError::Invalid)?.plan;
        plan.version = 0;
        Ok(SessionRecorder {
            sample_rate,
            seed,
            provenance,
            machine: Machine::here(Vec::new()),
            state: State::new(&plan, 0),
            versions: vec![(initial.files.clone(), plan)],
            edits: Vec::new(),
            next_seq: 1,
            last_event: Frame::ZERO,
            last_edit: Frame::ZERO,
            last_swap: None,
            end: None,
            boundaries: None,
        })
    }

    /// The external event to push for `payload` to `target`, stamped `stamp`,
    /// with `next_frame` the engine's next frame. Its frame is the effective
    /// frame, `max(stamp, next_frame, the last recorded event's frame)`, and
    /// its sequence number a new seq. Refused if the version running now would
    /// discard it.
    pub fn event(
        &mut self,
        stamp: Frame,
        next_frame: Frame,
        target: PortAddress,
        payload: EventPayload,
    ) -> Result<ExternalEvent, SessionError> {
        let frame = stamp.max(next_frame).max(self.last_event);
        let command = Command::Event { target, payload };
        self.state
            .check(frame, &command)
            .map_err(SessionError::Refused)?;
        let Command::Event { target, payload } = command else {
            return Err(SessionError::Refused("not an event".to_owned()));
        };
        Ok(ExternalEvent {
            frame,
            sequence: self.reserve(),
            target,
            payload,
        })
    }

    /// Records an event the engine accepted (`push_external` returned `Ok`),
    /// at its effective frame `event.frame`. Refused, like
    /// [`event`](SessionRecorder::event), if the version running at that
    /// frame would discard it.
    pub fn applied_event(&mut self, event: &ExternalEvent) -> Result<(), SessionError> {
        if event.frame < self.last_event {
            return Err(SessionError::Refused(format!(
                "event {} at frame {} comes before the last recorded event (frame {})",
                event.sequence, event.frame.0, self.last_event.0
            )));
        }
        let command = Command::Event {
            target: event.target.clone(),
            payload: event.payload,
        };
        self.state
            .check(event.frame, &command)
            .map_err(SessionError::Refused)?;
        self.last_event = event.frame;
        self.next_seq = self.next_seq.max(event.sequence.saturating_add(1));
        self.edits.push(Edit {
            frame: event.frame,
            seq: event.sequence,
            command,
        });
        Ok(())
    }

    /// The suspend or resume to push for `address`, stamped `stamp`, with
    /// `next_frame` the engine's next frame. Its frame is the effective frame,
    /// `max(stamp, next_frame, the last recorded edit's frame)`. Refused if the
    /// engine would discard it.
    pub fn edit(
        &mut self,
        kind: EditKind,
        stamp: Frame,
        next_frame: Frame,
        address: InstanceAddress,
    ) -> Result<PendingEdit, SessionError> {
        let frame = stamp.max(next_frame).max(self.last_edit);
        let command = Command::Edit {
            kind,
            address: address.clone(),
        };
        self.state
            .check(frame, &command)
            .map_err(SessionError::Refused)?;
        Ok(PendingEdit {
            kind,
            address,
            frame,
            seq: self.reserve(),
        })
    }

    /// Records an edit the engine accepted (`push_suspend`/`push_resume`
    /// returned `Ok`).
    pub fn applied_edit(&mut self, edit: PendingEdit) -> Result<(), SessionError> {
        let command = Command::Edit {
            kind: edit.kind,
            address: edit.address,
        };
        if edit.frame < self.last_edit {
            return Err(SessionError::Refused(format!(
                "an edit at frame {} comes before the last recorded edit (frame {})",
                edit.frame.0, self.last_edit.0
            )));
        }
        self.state
            .check(edit.frame, &command)
            .map_err(SessionError::Refused)?;
        self.state.apply(edit.frame, &command);
        self.last_edit = edit.frame;
        self.next_seq = self.next_seq.max(edit.seq.saturating_add(1));
        self.edits.push(Edit {
            frame: edit.frame,
            seq: edit.seq,
            command,
        });
        Ok(())
    }

    /// The version the next swap will record: set it as the swapped-in plan's
    /// `version` before `Engine::prepare_swap`, so the log's `plan_version`
    /// names it.
    pub fn next_version(&self) -> u64 {
        self.versions.len() as u64
    }

    /// Records a swap to `project`, committed for `frame` (the engine's next
    /// frame at the commit), and returns its version.
    pub fn applied_swap(
        &mut self,
        frame: Frame,
        project: &Project,
        registry: &Registry,
    ) -> Result<u64, SessionError> {
        self.applied_swap_as(frame, self.next_seq, project, registry)
    }

    /// [`applied_swap`](SessionRecorder::applied_swap) with the seq `seq`,
    /// which the caller chose (a [`Controller`](signal_output::Controller)'s
    /// command id).
    pub fn applied_swap_as(
        &mut self,
        frame: Frame,
        seq: u64,
        project: &Project,
        registry: &Registry,
    ) -> Result<u64, SessionError> {
        if self.last_swap.is_some_and(|last| frame <= last) {
            return Err(SessionError::Refused(format!(
                "a swap at frame {} after one at frame {}; at most one swap per frame, in order",
                frame.0,
                self.last_swap.map_or(0, |f| f.0)
            )));
        }
        let version = self.next_version();
        let mut plan = project.plan(registry).map_err(SessionError::Invalid)?.plan;
        plan.version = version;
        self.state.swap(&plan, version);
        self.versions.push((project.files.clone(), plan));
        self.last_swap = Some(frame);
        self.next_seq = self.next_seq.max(seq.saturating_add(1));
        self.edits.push(Edit {
            frame,
            seq,
            command: Command::Swap { version },
        });
        Ok(version)
    }

    /// Records the session's end: the run played `frames` frames, and replay
    /// renders that many by default (`session.toml`'s `frames`). Refused if a
    /// recorded command lies at or after it.
    pub fn set_end(&mut self, frames: u64) -> Result<(), SessionError> {
        if let Some(edit) = self.edits.iter().find(|e| e.frame.0 >= frames) {
            return Err(SessionError::Refused(format!(
                "the session cannot end at frame {frames}: seq {} applies at frame {}",
                edit.seq, edit.frame.0
            )));
        }
        self.end = Some(frames);
        Ok(())
    }

    /// The recorded end, if any.
    pub fn end(&self) -> Option<u64> {
        self.end
    }

    /// Whether any version has a module with a declared latency: only then
    /// does a session hold boundary records.
    pub fn latent(&self) -> bool {
        latent(&self.versions.iter().map(|(_, p)| p).collect::<Vec<_>>())
    }

    /// Records the outcomes at latent modules' due blocks (M2.7-4). Written
    /// only if some version has a latent module.
    pub fn set_boundaries(&mut self, mut boundaries: Boundaries) {
        boundaries.records.sort_by(|a, b| {
            (a.frame, a.instance.to_string()).cmp(&(b.frame, b.instance.to_string()))
        });
        self.boundaries = Some(boundaries);
    }

    /// The boundary records, if set.
    pub fn boundaries(&self) -> Option<&Boundaries> {
        self.boundaries.as_ref()
    }

    /// Records the compute devices the run used (M2.7-8): the GPU adapters
    /// of its latent modules, which the driver knows and the engine does
    /// not. The CPU architecture is recorded without asking.
    pub fn set_devices(&mut self, devices: Vec<Device>) {
        self.machine.devices = devices;
    }

    /// The machine recorded (`[machine]`, M2.7-8).
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// A new seq.
    fn reserve(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = seq.saturating_add(1);
        seq
    }

    /// The recorded commands in `edits.jsonl` order.
    pub fn edits(&self) -> Vec<Edit> {
        let mut edits = self.edits.clone();
        edits.sort_by_key(|e| (e.frame, e.seq));
        edits
    }

    /// Writes the session directory `dir`, which must not exist or be empty.
    /// The session is validated first, as [`Session::load`] would; an invalid
    /// one is not written.
    pub fn write(&self, dir: &Path) -> Result<(), SessionError> {
        let edits = self.edits();
        let numbered: Vec<(usize, Edit)> = edits
            .iter()
            .enumerate()
            .map(|(i, e)| (i + 1, e.clone()))
            .collect();
        let plans: Vec<ExecutionPlan> = self.versions.iter().map(|(_, p)| p.clone()).collect();
        let mut diagnostics = validate(&numbered, &plans);
        diagnostics.extend(end_errors(&numbered, self.end));
        if !diagnostics.is_empty() {
            return Err(SessionError::Invalid(diagnostics));
        }

        let io = |path: &Path| {
            let path = path.to_owned();
            move |error| SessionError::Io { path, error }
        };
        match fs::read_dir(dir) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(SessionError::Refused(format!(
                        "{} is not empty; a session is written into a new directory",
                        dir.display()
                    )));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(dir)(error)),
        }

        let boundaries = self.boundaries.as_ref().filter(|_| self.latent());
        let data: Option<Vec<u8>> = boundaries.filter(|b| b.data).map(|b| {
            b.records
                .iter()
                .filter_map(|r| match &r.outcome {
                    BoundaryKind::Delivered {
                        words: Some(words), ..
                    } => Some(words),
                    _ => None,
                })
                .flatten()
                .flat_map(|w| w.to_le_bytes())
                .collect()
        });
        let session = SessionOut {
            format_version: FORMAT_VERSION,
            sample_rate: self.sample_rate,
            seed: self.seed,
            frames: self.end,
            provenance: &self.provenance,
            machine: &self.machine,
            boundaries: boundaries.map(|b| BoundariesToml {
                complete: b.complete,
                data: b.data,
                delivered: b.delivered,
                missed: b.missed,
                lost: b.lost,
                reason: b.reason.clone(),
                data_fnv1a64: data.as_ref().map(|d| hex(fnv1a64(d))),
            }),
            versions: self
                .versions
                .iter()
                .enumerate()
                .map(|(version, (files, _))| VersionOut {
                    version: version as u64,
                    files: file_hashes(files),
                })
                .collect(),
        };
        let text = toml::to_string(&session)
            .map_err(|e| SessionError::Refused(format!("writing session.toml: {e}")))?;
        fs::create_dir_all(dir).map_err(io(dir))?;
        let path = dir.join(SESSION_FILE);
        fs::write(&path, text).map_err(io(&path))?;
        for (version, (files, _)) in self.versions.iter().enumerate() {
            let root = dir.join(version_dir(version as u64));
            for file in files {
                let path = root.join(&file.path);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(io(parent))?;
                }
                fs::write(&path, &file.text).map_err(io(&path))?;
            }
        }
        let mut log = String::new();
        for edit in &edits {
            log.push_str(&edit.to_line());
            log.push('\n');
        }
        let path = dir.join(EDITS_FILE);
        fs::write(&path, log).map_err(io(&path))?;
        if let Some(boundaries) = boundaries {
            let mut lines = String::new();
            for record in &boundaries.records {
                lines.push_str(&boundary_line(record));
                lines.push('\n');
            }
            let path = dir.join(BOUNDARIES_FILE);
            fs::write(&path, lines).map_err(io(&path))?;
        }
        if let Some(data) = data {
            let path = dir.join(BOUNDARY_DATA_FILE);
            fs::write(&path, data).map_err(io(&path))?;
        }
        Ok(())
    }
}

/// A reported command waiting for the frontier.
#[derive(Debug)]
enum Applied {
    Swap { plan_version: u64 },
    Edit(PendingEdit),
    Event(ExternalEvent),
}

/// Records a real-time run: the [`ControlSink`] that
/// [`Controller::poll`](signal_output::Controller::poll) feeds (operator
/// decision for M2-3: the engine knows nothing of sessions).
///
/// Reports arrive in the order the runner produced them, which is not the
/// order commands apply: an edit is reported when the runner takes it, a
/// swap when it commits. So reports wait until the runner has run past their
/// frame ([`frontier`](ControlSink::frontier)), and are then recorded in the
/// order the engine applied them: by frame, then swap, edits, events (ADR
/// 0016 §2), then id. Each command is checked against the version running at
/// its own frame; one the recorder refuses is the engine's discard, and is
/// listed in [`refused`](LiveRecorder::refused), not recorded.
///
/// It also records the outcome of every latent module's due block
/// ([`boundary`](ControlSink::boundary), M2.7-4) and, at
/// [`finish`](LiveRecorder::finish), whether it received every one: the
/// runner's counts in the last [`status`](ControlSink::status) must cover the
/// final frame and equal what arrived, with nothing dropped on the way.
pub struct LiveRecorder {
    recorder: SessionRecorder,
    registry: Registry,
    /// The project of every swap sent and not yet recorded, by plan version.
    swaps: BTreeMap<u64, Project>,
    pending: BTreeMap<(Frame, u8, u64), Applied>,
    refused: Vec<String>,
    /// The due blocks' outcomes received.
    records: Vec<BoundaryRecord>,
    /// Delivered results whose data was recorded but did not arrive.
    data_lost: u64,
    /// Whether delivered results are recorded with their data.
    record_data: bool,
    /// The newest runner status received.
    status: Option<RunnerStatus>,
}

impl LiveRecorder {
    /// Records into `recorder`, whose version 0 is the plan the run started
    /// from; `registry` builds the plans of swaps.
    pub fn new(recorder: SessionRecorder, registry: Registry) -> LiveRecorder {
        LiveRecorder {
            recorder,
            registry,
            swaps: BTreeMap::new(),
            pending: BTreeMap::new(),
            refused: Vec::new(),
            records: Vec::new(),
            data_lost: 0,
            record_data: false,
            status: None,
        }
    }

    /// Records delivered results with their data (`--record-boundary-data`):
    /// run the engine with `record_boundary_data` and a boundary-data queue.
    pub fn with_boundary_data(mut self) -> LiveRecorder {
        self.record_data = true;
        self
    }

    /// The plan to swap to `project`, its `version` set to the session version
    /// it will be recorded as. Pass it to
    /// [`Controller::swap`](signal_engine::Controller::swap); if the Controller
    /// refuses it, call [`swap_not_sent`](LiveRecorder::swap_not_sent).
    pub fn swap_plan(&mut self, project: Project) -> Result<ExecutionPlan, SessionError> {
        let mut plan = project
            .plan(&self.registry)
            .map_err(SessionError::Invalid)?
            .plan;
        plan.version = self.recorder.next_version() + self.swaps.len() as u64;
        self.swaps.insert(plan.version, project);
        Ok(plan)
    }

    /// Forgets the last [`swap_plan`](LiveRecorder::swap_plan), which was not
    /// sent.
    pub fn swap_not_sent(&mut self) {
        self.swaps.pop_last();
    }

    /// Commands the recorder refused, with the reason: the engine discarded
    /// them, or a swap was refused at its commit.
    pub fn refused(&self) -> &[String] {
        &self.refused
    }

    /// Reports not yet recorded: their frame has not been passed.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// The recorder.
    pub fn recorder(&self) -> &SessionRecorder {
        &self.recorder
    }

    /// Ends the recording when the driver has stopped: the last
    /// [`frontier`](ControlSink::frontier), with the Runner's own next frame,
    /// then [`set_end`](SessionRecorder::set_end) at that frame, so replay
    /// renders exactly the frames the run played. For a run with latent
    /// modules it also records the boundary outcomes, complete only if a
    /// runner status of `next_frame` arrived (poll once more after the driver
    /// stops, or pass the stopped Runner's own status to
    /// [`status`](ControlSink::status)) and its counts equal what was
    /// received, with no record or data dropped.
    pub fn finish(&mut self, next_frame: Frame) -> Result<(), SessionError> {
        self.frontier(next_frame);
        if self.recorder.latent() {
            let boundaries = self.boundaries(next_frame);
            self.recorder.set_boundaries(boundaries);
        }
        self.recorder.set_end(next_frame.0)
    }

    /// The boundary outcomes received, and whether they are all of the run's
    /// until `next_frame`.
    fn boundaries(&self, next_frame: Frame) -> Boundaries {
        let count = |missed: bool| {
            self.records
                .iter()
                .filter(|r| matches!(r.outcome, BoundaryKind::Missed(_)) == missed)
                .count() as u64
        };
        let (delivered, missed) = (count(false), count(true));
        let mut lost = None;
        let reason = match self.status {
            None => Some("no runner status reached the recorder".to_owned()),
            Some(status) if status.next_frame != next_frame => Some(format!(
                "the last runner status that reached the recorder is of frame {}, not of the end, frame {}",
                status.next_frame.0, next_frame.0
            )),
            Some(status) => {
                let expected = status.delivered_results.saturating_add(status.late_results);
                lost = Some(expected.saturating_sub(delivered.saturating_add(missed)));
                let dropped = status
                    .log_overflows
                    .saturating_add(status.engine_log_overflows);
                if status.delivered_results != delivered || status.late_results != missed {
                    Some(format!(
                        "the run had {} delivered and {} missed results; {delivered} and {missed} were recorded",
                        status.delivered_results, status.late_results
                    ))
                } else if dropped > 0 {
                    Some(format!("{dropped} log records were dropped (a full queue)"))
                } else if self.record_data
                    && status
                        .boundary_data_overflows
                        .saturating_add(self.data_lost)
                        > 0
                {
                    Some(format!(
                        "the data of {} delivered results was dropped (a full queue)",
                        status.boundary_data_overflows.max(self.data_lost)
                    ))
                } else {
                    None
                }
            }
        };
        Boundaries {
            complete: reason.is_none(),
            data: self.record_data,
            delivered,
            missed,
            lost: lost.filter(|_| reason.is_some()),
            reason,
            records: self.records.clone(),
        }
    }

    /// The recorder, to [`write`](SessionRecorder::write) the session.
    pub fn into_recorder(self) -> SessionRecorder {
        self.recorder
    }

    fn record(&mut self, applied: Applied, frame: Frame, id: u64) {
        let result = match applied {
            Applied::Swap { plan_version } => match self.swaps.remove(&plan_version) {
                Some(project) if plan_version == self.recorder.next_version() => self
                    .recorder
                    .applied_swap_as(frame, id, &project, &self.registry)
                    .map(|_| ()),
                _ => Err(SessionError::Refused(format!(
                    "a swap to plan version {plan_version} that this recorder did not plan next"
                ))),
            },
            Applied::Edit(edit) => self.recorder.applied_edit(edit),
            Applied::Event(event) => self.recorder.applied_event(&event),
        };
        if let Err(error) = result {
            self.refused
                .push(format!("command {id} at frame {}: {error}", frame.0));
        }
    }
}

impl ControlSink for LiveRecorder {
    fn report(&mut self, report: &Report, command: &SentCommand) {
        let frame = report.frame;
        if report.outcome == Outcome::Refused {
            if let SentCommand::Swap { plan_version } = command {
                self.swaps.remove(plan_version);
            }
            self.refused.push(format!(
                "command {} was refused by the engine at frame {}",
                report.id, frame.0
            ));
            return;
        }
        let (class, applied) = match command {
            SentCommand::Swap { plan_version } => (
                0,
                Applied::Swap {
                    plan_version: *plan_version,
                },
            ),
            SentCommand::Suspend(address) | SentCommand::Resume(address) => {
                let kind = match command {
                    SentCommand::Suspend(_) => EditKind::Suspend,
                    _ => EditKind::Resume,
                };
                let edit = PendingEdit {
                    kind,
                    address: address.clone(),
                    frame,
                    seq: report.id,
                };
                (1, Applied::Edit(edit))
            }
            SentCommand::Event(event) => {
                let event = ExternalEvent {
                    frame,
                    ..event.clone()
                };
                (2, Applied::Event(event))
            }
        };
        self.pending.insert((frame, class, report.id), applied);
    }

    fn status(&mut self, status: &RunnerStatus) {
        if self
            .status
            .is_none_or(|known| status.next_frame >= known.next_frame)
        {
            self.status = Some(*status);
        }
    }

    fn boundary(&mut self, line: BoundaryLine<'_>) {
        let outcome = match line.outcome {
            BoundaryOutcome::Missed { cause } => BoundaryKind::Missed(cause),
            BoundaryOutcome::Delivered {
                data,
                data_lost,
                fnv1a64,
            } => {
                if data_lost {
                    self.data_lost = self.data_lost.saturating_add(1);
                }
                BoundaryKind::Delivered {
                    words: data.filter(|_| self.record_data).map(<[f32]>::to_vec),
                    fnv1a64: Some(fnv1a64),
                }
            }
        };
        self.records.push(BoundaryRecord {
            frame: line.frame,
            instance: line.address.clone(),
            outcome,
        });
    }

    fn frontier(&mut self, next_frame: Frame) {
        while self
            .pending
            .first_key_value()
            .is_some_and(|((frame, _, _), _)| *frame < next_frame)
        {
            if let Some(((frame, _, id), applied)) = self.pending.pop_first() {
                self.record(applied, frame, id);
            }
        }
    }
}

/// A validated session, ready to replay.
pub struct Session {
    /// The master rate.
    pub sample_rate: u32,
    /// The master seed.
    pub seed: u64,
    /// The frames the recorded run played (`session.toml`'s `frames`), if it
    /// recorded its end.
    pub frames: Option<u64>,
    /// The build that recorded it.
    pub provenance: Provenance,
    /// The machine that recorded it (M2.7-8); `None` for a session written
    /// before it was recorded.
    pub machine: Option<Machine>,
    /// Every plan version's project, by version.
    pub versions: Vec<Project>,
    /// Every plan version's plan, by version, its `version` set to it.
    pub plans: Vec<ExecutionPlan>,
    /// The commands, in `edits.jsonl` order.
    pub edits: Vec<Edit>,
    /// The latent modules' due-block outcomes (M2.7-4); `None` for a session
    /// without (written before them, or of a project without latent modules).
    pub boundaries: Option<Boundaries>,
}

impl Session {
    /// Reads and validates the session directory `dir` with the module types
    /// of `registry`. Diagnostics name files relative to `dir`, with line and
    /// column where they apply; a file that cannot be read is an I/O error.
    pub fn load(dir: &Path, registry: &Registry) -> Result<Session, LoadError> {
        let read = |name: &str| {
            let path = dir.join(name);
            fs::read(&path).map_err(|error| LoadError::Io { path, error })
        };
        let text_of = |name: &str, bytes: Vec<u8>| {
            String::from_utf8(bytes).map_err(|_| {
                LoadError::Invalid(vec![error(name, 1, 1, "the file is not UTF-8".to_owned())])
            })
        };
        let text = text_of(SESSION_FILE, read(SESSION_FILE)?)?;
        let file: SessionIn = toml::from_str(&text).map_err(|e| {
            let offset = e.span().map_or(0, |s| s.start);
            let message = e.message().trim().to_owned();
            LoadError::Invalid(vec![Diagnostic::at(
                Severity::Error,
                SESSION_FILE,
                &text,
                offset,
                message,
            )])
        })?;
        let at = |offset: usize, message: String| {
            Diagnostic::at(Severity::Error, SESSION_FILE, &text, offset, message)
        };

        let mut diagnostics = Vec::new();
        if *file.format_version.get_ref() != FORMAT_VERSION {
            diagnostics.push(at(
                file.format_version.span().start,
                format!(
                    "unsupported format_version {}; this version of Signal reads {FORMAT_VERSION}",
                    file.format_version.get_ref()
                ),
            ));
        }
        if *file.sample_rate.get_ref() == 0 {
            diagnostics.push(at(
                file.sample_rate.span().start,
                "sample_rate must be at least 1".to_owned(),
            ));
        }
        if file.versions.is_empty() {
            diagnostics.push(at(0, "the session lists no versions".to_owned()));
        }
        if !diagnostics.is_empty() {
            return Err(LoadError::Invalid(diagnostics));
        }

        let mut versions = Vec::new();
        let mut plans = Vec::new();
        let mut sink = None;
        for (index, entry) in file.versions.iter().enumerate() {
            let version = index as u64;
            let span = entry.span().start;
            let entry = entry.get_ref();
            if entry.version != version {
                diagnostics.push(at(
                    span,
                    format!(
                        "version {} is listed in place of version {version}; versions are 0, 1, 2, … in order",
                        entry.version
                    ),
                ));
                continue;
            }
            let project = match Project::load(&dir.join(version_dir(version))) {
                Ok(project) => project,
                Err(LoadError::Invalid(found)) => {
                    diagnostics.extend(found.into_iter().map(|d| rebased(version, d)));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let present = file_hashes(&project.files);
            for (path, hash) in &present {
                match entry.files.get(path) {
                    Some(recorded) if recorded == hash => {}
                    Some(recorded) => diagnostics.push(error(
                        &format!("{}/{path}", version_dir(version)),
                        1,
                        1,
                        format!(
                            "the file's FNV-1a 64 is {hash}; session.toml records {recorded} (the copy was changed)"
                        ),
                    )),
                    None => diagnostics.push(error(
                        &format!("{}/{path}", version_dir(version)),
                        1,
                        1,
                        format!("session.toml does not list this file under version {version}"),
                    )),
                }
            }
            for path in entry.files.keys().filter(|p| !present.contains_key(*p)) {
                diagnostics.push(at(
                    span,
                    format!(
                        "version {version} lists {path}, which {}/ does not have",
                        version_dir(version)
                    ),
                ));
            }
            match project.plan(registry) {
                Ok(planned) => {
                    let mut plan = planned.plan;
                    plan.version = version;
                    match (sink_of(&project, &plan), &sink) {
                        (Err(found), _) => {
                            diagnostics.extend(found.into_iter().map(|d| rebased(version, d)));
                        }
                        (Ok(found), None) => sink = Some(found),
                        (Ok((address, width)), Some((first, first_width))) => {
                            if address != *first || width != *first_width {
                                diagnostics.push(rebased(
                                    version,
                                    project.instance_error(
                                        &address,
                                        format!(
                                            "{address}: the audio sink must stay {first} with {first_width} inputs in every version of a session"
                                        ),
                                    ),
                                ));
                            }
                        }
                    }
                    plans.push(plan);
                }
                Err(found) => diagnostics.extend(found.into_iter().map(|d| rebased(version, d))),
            }
            versions.push(project);
        }
        // Report the project copies' errors even when edits.jsonl is unreadable.
        let edits_text = match read(EDITS_FILE) {
            Err(_) if !diagnostics.is_empty() => return Err(LoadError::Invalid(diagnostics)),
            bytes => text_of(EDITS_FILE, bytes?)?,
        };
        let mut edits = Vec::new();
        let lines = edits_text.strip_suffix('\n').unwrap_or(&edits_text);
        if !lines.is_empty() {
            for (i, text) in lines.split('\n').enumerate() {
                let line = i + 1;
                match serde_json::from_str::<Line>(text) {
                    Ok(parsed) => match parsed.edit() {
                        Ok(edit) => edits.push((line, edit)),
                        Err(message) => diagnostics.push(error(EDITS_FILE, line, 1, message)),
                    },
                    Err(e) => diagnostics.push(error(
                        EDITS_FILE,
                        line,
                        e.column().max(1),
                        json_message(&e),
                    )),
                }
            }
        }
        if !diagnostics.is_empty() {
            return Err(LoadError::Invalid(diagnostics));
        }
        let mut diagnostics = validate(&edits, &plans);
        diagnostics.extend(end_errors(&edits, file.frames));
        if !diagnostics.is_empty() {
            return Err(LoadError::Invalid(diagnostics));
        }
        let boundaries = match &file.boundaries {
            None => None,
            Some(table) => {
                let span = table.span().start;
                let table = table.get_ref();
                let lines = text_of(BOUNDARIES_FILE, read(BOUNDARIES_FILE)?)?;
                let data = if table.data {
                    Some(read(BOUNDARY_DATA_FILE)?)
                } else {
                    None
                };
                let found = load_boundaries(
                    table,
                    &lines,
                    data.as_deref(),
                    (&edits, &plans, file.frames),
                );
                match found {
                    Ok(boundaries) => Some(boundaries),
                    Err(mut found) => {
                        for d in &mut found {
                            if d.file == SESSION_FILE {
                                *d = at(span, d.message.clone());
                            }
                        }
                        return Err(LoadError::Invalid(found));
                    }
                }
            }
        };

        Ok(Session {
            sample_rate: *file.sample_rate.get_ref(),
            seed: file.seed,
            frames: file.frames,
            provenance: file.provenance,
            machine: file.machine,
            versions,
            plans,
            edits: edits.into_iter().map(|(_, e)| e).collect(),
            boundaries,
        })
    }

    /// Whether any version has a module with a declared latency.
    pub fn latent(&self) -> bool {
        latent(&self.plans.iter().collect::<Vec<_>>())
    }

    /// The default replay length: the recorded end, else the last command's
    /// frame, then one second.
    pub fn default_frames(&self) -> u64 {
        if let Some(frames) = self.frames {
            return frames;
        }
        let end = self.edits.last().map_or(0, |e| e.frame.0.saturating_add(1));
        end.saturating_add(u64::from(self.sample_rate))
    }
}

/// `boundaries.jsonl` (and `boundary-data.f32`) checked against `[boundaries]`,
/// the commands and the plans: each line a due-block outcome of a latent
/// instance of the version running at its frame, before the session's end,
/// ordered by (frame, instance), as many of each kind as `session.toml` says,
/// and the data exactly the words the lines name.
fn load_boundaries(
    table: &BoundariesToml,
    text: &str,
    data: Option<&[u8]>,
    (edits, plans, frames): (&[(usize, Edit)], &[ExecutionPlan], Option<u64>),
) -> Result<Boundaries, Vec<Diagnostic>> {
    let at = |line: usize, message: String| error(BOUNDARIES_FILE, line, 1, message);
    let mut diagnostics = Vec::new();
    let swaps: Vec<(Frame, usize)> = edits
        .iter()
        .filter_map(|(_, e)| match e.command {
            Command::Swap { version } => Some((e.frame, version as usize)),
            _ => None,
        })
        .collect();
    // The words the lines name, checked against the data present before
    // anything is allocated for them: the parsed count is authoritative.
    let available = data.map_or(0, |d| d.len() / 4) as u64;
    let mut words_total = 0u64;
    let mut counts = Vec::new();
    let mut parsed = Vec::new();
    let mut previous: Option<(u64, String)> = None;
    let body = text.strip_suffix('\n').unwrap_or(text);
    for (i, line_text) in body.split('\n').enumerate().filter(|_| !body.is_empty()) {
        let line = i + 1;
        let read: BoundaryLineIn = match serde_json::from_str(line_text) {
            Ok(read) => read,
            Err(e) => {
                diagnostics.push(error(
                    BOUNDARIES_FILE,
                    line,
                    e.column().max(1),
                    json_message(&e),
                ));
                continue;
            }
        };
        let instance = match read.instance.parse::<InstanceAddress>() {
            Ok(instance) => instance,
            Err(e) => {
                diagnostics.push(at(line, format!("instance `{}`: {e}", read.instance)));
                continue;
            }
        };
        let key = (read.frame, read.instance.clone());
        if previous.as_ref().is_some_and(|p| key <= *p) {
            diagnostics.push(at(
                line,
                format!(
                    "(frame {}, {}) does not come after the previous line's; lines are ordered by frame, then instance",
                    read.frame, read.instance
                ),
            ));
        }
        previous = Some(key);
        if frames.is_some_and(|end| read.frame >= end) {
            diagnostics.push(at(
                line,
                format!("frame {} is at or after the session's end", read.frame),
            ));
        }
        let version = swaps
            .iter()
            .take_while(|(f, _)| f.0 <= read.frame)
            .last()
            .map_or(0, |&(_, v)| v);
        let is_latent = plans.get(version).is_some_and(|p| {
            p.instances
                .iter()
                .any(|i| i.address == instance && i.descriptor.latency.is_some())
        });
        if !is_latent {
            diagnostics.push(at(
                line,
                format!(
                    "{instance} is not a latent instance of version {version}, which runs at frame {}",
                    read.frame
                ),
            ));
        }
        let cause = match read.cause.as_deref().map(|c| (c, LossCause::from_name(c))) {
            None => None,
            Some((_, Some(cause))) if read.outcome == "missed" => Some(cause),
            Some((name, known)) => {
                diagnostics.push(at(
                    line,
                    match known {
                        None => format!("cause `{name}` is not a known cause"),
                        Some(_) => format!("cause `{name}` on a `{}` line", read.outcome),
                    },
                ));
                continue;
            }
        };
        // The result's hash (M2.7-8): 16 lowercase hex digits, on a
        // delivered line only.
        let fnv1a64 = match read.fnv1a64.as_deref() {
            None => None,
            Some(text) if read.outcome == "delivered" => match parse_hex(text) {
                Some(hash) => Some(hash),
                None => {
                    diagnostics.push(at(
                        line,
                        format!("fnv1a64 `{text}`: expected 16 lowercase hex digits"),
                    ));
                    continue;
                }
            },
            Some(_) => {
                diagnostics.push(at(line, format!("fnv1a64 on a `{}` line", read.outcome)));
                continue;
            }
        };
        let outcome = match (read.outcome.as_str(), read.words) {
            ("missed", None) => BoundaryKind::Missed(cause),
            ("delivered", None) => BoundaryKind::Delivered {
                words: None,
                fnv1a64,
            },
            ("delivered", Some(words)) if table.data => {
                match words_total.checked_add(words).filter(|&t| t <= available) {
                    Some(total) => words_total = total,
                    None => {
                        diagnostics.push(at(
                            line,
                            format!(
                                "words {words}: boundary-data.f32 holds {available} words, {} of them named by earlier lines",
                                words_total
                            ),
                        ));
                        continue;
                    }
                }
                counts.push((words as usize, line));
                BoundaryKind::Delivered {
                    words: Some(Vec::new()),
                    fnv1a64,
                }
            }
            ("delivered", Some(_)) => {
                diagnostics.push(at(
                    line,
                    "words, but session.toml says the session records no data".to_owned(),
                ));
                continue;
            }
            (other, _) => {
                diagnostics.push(at(
                    line,
                    format!(
                        "outcome `{other}`, with words {:?}; expected \"delivered\" (words only with data) or \"missed\"",
                        read.words
                    ),
                ));
                continue;
            }
        };
        parsed.push(BoundaryRecord {
            frame: Frame(read.frame),
            instance,
            outcome,
        });
    }
    let delivered = parsed
        .iter()
        .filter(|r| !matches!(r.outcome, BoundaryKind::Missed(_)))
        .count() as u64;
    let missed = parsed.len() as u64 - delivered;
    if diagnostics.is_empty() && (delivered != table.delivered || missed != table.missed) {
        diagnostics.push(error(
            SESSION_FILE,
            1,
            1,
            format!(
                "[boundaries] says {} delivered and {} missed; boundaries.jsonl has {delivered} and {missed}",
                table.delivered, table.missed
            ),
        ));
    }
    if let Some(data) = data {
        let hash = hex(fnv1a64(data));
        if table.data_fnv1a64.as_deref() != Some(hash.as_str()) {
            diagnostics.push(error(
                BOUNDARY_DATA_FILE,
                1,
                1,
                format!(
                    "the file's FNV-1a 64 is {hash}; session.toml records {}",
                    table.data_fnv1a64.as_deref().unwrap_or("none")
                ),
            ));
        } else if data.len() as u64 != words_total * 4 {
            diagnostics.push(error(
                BOUNDARY_DATA_FILE,
                1,
                1,
                format!(
                    "{} bytes; boundaries.jsonl names {words_total} words ({} bytes)",
                    data.len(),
                    words_total * 4
                ),
            ));
        } else if diagnostics.is_empty() {
            // Every count is within the data, and they add up to it.
            let words = data.as_chunks::<4>().0;
            let mut counts = counts.into_iter();
            let mut start = 0;
            for record in &mut parsed {
                if let BoundaryKind::Delivered {
                    words: Some(v),
                    fnv1a64,
                } = &mut record.outcome
                {
                    let (n, line) = counts.next().unwrap_or((0, 0));
                    let chunk = words.get(start..start + n).unwrap_or(&[]);
                    v.extend(chunk.iter().map(|&b| f32::from_le_bytes(b)));
                    start += n;
                    // M2.7-8: the recorded data is the result the run hashed.
                    let hash = fnv1a64_words(v);
                    if fnv1a64.is_some_and(|recorded| recorded != hash) {
                        diagnostics.push(at(
                            line,
                            format!(
                                "the {n} words of this line in boundary-data.f32 have FNV-1a 64 {}; the line records {}",
                                hex(hash),
                                fnv1a64.map(hex).unwrap_or_default()
                            ),
                        ));
                    }
                }
            }
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    Ok(Boundaries {
        complete: table.complete,
        data: table.data,
        delivered,
        missed,
        lost: table.lost,
        reason: table.reason.clone(),
        records: parsed,
    })
}

/// A hash as written by [`hex`]: exactly 16 lowercase hex digits.
fn parse_hex(text: &str) -> Option<u64> {
    let valid = text.len() == 16 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    valid.then(|| u64::from_str_radix(text, 16).ok()).flatten()
}

/// serde_json's message without its own " at line L column C" suffix.
fn json_message(error: &serde_json::Error) -> String {
    let text = error.to_string();
    match text.rfind(" at line ") {
        Some(at) => text[..at].to_owned(),
        None => text,
    }
}

/// A replayed session.
pub struct Replay {
    /// The audio and log, as `signal render` records them.
    pub render: Render,
    /// Commands at or after the last rendered frame, not applied.
    pub skipped: usize,
    /// How the latent modules' due results were treated (M2.7-4).
    pub boundaries: BoundaryReplay,
    /// Exact replay (M2.7-8): recorded deliveries replayed without a hash to
    /// check them against (a session written before hashes). Their
    /// exactness is unverified; 0 for every session recorded since.
    pub unverified: u64,
    /// Exact replay (M2.7-8): `--trust-provenance` let it run although the
    /// recording's build or machine differ. Its manifest says so.
    pub trusted_provenance: bool,
    /// M2.8-7: the snapshots of [`ReplayOptions::vectors`], one track per
    /// tap: the visual streams the replay reproduces, as the real-time
    /// vector adapter takes them.
    pub vectors: Vec<VectorTrack>,
}

/// How a replay treated the latent modules' due results
/// (`docs/semantics.md` §15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryReplay {
    /// The session's plans have no latent module: exact and ideal agree.
    NotLatent,
    /// Exact replay: every recorded outcome was applied.
    Exact,
    /// The session has latent modules but no boundary records (written
    /// before them): every result was awaited, an ideal render.
    NotRecorded,
    /// `--ideal`: every result was awaited, whatever was recorded.
    Ideal,
}

impl BoundaryReplay {
    /// Whether the replay is an ideal render: what its manifest says.
    pub fn ideal(self) -> bool {
        matches!(self, BoundaryReplay::NotRecorded | BoundaryReplay::Ideal)
    }
}

/// How to replay a session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayOptions {
    /// Frames to render; by default [`Session::default_frames`].
    pub frames: Option<u64>,
    /// The engine's thread count (`EngineConfig::threads`); it never changes
    /// the output.
    pub threads: usize,
    /// Render ideally: await every latent module's result, whatever the
    /// session recorded (`signal replay --ideal`).
    pub ideal: bool,
    /// Replay exactly although the recording's build or machine differ from
    /// [`here`](ReplayOptions::here) (`signal replay --trust-provenance`,
    /// M2.7-8): every recomputed result is still checked against its hash.
    pub trust_provenance: bool,
    /// The replaying build and machine (M2.7-8), compared with the
    /// recording's before an exact replay that must recompute results.
    /// `None`: this build ([`provenance::collect`](crate::provenance::collect),
    /// asked only when the comparison applies) on this machine with no
    /// compute device; a caller whose modules use a GPU names it here.
    pub here: Option<Here>,
    /// M2.8-7: vector outputs to snapshot during the replay
    /// ([`Replay::vectors`]); they only read, the render is unchanged.
    pub vectors: Vec<VectorTap>,
}

/// Why a validated session could not be replayed. Validation rules these out,
/// so each is a bug.
#[derive(Debug)]
pub enum ReplayError {
    /// The engine refused the plan, a swap or an edit.
    Engine(EngineError),
    /// The render failed.
    Render(RenderError),
    /// The engine refused a push (its queue was full).
    Refused(String),
    /// The session's boundary records are incomplete (M2.7-4): it cannot be
    /// replayed exactly; an ideal render can.
    IncompleteBoundaries {
        /// Why, as recorded.
        reason: String,
    },
    /// An exact replay asked for more frames than the session recorded: the
    /// frames past its end were never played (M2.7-4).
    BeyondRecordedEnd {
        /// The frames asked for.
        frames: u64,
        /// The session's recorded end.
        end: u64,
    },
    /// Recorded deliveries that this run could not reproduce: the device
    /// lost them, or the module refused the recorded data (M2.7-4).
    Unreproduced {
        /// How many.
        results: u64,
    },
    /// The result of `module`'s due block at frame `block`, recomputed or
    /// injected, differs from the one the performance used (its recorded
    /// hash, M2.7-8): the replay is not the performance. The first such block;
    /// it takes precedence over every other failure of the replay.
    ReplayMismatch {
        /// The latent instance.
        module: InstanceAddress,
        /// The due frame: the first of its block.
        block: Frame,
    },
    /// The session's results must be recomputed (it has no data for them),
    /// but this build or machine is not the recording's (M2.7-8): the exact
    /// replay is refused before it starts.
    ProvenanceMismatch {
        /// What differs, recorded and here.
        differences: Vec<String>,
    },
    /// The recorded outcomes and the replay's due blocks disagree (a bug, or
    /// a changed session).
    BoundaryMismatch {
        /// Due blocks without an outcome plus outcomes without a due block.
        mismatches: u64,
    },
    /// Input had no effect, or a queue or the log overflowed.
    Discarded {
        /// Input the engine discarded.
        discards: InputDiscards,
        /// External events lost to a full staging buffer.
        input_overflows: u64,
        /// Log records lost to a full log.
        log_overflows: u64,
    },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Engine(error) => write!(f, "replay: {error}"),
            ReplayError::Render(error) => write!(f, "replay: {error}"),
            ReplayError::Refused(message) => write!(f, "replay: {message}"),
            ReplayError::IncompleteBoundaries { reason } => write!(
                f,
                "replay: the session's boundary records are incomplete ({reason}): it cannot be replayed exactly; `signal replay --ideal` renders it with every result awaited"
            ),
            ReplayError::BeyondRecordedEnd { frames, end } => write!(
                f,
                "replay: {frames} frames asked for, but the session recorded {end}: an exact replay reproduces only what was played; `signal replay --ideal` renders past the end"
            ),
            ReplayError::Unreproduced { results } => write!(
                f,
                "replay: {results} recorded results of latent modules could not be reproduced on this machine (no device, or another one); record with --record-boundary-data, or render with --ideal"
            ),
            ReplayError::ReplayMismatch { module, block } => write!(
                f,
                "replay: the result of {module}'s block at frame {} differs from the one the performance used (its recorded FNV-1a 64): this machine does not reproduce the performance exactly; `signal replay --ideal` renders it with every result awaited, and a session recorded with `signal play --record-boundary-data` replays exactly anywhere",
                block.0
            ),
            ReplayError::ProvenanceMismatch { differences } => write!(
                f,
                "replay: the session's results of latent modules must be recomputed, but this is not the build and machine that recorded it ({}): an exact replay is refused; `signal replay --ideal` renders it with every result awaited, `--trust-provenance` replays exactly anyway (each result still checked against its recorded hash, and the manifest says so), and a session recorded with `signal play --record-boundary-data` replays exactly anywhere",
                differences.join("; ")
            ),
            ReplayError::BoundaryMismatch { mismatches } => write!(
                f,
                "replay: the recorded boundary outcomes do not match the replay's due blocks ({mismatches} mismatches)"
            ),
            ReplayError::Discarded {
                discards,
                input_overflows,
                log_overflows,
            } => write!(
                f,
                "replay: the engine discarded input ({} unrouted, {} invalid) or overflowed ({input_overflows} input, {log_overflows} log)",
                discards.unrouted, discards.invalid
            ),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Replays `session` for `frames` frames (by default
/// [`Session::default_frames`]) exactly as it was played: see [`replay_with`].
/// `threads` is the engine's thread count (`EngineConfig::threads`); it
/// never changes the output.
pub fn replay(
    session: &Session,
    registry: &Registry,
    frames: Option<u64>,
    threads: usize,
) -> Result<Replay, ReplayError> {
    let options = ReplayOptions {
        frames,
        threads,
        ..ReplayOptions::default()
    };
    replay_with(session, registry, options)
}

/// Replays `session`: builds the engine from version 0 and, at each frame
/// with commands, commits its swap and pushes its other commands with
/// `push_external`, `push_suspend` and `push_resume`, as a live driver does.
/// It renders in blocks split at those frames (`docs/semantics.md` §13).
///
/// The input queues are sized for the session's busiest frame (queue capacity
/// is not part of the semantics, `docs/semantics.md` §11), and reclaimed
/// before every block. Any discarded input or overflow afterwards is an error.
///
/// With boundary records it is an **exact replay** (`docs/semantics.md`
/// §15): each recorded outcome is pushed with `push_boundary`, a recorded
/// miss is applied without waiting, recorded data is shown, and the audio is
/// stored as the device received it. Incomplete records are an error. Every
/// delivered result, recomputed or injected, is checked against its recorded
/// hash (M2.7-8): the first that differs fails with
/// [`ReplayMismatch`](ReplayError::ReplayMismatch), before any other error.
/// Results to recompute are the recording's only on its build and machine:
/// another one is refused up front
/// ([`ProvenanceMismatch`](ReplayError::ProvenanceMismatch)) unless
/// [`trust_provenance`](ReplayOptions::trust_provenance). With `ideal`, or
/// without records, it is an **ideal render**: every result is awaited, and
/// the manifest says `ideal`.
pub fn replay_with(
    session: &Session,
    registry: &Registry,
    options: ReplayOptions,
) -> Result<Replay, ReplayError> {
    let frames = options.frames.unwrap_or_else(|| session.default_frames());
    let mode = match (&session.boundaries, options.ideal) {
        (_, true) => BoundaryReplay::Ideal,
        (Some(boundaries), false) if !boundaries.complete => {
            return Err(ReplayError::IncompleteBoundaries {
                reason: boundaries
                    .reason
                    .clone()
                    .unwrap_or_else(|| "records are missing".to_owned()),
            });
        }
        (Some(_), false) => {
            // Frames past the end were never played: nothing to reproduce.
            if let Some(end) = session.frames.filter(|&end| frames > end) {
                return Err(ReplayError::BeyondRecordedEnd { frames, end });
            }
            BoundaryReplay::Exact
        }
        (None, false) if session.latent() => BoundaryReplay::NotRecorded,
        (None, false) => BoundaryReplay::NotLatent,
    };
    let script: &[BoundaryRecord] = match (&session.boundaries, mode) {
        (Some(b), BoundaryReplay::Exact) => &b.records,
        _ => &[],
    };
    let script = &script[..script.partition_point(|r| r.frame.0 < frames)];
    // M2.7-8: results the replay must recompute (no recorded data) are the
    // recording's only on its build and machine, checked before it starts.
    let recomputed = script
        .iter()
        .any(|r| matches!(r.outcome, BoundaryKind::Delivered { words: None, .. }));
    let mut trusted = false;
    if let (Some(recorded), true) = (&session.machine, recomputed) {
        let here = options.here.clone().unwrap_or_else(|| Here {
            build: crate::provenance::collect(
                &crate::provenance::source_dir(),
                session.versions.first().map_or(&[][..], |p| &p.files),
            ),
            machine: Machine::here(Vec::new()),
        });
        let differences = provenance_differences(&session.provenance, recorded, &here);
        if !differences.is_empty() {
            if !options.trust_provenance {
                return Err(ReplayError::ProvenanceMismatch { differences });
            }
            trusted = true;
        }
    }
    let unverified = script
        .iter()
        .filter(|r| matches!(r.outcome, BoundaryKind::Delivered { fnv1a64: None, .. }))
        .count() as u64;
    let mut externals = BTreeMap::<u64, usize>::new();
    let mut edits = BTreeMap::<u64, usize>::new();
    for edit in &session.edits {
        match edit.command {
            Command::Event { .. } => *externals.entry(edit.frame.0).or_default() += 1,
            Command::Edit { .. } => *edits.entry(edit.frame.0).or_default() += 1,
            Command::Swap { .. } => {}
        }
    }
    let busiest = externals.values().chain(edits.values()).copied().max();
    // Outcomes are pushed up to a block ahead, and wait at most a block more.
    let block = EngineConfig::DEFAULT_BLOCK_CAPACITY;
    let window = 2 * block as u64;
    let mut most_outcomes = 0;
    let mut first = 0;
    for (last, record) in script.iter().enumerate() {
        while script[first].frame.0 + window <= record.frame.0 {
            first += 1;
        }
        most_outcomes = most_outcomes.max(last + 1 - first);
    }
    let config = EngineConfig {
        queue_capacity: busiest
            .unwrap_or(0)
            .max(most_outcomes)
            .max(EngineConfig::DEFAULT_QUEUE_CAPACITY),
        threads: options.threads,
        scripted_boundaries: mode == BoundaryReplay::Exact,
        ..EngineConfig::new(session.sample_rate, session.seed)
    };
    let first = session
        .plans
        .first()
        .ok_or_else(|| ReplayError::Refused("the session has no version 0".to_owned()))?;
    let mut engine =
        Engine::new(first, registry, config, Vec::new()).map_err(ReplayError::Engine)?;

    let mut next = 0usize;
    let mut next_outcome = 0usize;
    let mut refused = None;
    let mut mismatch = None;
    let block = config.block_capacity;
    let rendered = render_blocks_tapped(
        &mut engine,
        frames,
        block,
        |engine| {
            // Before this frame's swap: the mismatch names the plan that ran.
            take_mismatch(engine, &mut mismatch);
            engine.reclaim_externals().for_each(drop);
            engine.reclaim_edits().for_each(drop);
            engine.reclaim_boundaries();
            // The recorded outcomes of the next block, at most.
            let horizon = engine.next_frame().0.saturating_add(block as u64);
            while let Some(record) = script.get(next_outcome) {
                if record.frame.0 >= horizon {
                    break;
                }
                let outcome = match &record.outcome {
                    BoundaryKind::Missed(cause) => Scripted::Missed(*cause),
                    BoundaryKind::Delivered { words, fnv1a64 } => Scripted::Delivered {
                        words: words.as_ref().map(|w| w.clone().into_boxed_slice()),
                        fnv1a64: *fnv1a64,
                    },
                };
                if engine
                    .push_boundary(record.instance.clone(), record.frame, outcome)
                    .is_err()
                {
                    refused.get_or_insert_with(|| "the boundary queue is full".to_owned());
                }
                next_outcome += 1;
            }
            let frame = engine.next_frame();
            let end = next + session.edits[next..].partition_point(|e| e.frame <= frame);
            let batch = &session.edits[next..end];
            next = end;
            for edit in batch {
                if let Command::Swap { version } = edit.command {
                    let plan = usize::try_from(version)
                        .ok()
                        .and_then(|v| session.plans.get(v))
                        .ok_or(EngineError::StaleSwap)?;
                    let mut swap = engine.prepare_swap(plan, registry)?;
                    engine.commit_swap(&mut swap, frame)?;
                }
            }
            for edit in batch {
                let pushed = match &edit.command {
                    Command::Event { target, payload } => engine
                        .push_external(ExternalEvent {
                            frame: edit.frame,
                            sequence: edit.seq,
                            target: target.clone(),
                            payload: *payload,
                        })
                        .map_err(|e| e.to_string()),
                    Command::Edit { kind, address } => match kind {
                        EditKind::Suspend => engine.push_suspend(address.clone(), edit.frame),
                        EditKind::Resume => engine.push_resume(address.clone(), edit.frame),
                    }
                    .map_err(|e| e.to_string()),
                    Command::Swap { .. } => Ok(()),
                };
                if let Err(message) = pushed {
                    refused.get_or_insert(message);
                }
            }
            let next_edit = session.edits.get(next).map(|e| e.frame);
            let next_record = script.get(next_outcome).map(|r| r.frame);
            Ok([next_edit, next_record].into_iter().flatten().min())
        },
        &options.vectors,
    );
    // The first result that differs from the performance's is the cause of
    // whatever else went wrong after it (M2.7-8): it is reported first.
    take_mismatch(&mut engine, &mut mismatch);
    if let Some((module, block)) = mismatch {
        return Err(ReplayError::ReplayMismatch { module, block });
    }
    let (mut rendered, vectors) = rendered.map_err(ReplayError::Render)?;
    if let Some(message) = refused {
        return Err(ReplayError::Refused(message));
    }
    let discards = engine.input_discards();
    if discards != InputDiscards::default()
        || engine.input_overflows() != 0
        || engine.log_overflows() != 0
    {
        return Err(ReplayError::Discarded {
            discards,
            input_overflows: engine.input_overflows(),
            log_overflows: engine.log_overflows(),
        });
    }
    if engine.boundary_mismatches() != 0 {
        return Err(ReplayError::BoundaryMismatch {
            mismatches: engine.boundary_mismatches(),
        });
    }
    if engine.boundary_unreproduced() != 0 {
        return Err(ReplayError::Unreproduced {
            results: engine.boundary_unreproduced(),
        });
    }
    if mode == BoundaryReplay::Exact {
        rendered.conversion = Conversion::Device;
    }
    rendered.ideal = mode.ideal();
    rendered.trust_provenance = trusted;
    Ok(Replay {
        render: rendered,
        skipped: session.edits.len() - next,
        boundaries: mode,
        unverified,
        trusted_provenance: trusted,
        vectors,
    })
}

/// Keeps in `first` the engine's hash mismatch (M2.7-8), unless an earlier
/// one is kept already.
fn take_mismatch(engine: &mut Engine, first: &mut Option<(InstanceAddress, Frame)>) {
    if let Some((frame, address)) = engine.take_boundary_hash_mismatch()
        && first.is_none()
    {
        *first = Some((address, frame));
    }
}

/// What differs between the recording's build and machine and `here`
/// (M2.7-8), each as "what: recorded … here …"; empty when they agree. The
/// build is compared as the "another build" warning does, with the dirty
/// flag.
fn provenance_differences(build: &Provenance, machine: &Machine, here: &Here) -> Vec<String> {
    let dirty = |d: Option<bool>| d.map_or("unknown".to_owned(), |d| d.to_string());
    let mut differences = Vec::new();
    let mut compare = |what: &str, recorded: String, now: String| {
        if recorded != now {
            differences.push(format!("{what}: recorded {recorded}, here {now}"));
        }
    };
    compare(
        "revision",
        build.signal_revision.clone(),
        here.build.signal_revision.clone(),
    );
    compare(
        "dirty",
        dirty(build.signal_dirty),
        dirty(here.build.signal_dirty),
    );
    compare(
        "compiler",
        build.rustc_version.clone(),
        here.build.rustc_version.clone(),
    );
    compare(
        "Cargo.lock",
        build.cargo_lock_hash.clone(),
        here.build.cargo_lock_hash.clone(),
    );
    compare(
        "CPU architecture",
        machine.cpu_arch.clone(),
        here.machine.cpu_arch.clone(),
    );
    let devices = |devices: &[Device]| {
        if devices.is_empty() {
            return "none".to_owned();
        }
        devices
            .iter()
            .map(|d| {
                format!(
                    "{} {} (vendor {:#x}, device {:#x}, driver {} {})",
                    d.backend, d.name, d.vendor, d.device_id, d.driver, d.driver_info
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    if machine.devices != here.machine.devices {
        compare(
            "devices",
            devices(&machine.devices),
            devices(&here.machine.devices),
        );
    }
    differences
}
