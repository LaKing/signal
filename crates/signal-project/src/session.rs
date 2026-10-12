//! Session directories (`docs/session-format.md`): the initial project, every
//! plan version swapped in, and every command with its effective frame, as
//! text. [`SessionRecorder`] writes one; [`LiveRecorder`] feeds it from a
//! real-time [`Controller`](signal_engine::Controller); [`Session::load`] reads
//! and validates one; [`replay`] renders it offline through the same engine API
//! a live driver uses (ADR 0006, V10). Control-thread code: it allocates and
//! does I/O.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use signal_core::{
    Curve, Direction, EventPayload, Frame, InstanceAddress, ModuleDescriptor, PortAddress,
    PortShape, Registry, SinkKind, fnv1a64,
};
use signal_engine::{
    ControlSink, Engine, EngineConfig, EngineError, ExternalEvent, InputDiscards, Outcome,
    Provenance, Render, RenderError, Report, SentCommand, render_blocks,
};
use signal_graph::ExecutionPlan;

use crate::{Diagnostic, LoadError, Project, Severity, SourceFile};

/// The session format version this build reads and writes.
pub const FORMAT_VERSION: i64 = 1;

/// The session file in a session directory.
pub const SESSION_FILE: &str = "session.toml";

/// The command log in a session directory.
pub const EDITS_FILE: &str = "edits.jsonl";

/// The folder of plan versions (`projects/v<N>/`) in a session directory.
pub const PROJECTS_DIR: &str = "projects";

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
            state: State::new(&plan, 0),
            versions: vec![(initial.files.clone(), plan)],
            edits: Vec::new(),
            next_seq: 1,
            last_event: Frame::ZERO,
            last_edit: Frame::ZERO,
            last_swap: None,
            end: None,
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
    /// which the caller chose (a [`Controller`](signal_engine::Controller)'s
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

        let session = SessionOut {
            format_version: FORMAT_VERSION,
            sample_rate: self.sample_rate,
            seed: self.seed,
            frames: self.end,
            provenance: &self.provenance,
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
        fs::write(&path, log).map_err(io(&path))
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
/// [`Controller::poll`](signal_engine::Controller::poll) feeds (operator
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
pub struct LiveRecorder {
    recorder: SessionRecorder,
    registry: Registry,
    /// The project of every swap sent and not yet recorded, by plan version.
    swaps: BTreeMap<u64, Project>,
    pending: BTreeMap<(Frame, u8, u64), Applied>,
    refused: Vec<String>,
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
        }
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
    /// renders exactly the frames the run played.
    pub fn finish(&mut self, next_frame: Frame) -> Result<(), SessionError> {
        self.frontier(next_frame);
        self.recorder.set_end(next_frame.0)
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
    /// Every plan version's project, by version.
    pub versions: Vec<Project>,
    /// Every plan version's plan, by version, its `version` set to it.
    pub plans: Vec<ExecutionPlan>,
    /// The commands, in `edits.jsonl` order.
    pub edits: Vec<Edit>,
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

        Ok(Session {
            sample_rate: *file.sample_rate.get_ref(),
            seed: file.seed,
            frames: file.frames,
            provenance: file.provenance,
            versions,
            plans,
            edits: edits.into_iter().map(|(_, e)| e).collect(),
        })
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
/// [`Session::default_frames`]): builds the engine from version 0 and, at
/// each frame with commands, commits its swap and pushes its other commands
/// with `push_external`, `push_suspend` and `push_resume`, as a live driver
/// does. It renders in blocks split at those frames (`docs/semantics.md` §13).
///
/// The input queues are sized for the session's busiest frame (queue capacity
/// is not part of the semantics, `docs/semantics.md` §11), and reclaimed
/// before every block. Any discarded input or overflow afterwards is an error.
pub fn replay(
    session: &Session,
    registry: &Registry,
    frames: Option<u64>,
) -> Result<Replay, ReplayError> {
    let frames = frames.unwrap_or_else(|| session.default_frames());
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
    let config = EngineConfig {
        queue_capacity: busiest
            .unwrap_or(0)
            .max(EngineConfig::DEFAULT_QUEUE_CAPACITY),
        ..EngineConfig::new(session.sample_rate, session.seed)
    };
    let first = session
        .plans
        .first()
        .ok_or_else(|| ReplayError::Refused("the session has no version 0".to_owned()))?;
    let mut engine =
        Engine::new(first, registry, config, Vec::new()).map_err(ReplayError::Engine)?;

    let mut next = 0usize;
    let mut refused = None;
    let block = config.block_capacity;
    let rendered = render_blocks(&mut engine, frames, block, |engine| {
        engine.reclaim_externals().for_each(drop);
        engine.reclaim_edits().for_each(drop);
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
        Ok(session.edits.get(next).map(|e| e.frame))
    })
    .map_err(ReplayError::Render)?;
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
    Ok(Replay {
        render: rendered,
        skipped: session.edits.len() - next,
    })
}
