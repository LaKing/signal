//! Live edits from project files (ADR 0006, D17): [`ProjectWatcher`] notices
//! that a project folder's files changed, and [`send_edit`] turns a changed
//! project into commands on a real-time [`Controller`], recorded through a
//! [`LiveRecorder`]. Control-thread code: it allocates and does I/O.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use signal_core::{Frame, PortAddress, Registry};
use signal_engine::{
    BoundaryLine, ControlSink, HangLine, LogLine, ParameterValue, Report, RunnerStatus,
    SentCommand, Stamp,
};
use signal_output::Controller;

use crate::session::{LiveRecorder, sink_of};
use crate::{Diagnostic, Project, ProjectEdit, diff};

/// How often `--watch` polls a project folder.
pub const WATCH_INTERVAL_MS: u64 = 100;

/// Each file of a folder (relative path) with its modification time and length.
type Snapshot = BTreeMap<PathBuf, (Option<SystemTime>, u64)>;

/// Notices changes to the files of a project folder by polling their
/// modification times and lengths (no file-system events, no new crates).
/// Hidden files and folders are ignored.
pub struct ProjectWatcher {
    dir: PathBuf,
    last: Snapshot,
}

impl ProjectWatcher {
    /// Watches `dir`, as it is now.
    pub fn new(dir: &Path) -> io::Result<ProjectWatcher> {
        Ok(ProjectWatcher {
            dir: dir.to_owned(),
            last: snapshot(dir)?,
        })
    }

    /// Whether a file was added, removed or changed since the last poll.
    pub fn poll(&mut self) -> io::Result<bool> {
        let now = snapshot(&self.dir)?;
        let changed = now != self.last;
        self.last = now;
        Ok(changed)
    }

    /// `file` (relative to the folder) as it is now, for [`accept`](Self::accept).
    pub fn stamp(&self, file: &Path) -> FileStamp {
        FileStamp(
            fs::symlink_metadata(self.dir.join(file))
                .ok()
                .filter(|metadata| metadata.is_file())
                .map(|metadata| (metadata.modified().ok(), metadata.len())),
        )
    }

    /// Takes `file` as `stamp` found it as already seen: for a write whose
    /// content the running project already holds, so the next
    /// [`poll`](Self::poll) does not report it. A change after the stamp, and
    /// every other file's, is still reported.
    pub fn accept(&mut self, file: &Path, stamp: FileStamp) {
        match stamp.0 {
            Some(state) => self.last.insert(file.to_owned(), state),
            None => self.last.remove(file),
        };
    }
}

/// A project folder's files as [`ProjectWatcher`] sees them (each one's
/// modification time and length), less one ignored file: equal states mean
/// nothing a load reads changed in between. For caching what a load gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderState(Snapshot);

impl FolderState {
    /// `dir` as it is now, without `ignore` (relative to the folder).
    pub fn read(dir: &Path, ignore: &Path) -> io::Result<FolderState> {
        let mut files = snapshot(dir)?;
        files.remove(ignore);
        Ok(FolderState(files))
    }
}

/// A file's modification time and length as [`ProjectWatcher::stamp`] read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp(Option<(Option<SystemTime>, u64)>);

fn snapshot(dir: &Path) -> io::Result<Snapshot> {
    let mut files = Snapshot::new();
    let mut folders = vec![dir.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                folders.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap_or(&path).to_owned();
                files.insert(relative, (metadata.modified().ok(), metadata.len()));
            }
        }
    }
    Ok(files)
}

/// What [`send_edit`] sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Parameter `set`s (or ramps) sent.
    pub sets: usize,
    /// The plan version swapped to, if the edit was structural.
    pub swap: Option<u64>,
    /// Instances suspended for removal before the swap.
    pub removed: usize,
    /// Slots whose content was replaced (ADR 0037 §6): the swap above
    /// unloaded them, and [`load_swap`](Applied::load_swap) loaded the new
    /// content a frame later.
    pub replaced: usize,
    /// A Replace's second swap, which loads the new content.
    pub load_swap: Option<u64>,
    /// Commands refused after a swap went out (only a desynchronised
    /// Controller refuses then): what followed it was not sent, and the
    /// project returned as running is the last one swapped in. For a
    /// Replace whose load did not go out, the slots play empty.
    pub unfinished: bool,
}

/// What `--watch` reports: `applied a swap to version 3 (1 sets, 0 removed)`,
/// `applied 2 sets`, or `no change to apply`.
impl fmt::Display for Applied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.swap, self.load_swap) {
            (Some(version), Some(load)) => write!(
                f,
                "replaced {} slot content{}: unloaded by a swap to version {version}, loaded by a swap to version {load} ({} sets, {} removed)",
                self.replaced,
                if self.replaced == 1 { "" } else { "s" },
                self.sets,
                self.removed
            ),
            (Some(version), None) if self.unfinished && self.replaced > 0 => write!(
                f,
                "unloaded {} slot{} by a swap to version {version}, but the load could not be sent: the slot plays empty ({} sets, {} removed)",
                self.replaced,
                if self.replaced == 1 { "" } else { "s" },
                self.sets,
                self.removed
            ),
            (Some(version), _) if self.unfinished => write!(
                f,
                "applied a swap to version {version} ({} sets, {} removed), but the commands after it could not be sent",
                self.sets, self.removed
            ),
            (Some(version), _) => write!(
                f,
                "applied a swap to version {version} ({} sets, {} removed)",
                self.sets, self.removed
            ),
            (None, _) if self.sets == 0 => f.write_str("no change to apply"),
            (None, _) => write!(f, "applied {} sets", self.sets),
        }
    }
}

/// Why [`send_edit`] sent nothing (or not everything).
#[derive(Debug)]
pub enum EditError {
    /// The new project cannot replace the running one: its plan is invalid,
    /// it changes the seed or audio sink (a changed sample rate is no error,
    /// ADR 0036).
    Invalid(Vec<Diagnostic>),
    /// The Controller refused a command (a full queue, a refused swap).
    Refused(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::Invalid(diagnostics) => {
                for (i, diagnostic) in diagnostics.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{diagnostic}")?;
                }
                Ok(())
            }
            EditError::Refused(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EditError {}

/// Sends the edit that turns the running `old` project into `new` (M2-2
/// [`diff`]) through `controller`, recorded by `live`, and returns the project
/// now running.
///
/// Parameter changes are `set`s stamped [`Stamp::Default`]. A structural
/// change is a swap at the default frame with the survivors' changed
/// parameters stamped for the same frame; removed instances are first
/// suspended there and the swap follows at the removal frame
/// (`docs/semantics.md` §12, "Removal"). The Controller is polled first, so
/// the default frame is current. Nothing is sent when the new project cannot
/// replace the old one, when its swap cannot be prepared
/// ([`Controller::prepare_swap`]), or when the Controller cannot take every
/// command of the edit, and the resumes that would undo its suspends, now
/// ([`Controller::available`]).
pub fn send_edit(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    old: &Project,
    new: Project,
    registry: &Registry,
) -> Result<(Project, Applied), EditError> {
    send_edit_observed(controller, live, &mut (), old, new, registry)
}

/// Both sinks of one poll: everything goes to the first, then the second.
pub struct Both<'a, A, B>(pub &'a mut A, pub &'a mut B);

impl<A: ControlSink, B: ControlSink> ControlSink for Both<'_, A, B> {
    fn report(&mut self, report: &Report, command: &SentCommand) {
        self.0.report(report, command);
        self.1.report(report, command);
    }
    fn log(&mut self, line: LogLine<'_>) {
        self.0.log(line);
        self.1.log(line);
    }
    fn parameter(&mut self, port: &PortAddress, value: ParameterValue) {
        self.0.parameter(port, value);
        self.1.parameter(port, value);
    }
    fn status(&mut self, status: &RunnerStatus) {
        self.0.status(status);
        self.1.status(status);
    }
    fn boundary(&mut self, line: BoundaryLine<'_>) {
        self.0.boundary(line);
        self.1.boundary(line);
    }
    fn hang(&mut self, line: HangLine<'_>) {
        self.0.hang(line);
        self.1.hang(line);
    }
    fn frontier(&mut self, next_frame: Frame) {
        self.0.frontier(next_frame);
        self.1.frontier(next_frame);
    }
}

/// A structural edit keeps the running project's audio sink: the same
/// instance with as many inputs (`old` planned as `before`, `new` as `plan`).
fn same_sink(
    old: &Project,
    before: &signal_graph::ExecutionPlan,
    new: &Project,
    plan: &signal_graph::ExecutionPlan,
) -> Result<(), Vec<crate::Diagnostic>> {
    let sink = sink_of(old, before)?;
    let after = sink_of(new, plan)?;
    if after != sink {
        return Err(vec![new.instance_error(
            &after.0,
            format!(
                "{}: the audio sink must stay {} with {} inputs while the project plays; restart to change it",
                after.0, sink.0, sink.1
            ),
        )]);
    }
    Ok(())
}

/// Whether `new`, planned as `planned`, could replace the running `old` by a
/// live edit, with the same checks as [`send_edit`] (the diff, and the audio
/// sink kept by a structural edit), without sending anything: what a patch
/// is checked with before it is written (M2.9-3). `running` is `old`'s plan
/// when the caller holds it (`old` is planned again otherwise).
pub fn check_edit(
    old: &Project,
    running: Option<&signal_graph::ExecutionPlan>,
    new: &Project,
    planned: signal_graph::ExecutionPlan,
    registry: &Registry,
) -> Result<(), Vec<crate::Diagnostic>> {
    crate::diff::restart(old, new)?;
    let planned_old;
    let before = match running {
        Some(plan) => plan,
        None => {
            planned_old = old.plan(registry)?.plan;
            &planned_old
        }
    };
    let options = crate::DiffOptions::default();
    match crate::diff::diff_plans(old, before, new, planned, &options)? {
        ProjectEdit::Parameters(_) => Ok(()),
        ProjectEdit::Structural { plan, .. } => same_sink(old, before, new, &plan),
    }
}

/// [`send_edit`], with the Controller's polls during the edit also handed to
/// `observer` (a UI's acknowledgements and values, M2.8-1).
///
/// **A Replace** (ADR 0037 §6): when a slot of the project's own files
/// changes what it loads (another canonical `load`, both loaded), the edit is
/// sent as an unload, then a load: the old content is suspended at *s*, the
/// plan with those slots empty (as `slot.unload` writes them, the rest of
/// the edit included) is swapped in at *s* + *F*, and the new project at
/// *s* + *F* + 1, so the old content fades out to +0.0, the new one fades in,
/// and no state carries over, even between instances whose address and type
/// the two contents share. Both swaps are prepared before anything is sent
/// ([`Controller::prepare_swap_after`]), and the Controller must take every
/// command of both.
pub fn send_edit_observed(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    observer: &mut impl ControlSink,
    old: &Project,
    new: Project,
    registry: &Registry,
) -> Result<(Project, Applied), EditError> {
    let edit = diff(old, &new, registry).map_err(EditError::Invalid)?;
    let refused = |error: signal_engine::ControlError| EditError::Refused(error.to_string());
    let mut applied = Applied::default();
    // A Replace is one even when the two contents plan alike but for their
    // values (the same instances, other parameters): no state carries over.
    let replaced = replaced_slots(old, &new, registry);
    match edit {
        ProjectEdit::Parameters(changes) if replaced.is_empty() => {
            controller.poll(&mut Both(live, observer));
            room(controller, changes.len())?;
            for change in changes {
                controller
                    .send_event(change.port, change.payload, Stamp::Default)
                    .map_err(refused)?;
                applied.sets += 1;
            }
            Ok((new, applied))
        }
        edit => {
            let before = old.plan(registry).map_err(EditError::Invalid)?.plan;
            let steps = if let ProjectEdit::Structural {
                plan,
                removed,
                sets,
                ..
            } = edit
                && replaced.is_empty()
            {
                same_sink(old, &before, &new, &plan).map_err(EditError::Invalid)?;
                vec![Step {
                    project: new,
                    removed,
                    sets,
                }]
            } else {
                // Unload, then load: the slots emptied first, with the rest
                // of the edit; then their new content, which only adds.
                let empty = new.with_slots_emptied(&replaced);
                let unload = diff(old, &empty, registry).map_err(EditError::Invalid)?;
                let load = diff(&empty, &new, registry).map_err(EditError::Invalid)?;
                let mut steps = Vec::new();
                for (project, edit) in [(empty, unload), (new, load)] {
                    match edit {
                        ProjectEdit::Structural {
                            plan,
                            removed,
                            sets,
                            ..
                        } => {
                            same_sink(old, &before, &project, &plan).map_err(EditError::Invalid)?;
                            steps.push(Step {
                                project,
                                removed,
                                sets,
                            });
                        }
                        // Nothing to swap (content without instances): its
                        // sets, if any, go with the step before.
                        ProjectEdit::Parameters(changes) => match steps.last_mut() {
                            Some(last) => {
                                last.project = project;
                                last.sets.extend(changes);
                            }
                            None => steps.push(Step {
                                project,
                                removed: Vec::new(),
                                sets: changes,
                            }),
                        },
                    }
                }
                if steps.iter().skip(1).any(|s| !s.removed.is_empty()) {
                    return Err(EditError::Refused(
                        "a Replace's load would remove instances; nothing was sent".to_owned(),
                    ));
                }
                applied.replaced = replaced.len();
                steps
            };
            send_swaps(controller, live, observer, steps, &mut applied)
        }
    }
}

/// One swap of a structural edit: the project it plays, the instances it
/// removes (suspended first) and the survivors' changed parameters.
struct Step<P = Project> {
    project: P,
    removed: Vec<signal_core::InstanceAddress>,
    sets: Vec<crate::ParameterChange>,
}

/// The slots of `new`'s own files whose content differs from `old`'s, both
/// loaded: a Replace each (ADR 0037 §6). Another `load` whose content plans
/// alike (the same instances, types, parameters and wiring under the slot,
/// as an own copy of it under another name does) is no Replace: the edit is
/// an ordinary one, which changes nothing that plays.
fn replaced_slots(old: &Project, new: &Project, registry: &Registry) -> Vec<crate::slot::SlotKey> {
    let before = old.slot_contents();
    new.slot_contents()
        .into_iter()
        .filter(|(key, content)| before.get(key).is_some_and(|c| c != content))
        .map(|(key, _)| key)
        .filter(|key| {
            let (was, now) = (
                old.slot_content_plan(key, registry),
                new.slot_content_plan(key, registry),
            );
            was.is_none() || was != now
        })
        .collect()
}

/// Sends `steps` (one swap each; only the first may remove instances), the
/// first at the default frame, after its removals' fade, each next one a
/// frame after the one before. Nothing is sent unless every swap is
/// prepared and the Controller can take every command, and the resumes
/// that would undo the suspends, now.
fn send_swaps(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    observer: &mut impl ControlSink,
    steps: Vec<Step>,
    applied: &mut Applied,
) -> Result<(Project, Applied), EditError> {
    let removed = steps
        .first()
        .map_or(&[][..], |s| s.removed.as_slice())
        .to_vec();
    // Nothing is sent unless the whole edit can complete: a removal
    // stopped half-way would leave instances suspended in the plan that
    // keeps playing. So the swaps are prepared first (every refusal but a
    // full queue happens there), and the batch (suspends, swaps, sets) and
    // the resumes that would undo the suspends must all fit in the queue.
    let commands = 2 * removed.len() + steps.iter().map(|s| 1 + s.sets.len()).sum::<usize>();
    controller.poll(&mut Both(live, observer));
    room(controller, commands)?;
    let unsend = |live: &mut LiveRecorder, count: usize| {
        for _ in 0..count {
            live.swap_not_sent();
        }
    };
    // The projects that play, kept as planned: re-reading one from its files
    // in memory would lose what it includes (ADR 0035).
    let mut planned = Vec::new();
    let mut readies: Vec<signal_output::ReadySwap> = Vec::new();
    for step in &steps {
        let plan = match live.swap_plan(step.project.clone()) {
            Ok(plan) => plan,
            Err(error) => {
                unsend(live, planned.len());
                return Err(EditError::Refused(error.to_string()));
            }
        };
        let ready = match readies.last() {
            None => controller.prepare_swap(&plan),
            Some(previous) => controller.prepare_swap_after(previous, &plan),
        };
        planned.push(plan.version);
        match ready {
            Ok(ready) => readies.push(ready),
            Err(error) => {
                unsend(live, planned.len());
                return Err(EditError::Refused(format!("{error}; nothing was sent")));
            }
        }
    }
    // Preparing can take long while the runner plays on: poll again so the
    // frames below come from a fresh status, not one stale by the
    // preparation (else the suspend and the swap could both be late and
    // land on one frame, skipping the removal's fade).
    controller.poll(&mut Both(live, observer));
    if let Err(error) = room(controller, commands) {
        unsend(live, planned.len());
        return Err(error);
    }

    let start = controller.default_frame();
    let mut unsent = |count: usize| unsend(live, count);
    let playing = send_prepared(
        controller,
        &mut unsent,
        start,
        steps.into_iter().zip(readies).zip(planned).collect(),
        applied,
    )?;
    Ok((playing, *applied))
}

/// A step with its prepared swap and the plan version it records.
type Prepared<P, R> = ((Step<P>, R), u64);

/// What sends an edit's commands: the Controller, or a test's stand-in.
trait Sender {
    /// A prepared swap.
    type Ready;
    fn suspend(&mut self, address: signal_core::InstanceAddress, stamp: Stamp) -> Sent;
    fn resume(&mut self, address: signal_core::InstanceAddress, stamp: Stamp) -> Sent;
    fn swap(&mut self, ready: Self::Ready, stamp: Stamp) -> Sent;
    fn event(&mut self, change: crate::ParameterChange, stamp: Stamp) -> Sent;
    fn removal_swap_frame(&self, suspend: Frame) -> Frame;
}

type Sent = Result<(), signal_engine::ControlError>;

impl Sender for Controller {
    type Ready = signal_output::ReadySwap;
    fn suspend(&mut self, address: signal_core::InstanceAddress, stamp: Stamp) -> Sent {
        self.send_suspend(address, stamp).map(|_| ())
    }
    fn resume(&mut self, address: signal_core::InstanceAddress, stamp: Stamp) -> Sent {
        self.send_resume(address, stamp).map(|_| ())
    }
    fn swap(&mut self, ready: Self::Ready, stamp: Stamp) -> Sent {
        self.send_swap(ready, stamp).map(|_| ())
    }
    fn event(&mut self, change: crate::ParameterChange, stamp: Stamp) -> Sent {
        self.send_event(change.port, change.payload, stamp)
            .map(|_| ())
    }
    fn removal_swap_frame(&self, suspend: Frame) -> Frame {
        std::ops::Deref::deref(self).removal_swap_frame(suspend)
    }
}

/// Sends prepared `steps` (each with its swap and the plan version it
/// records): the first step's removals suspended at `start`, its swap after
/// their fade (or at `start`), each next swap a frame after the one before,
/// each step's sets at its swap's frame. Returns what plays once they apply.
///
/// Until the first swap is sent, a refusal sends nothing more: the suspends
/// sent are resumed, every swap is unsent (`unsend`), and the edit is an
/// error, the running project unchanged. Once a swap has gone out, it plays
/// whatever follows: a later refusal (a set, the next swap) stops there,
/// unsends the swaps not sent, marks the edit `unfinished`, and returns the
/// project of the last swap sent, so the caller's running project is the
/// engine's (the queue was counted first, so only a desynchronised
/// Controller refuses then).
fn send_prepared<S: Sender, P>(
    sender: &mut S,
    unsend: &mut dyn FnMut(usize),
    start: Frame,
    steps: Vec<Prepared<P, S::Ready>>,
    applied: &mut Applied,
) -> Result<P, EditError> {
    let refused = |error: signal_engine::ControlError| EditError::Refused(error.to_string());
    let count = steps.len();
    let removed = steps
        .first()
        .map_or(&[][..], |((s, _), _)| s.removed.as_slice())
        .to_vec();
    let resume_all = |sender: &mut S, upto: usize| {
        for address in &removed[..upto] {
            let _ = sender.resume(address.clone(), Stamp::At(Frame(start.0 + 1)));
        }
    };
    for (i, address) in removed.iter().enumerate() {
        if let Err(error) = sender.suspend(address.clone(), Stamp::At(start)) {
            unsend(count);
            resume_all(sender, i);
            return Err(refused(error));
        }
        applied.removed += 1;
    }
    let mut frame = if removed.is_empty() {
        start
    } else {
        sender.removal_swap_frame(start)
    };
    let mut playing = None;
    'steps: for (i, ((step, ready), version)) in steps.into_iter().enumerate() {
        if let Err(error) = sender.swap(ready, Stamp::At(frame)) {
            unsend(count - i);
            if i == 0 {
                resume_all(sender, removed.len());
                return Err(refused(error));
            }
            // A Replace whose unload went out and whose load did not: the
            // slot plays empty, which is what runs now.
            applied.unfinished = true;
            break;
        }
        if i == 0 {
            applied.swap = Some(version);
        } else {
            applied.load_swap = Some(version);
        }
        playing = Some(step.project);
        for change in step.sets {
            if sender.event(change, Stamp::At(frame)).is_err() {
                // The swap plays; what follows it does not go out.
                unsend(count - i - 1);
                applied.unfinished = true;
                break 'steps;
            }
            applied.sets += 1;
        }
        frame = Frame(frame.0.saturating_add(1));
    }
    playing.ok_or_else(|| EditError::Refused("nothing was sent".to_owned()))
}

/// Why [`reload`] kept the running project.
#[derive(Debug)]
pub enum ReloadError {
    /// The folder's project does not load, or cannot replace the running one;
    /// the diagnostics' files are joined to the folder ([`located`](crate::located)).
    Invalid(Vec<Diagnostic>),
    /// The folder could not be read, or the Controller refused the edit.
    Failed(String),
}

/// What `--watch` (and Signal.app) does when a project folder changed: loads
/// `dir` and sends the edit from `current` to it ([`send_edit`]). On success
/// `current` is the new project; on an error nothing changed and the last
/// good version keeps playing.
pub fn reload(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    current: &mut Project,
    dir: &Path,
    registry: &Registry,
) -> Result<Applied, ReloadError> {
    reload_observed(controller, live, &mut (), current, dir, registry)
}

/// [`reload`], with the Controller's polls during the edit also handed to
/// `observer` ([`send_edit_observed`]).
pub fn reload_observed(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    observer: &mut impl ControlSink,
    current: &mut Project,
    dir: &Path,
    registry: &Registry,
) -> Result<Applied, ReloadError> {
    let new = match Project::load(dir) {
        Ok(new) => new,
        Err(crate::LoadError::Invalid(diagnostics)) => {
            return Err(ReloadError::Invalid(crate::located(dir, diagnostics)));
        }
        Err(error) => return Err(ReloadError::Failed(error.to_string())),
    };
    match send_edit_observed(controller, live, observer, current, new, registry) {
        Ok((project, applied)) => {
            *current = project;
            Ok(applied)
        }
        Err(EditError::Invalid(diagnostics)) => {
            Err(ReloadError::Invalid(crate::located(dir, diagnostics)))
        }
        Err(EditError::Refused(message)) => Err(ReloadError::Failed(message)),
    }
}

/// Refuses an edit of `commands` commands that the Controller cannot take
/// whole now; nothing is sent.
fn room(controller: &Controller, commands: usize) -> Result<(), EditError> {
    let available = controller.available();
    if commands <= available {
        return Ok(());
    }
    let in_flight = controller.in_flight();
    Err(EditError::Refused(format!(
        "the edit needs {commands} commands but the command queue has room for {available} \
         ({in_flight} in flight, at most {}); nothing was sent. Save again once the \
         commands in flight have applied, or remove fewer instances at once (one save \
         can remove at most about half the queue: each removal also reserves its resume)",
        available + in_flight
    )))
}

#[cfg(test)]
mod sending {
    //! `send_prepared`'s refusals, with a stand-in sender that refuses its
    //! `fail_at`-th command: before the first swap nothing is left behind,
    //! after it the edit is `unfinished` and what plays is returned.
    use super::*;
    use signal_core::{EventPayload, InstanceAddress};
    use signal_engine::ControlError;

    #[derive(Default)]
    struct Fake {
        fail_at: usize,
        failed: bool,
        sent: Vec<String>,
    }

    impl Fake {
        fn send(&mut self, what: String) -> Sent {
            if self.sent.len() + 1 == self.fail_at && !self.failed {
                self.failed = true;
                return Err(ControlError::Desynchronised);
            }
            self.sent.push(what);
            Ok(())
        }
    }

    impl Sender for Fake {
        type Ready = &'static str;
        fn suspend(&mut self, address: InstanceAddress, stamp: Stamp) -> Sent {
            self.send(format!("suspend {address} {stamp:?}"))
        }
        fn resume(&mut self, address: InstanceAddress, stamp: Stamp) -> Sent {
            self.send(format!("resume {address} {stamp:?}"))
        }
        fn swap(&mut self, ready: &'static str, stamp: Stamp) -> Sent {
            self.send(format!("swap {ready} {stamp:?}"))
        }
        fn event(&mut self, change: crate::ParameterChange, stamp: Stamp) -> Sent {
            self.send(format!("set {} {stamp:?}", change.port))
        }
        fn removal_swap_frame(&self, suspend: Frame) -> Frame {
            Frame(suspend.0 + 240)
        }
    }

    /// A Replace: `/x` removed, the unload with one set, the load with one.
    fn replace() -> Vec<((Step<&'static str>, &'static str), u64)> {
        let set = |port: &str| crate::ParameterChange {
            port: port.parse().unwrap(),
            payload: EventPayload::Set(1.0),
        };
        vec![
            (
                (
                    Step {
                        project: "empty",
                        removed: vec!["/x".parse().unwrap()],
                        sets: vec![set("/a.p")],
                    },
                    "unload",
                ),
                1,
            ),
            (
                (
                    Step {
                        project: "loaded",
                        removed: Vec::new(),
                        sets: vec![set("/b.p")],
                    },
                    "load",
                ),
                2,
            ),
        ]
    }

    /// Runs the Replace with the `fail_at`-th command refused: the result,
    /// what `Applied` says, the commands sent and how many swaps were
    /// unsent.
    fn run(fail_at: usize) -> (Result<&'static str, String>, Applied, Vec<String>, usize) {
        let mut fake = Fake {
            fail_at,
            ..Fake::default()
        };
        let mut unsent = 0;
        let mut applied = Applied::default();
        let result = send_prepared(
            &mut fake,
            &mut |n| unsent += n,
            Frame(1000),
            replace(),
            &mut applied,
        )
        .map_err(|e| e.to_string());
        (result, applied, fake.sent, unsent)
    }

    #[test]
    fn everything_sent_plays_the_load() {
        let (result, applied, sent, unsent) = run(0);
        assert_eq!(result, Ok("loaded"));
        assert_eq!(
            sent,
            [
                "suspend /x At(Frame(1000))",
                "swap unload At(Frame(1240))",
                "set /a.p At(Frame(1240))",
                "swap load At(Frame(1241))",
                "set /b.p At(Frame(1241))",
            ]
        );
        assert_eq!((applied.swap, applied.load_swap), (Some(1), Some(2)));
        assert!(!applied.unfinished);
        assert_eq!(unsent, 0);
    }

    #[test]
    fn a_refusal_before_the_first_swap_leaves_nothing_behind() {
        // The unload swap refused: the suspend is resumed, both unsent.
        let (result, applied, sent, unsent) = run(2);
        assert!(result.is_err());
        assert_eq!(
            sent,
            ["suspend /x At(Frame(1000))", "resume /x At(Frame(1001))"]
        );
        assert_eq!(unsent, 2);
        assert_eq!(applied.swap, None);
    }

    #[test]
    fn a_refusal_after_the_unload_plays_the_unload_unfinished() {
        // The unload's set refused: the unload plays, the load is unsent.
        let (result, applied, sent, unsent) = run(3);
        assert_eq!(result, Ok("empty"));
        assert_eq!(sent.len(), 2);
        assert_eq!(unsent, 1);
        assert!(applied.unfinished);
        assert_eq!((applied.swap, applied.load_swap), (Some(1), None));
        // The load swap refused: the same.
        let (result, applied, _, unsent) = run(4);
        assert_eq!(result, Ok("empty"));
        assert_eq!(unsent, 1);
        assert!(applied.unfinished);
        let shown = Applied {
            replaced: 1,
            ..applied
        }
        .to_string();
        assert!(shown.contains("the slot plays empty"), "{shown}");
        // The load's set refused: the load plays, nothing left to unsend.
        let (result, applied, _, unsent) = run(5);
        assert_eq!(result, Ok("loaded"));
        assert_eq!(unsent, 0);
        assert!(applied.unfinished);
        assert_eq!(applied.load_swap, Some(2));
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectWatcher;
    use std::fs;
    use std::path::Path;

    #[test]
    fn the_watcher_notices_changed_and_added_files_only() {
        let dir = std::env::temp_dir().join(format!("signal-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("composites")).unwrap();
        fs::write(dir.join("project.toml"), "a").unwrap();
        let mut watcher = ProjectWatcher::new(&dir).unwrap();
        assert!(!watcher.poll().unwrap(), "nothing changed");
        fs::write(dir.join("project.toml"), "ab").unwrap();
        assert!(watcher.poll().unwrap(), "a changed file");
        assert!(!watcher.poll().unwrap());
        fs::write(dir.join("composites/voice.toml"), "x").unwrap();
        assert!(watcher.poll().unwrap(), "an added file");
        fs::write(dir.join(".hidden"), "x").unwrap();
        assert!(!watcher.poll().unwrap(), "hidden files are ignored");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_accepted_write_is_not_reported_but_other_changes_are() {
        let dir = std::env::temp_dir().join(format!("signal-accept-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("composites")).unwrap();
        fs::write(dir.join("project.toml"), "a").unwrap();
        let mut watcher = ProjectWatcher::new(&dir).unwrap();
        fs::write(dir.join("project.toml"), "ab").unwrap();
        let stamp = watcher.stamp(Path::new("project.toml"));
        watcher.accept(Path::new("project.toml"), stamp);
        assert!(!watcher.poll().unwrap(), "the accepted write");
        fs::write(dir.join("composites/voice.toml"), "x").unwrap();
        fs::write(dir.join("project.toml"), "abc").unwrap();
        let stamp = watcher.stamp(Path::new("project.toml"));
        watcher.accept(Path::new("project.toml"), stamp);
        assert!(watcher.poll().unwrap(), "another file changed meanwhile");
        fs::write(dir.join("project.toml"), "abcd").unwrap();
        let stamp = watcher.stamp(Path::new("project.toml"));
        fs::write(dir.join("project.toml"), "abcde").unwrap();
        watcher.accept(Path::new("project.toml"), stamp);
        assert!(watcher.poll().unwrap(), "a change after the stamp");
        fs::remove_dir_all(&dir).unwrap();
    }
}
