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
    /// ADR 0035 §7: the folders of the projects it includes outside its own
    /// folder (`../studio`), each by the path the host reaches it by, as
    /// they were at the last poll.
    includes: Vec<(PathBuf, PathBuf, Snapshot)>,
}

impl ProjectWatcher {
    /// Watches `dir`, as it is now, and the folders of the projects it
    /// includes outside it.
    pub fn new(dir: &Path) -> io::Result<ProjectWatcher> {
        let includes = include_dirs(dir)
            .into_iter()
            .map(|(shown, folder)| {
                let files = snapshot(&folder).unwrap_or_default();
                (shown, folder, files)
            })
            .collect();
        Ok(ProjectWatcher {
            dir: dir.to_owned(),
            last: snapshot(dir)?,
            includes,
        })
    }

    /// Whether a file was added, removed or changed since the last poll.
    pub fn poll(&mut self) -> io::Result<bool> {
        Ok(!self.changes()?.is_empty())
    }

    /// The files (relative to the folder, in path order) added, removed or
    /// changed since the last poll: empty when nothing changed. A caller
    /// tells a change that can alter what plays from one that cannot with
    /// [`is_presentation`].
    pub fn changes(&mut self) -> io::Result<Vec<PathBuf>> {
        let now = snapshot(&self.dir)?;
        let mut changed: Vec<PathBuf> = now
            .iter()
            .filter(|(path, state)| self.last.get(*path) != Some(*state))
            .map(|(path, _)| path.clone())
            .chain(
                self.last
                    .keys()
                    .filter(|path| !now.contains_key(*path))
                    .cloned(),
            )
            .collect();
        // A changed definition may include another project: the folders
        // watched are read again (each one's files as they are now).
        let definition = |p: &Path| {
            p.as_os_str().is_empty()
                || p == Path::new(crate::PROJECT_FILE)
                || p.starts_with(crate::COMPOSITES)
        };
        if changed.iter().any(|p| definition(p)) {
            self.rediscover();
        }
        // ADR 0035 §7: an included project's files, by the path the host
        // reaches them by (`../studio/patterns/drums.toml`).
        let mut included_definition = false;
        for (shown, folder, last) in &mut self.includes {
            let now = snapshot(folder).unwrap_or_default();
            let found: Vec<PathBuf> = now
                .iter()
                .filter(|(path, state)| last.get(*path) != Some(*state))
                .map(|(path, _)| path.clone())
                .chain(last.keys().filter(|path| !now.contains_key(*path)).cloned())
                .collect();
            included_definition |= found.iter().any(|p| definition(p));
            changed.extend(found.into_iter().map(|path| under(shown, path)));
            *last = now;
        }
        // An include's own definition changed (the audit of 2026-11-14): it
        // may include another project, watched from now on.
        if included_definition {
            self.rediscover();
        }
        changed.sort();
        self.last = now;
        Ok(changed)
    }

    /// The included folders read again ([`include_dirs`]), each known one
    /// keeping its last snapshot, a new one taken as it is now.
    fn rediscover(&mut self) {
        let known: BTreeMap<PathBuf, Snapshot> = self
            .includes
            .drain(..)
            .map(|(shown, _, files)| (shown, files))
            .collect();
        self.includes = include_dirs(&self.dir)
            .into_iter()
            .map(|(shown, folder)| {
                let files = known
                    .get(&shown)
                    .cloned()
                    .unwrap_or_else(|| snapshot(&folder).unwrap_or_default());
                (shown, folder, files)
            })
            .collect();
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

/// Whether `file` (relative to the project folder) is presentation only: a
/// file under `ui/` (the panels, their styles and scripts, the schematic's
/// positions, the window layout). No load reads one, so a change to it
/// cannot change what plays: the live reload is not run for it (the UI is
/// still told). Every other file counts as one a load may read.
pub fn is_presentation(file: &Path) -> bool {
    file.starts_with("ui")
}

/// A project folder's files as [`ProjectWatcher`] sees them (each one's
/// modification time and length), less the presentation files
/// ([`is_presentation`]): equal states mean nothing a load reads changed in
/// between. For caching what a load gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderState(Snapshot);

impl FolderState {
    /// `dir` as it is now, without its presentation files.
    pub fn read(dir: &Path) -> io::Result<FolderState> {
        let mut files = snapshot(dir)?;
        files.retain(|path, _| !is_presentation(path));
        // ADR 0035 §7: what a load reads of the projects it includes outside it.
        for (shown, folder) in include_dirs(dir) {
            for (path, state) in snapshot(&folder).unwrap_or_default() {
                if !is_presentation(&path) {
                    files.insert(under(&shown, path), state);
                }
            }
        }
        Ok(FolderState(files))
    }
}

/// A file's modification time and length as [`ProjectWatcher::stamp`] read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp(Option<(Option<SystemTime>, u64)>);

/// `path`, a file of the include the host reaches by `shown`, as the host
/// names it: `shown` itself for a `.signal` include's file (the empty path).
fn under(shown: &Path, path: PathBuf) -> PathBuf {
    if path.as_os_str().is_empty() {
        shown.to_owned()
    } else {
        shown.join(path)
    }
}

/// ADR 0035 §7: the folders of the projects `dir`'s project includes or
/// loads into slots, and theirs in turn, that lie outside `dir` (one inside
/// is watched with it), each with the path the host reaches it by
/// (`../studio`), at most [`crate::MAX_DEPTH`] deep, each folder once. A
/// `.signal` include outside `dir` names its file, watched as one (what it
/// holds is its own); a folder that does not exist names none.
pub fn include_dirs(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let Ok(root) = fs::canonicalize(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut seen = vec![root.clone()];
    let mut todo = vec![(PathBuf::new(), root.clone(), 0usize)];
    while let Some((shown, folder, depth)) = todo.pop() {
        if depth >= crate::MAX_DEPTH {
            continue;
        }
        let Ok(files) = crate::read_sources(&folder) else {
            continue;
        };
        for written in crate::included_paths(&files) {
            let Ok(canonical) = fs::canonicalize(folder.join(&written)) else {
                continue;
            };
            if canonical.is_file() {
                // A `.signal` include: its file is watched (outside `dir`),
                // and what it holds is its own.
                if !seen.contains(&canonical) && !canonical.starts_with(&root) {
                    found.push((shown.join(&written), canonical.clone()));
                }
                seen.push(canonical);
                continue;
            }
            if seen.contains(&canonical) {
                continue;
            }
            seen.push(canonical.clone());
            let path = shown.join(&written);
            if !canonical.starts_with(&root) {
                found.push((path.clone(), canonical.clone()));
            }
            todo.push((path, canonical, depth + 1));
        }
    }
    found.sort();
    found
}

fn snapshot(dir: &Path) -> io::Result<Snapshot> {
    let mut files = Snapshot::new();
    // A `.signal` include's file: itself, by the empty path.
    let metadata = fs::metadata(dir)?;
    if metadata.is_file() {
        files.insert(PathBuf::new(), (metadata.modified().ok(), metadata.len()));
        return Ok(files);
    }
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
    fn sleep(&mut self, members: &signal_core::InstanceAddress, asleep: bool) {
        self.0.sleep(members, asleep);
        self.1.sleep(members, asleep);
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
    // ADR 0038: the run routes its play sides when it starts (and the engine
    // keeps a run's sinks): adding, removing or re-roling one needs a
    // restart.
    let sides = |project: &Project, plan: &signal_graph::ExecutionPlan| {
        let mut sides: Vec<(String, String, usize)> = crate::device::play_sides(project, plan)
            .into_iter()
            .map(|s| (s.address.to_string(), s.role, s.width))
            .collect();
        sides.sort();
        sides
    };
    let (was, now) = (sides(old, before), sides(new, plan));
    if was != now {
        let list = |sides: &[(String, String, usize)]| {
            sides
                .iter()
                .map(|(address, role, _)| format!("{address} ({role})"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(vec![new.instances_error(format!(
            "the outputs must stay {} while the project plays (now {}); restart to change them",
            list(&was),
            list(&now)
        ))]);
    }
    // ADR 0038 §7: the run opens its inputs when it starts; a capture side
    // added, removed or re-roled live would read 0 until a restart.
    let captures = |project: &Project, plan: &signal_graph::ExecutionPlan| {
        let mut sides: Vec<(String, String, usize)> = crate::device::capture_sides(project, plan)
            .into_iter()
            .map(|s| (s.address.to_string(), s.role, s.width))
            .collect();
        sides.sort();
        sides
    };
    let (was, now) = (captures(old, before), captures(new, plan));
    if was != now {
        let list = |sides: &[(String, String, usize)]| {
            if sides.is_empty() {
                return "none".to_owned();
            }
            sides
                .iter()
                .map(|(address, role, _)| format!("{address} ({role})"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(vec![new.instances_error(format!(
            "the inputs must stay {} while the project plays (now {}); restart to change them",
            list(&was),
            list(&now)
        ))]);
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
    send_edit_with_bases(controller, live, observer, old, new, registry, &|_| None)
}

/// The live bases of the running plan's parameters as an observer last saw
/// them (`ControlSink::parameter`), by port; `None` where none was seen.
pub type Bases<'a> = &'a dyn Fn(&PortAddress) -> Option<f64>;

/// [`send_edit_observed`], with `bases` giving the running parameters'
/// live bases: a swap that adds voices to a polyphonic slot (ADR 0044 §8)
/// also sends each added voice, stamped for the swap's frame, a `set` of
/// every parameter whose surviving voice `v0` has another live base than
/// the new plan's value, and a `ramp` where a ramp sent to `v0` is still in
/// progress (to its target over its remaining frames), so the added voices
/// match the survivors. These sends are recorded like every edit, so a
/// replay reproduces them.
pub fn send_edit_with_bases(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    observer: &mut impl ControlSink,
    old: &Project,
    new: Project,
    registry: &Registry,
    bases: Bases<'_>,
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
                let mut sets = sets;
                sets.extend(voice_catch_up(
                    &before,
                    &plan,
                    bases,
                    live,
                    controller.default_frame(),
                ));
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

/// ADR 0044 §8: what brings the voices `after` adds to a polyphonic slot
/// (an added instance under `<slot>/v<e>`, beside a `<slot>/alloc` of
/// `std.voice_alloc`) to the live state of the surviving voice `v0`: for
/// each parameter, a `ramp` to the target of a ramp sent to `v0` still in
/// progress at `now`, else a `set` of `v0`'s live base where it differs
/// from the value `after` plans.
fn voice_catch_up(
    before: &signal_graph::ExecutionPlan,
    after: &signal_graph::ExecutionPlan,
    bases: Bases<'_>,
    live: &LiveRecorder,
    now: Frame,
) -> Vec<crate::ParameterChange> {
    use signal_core::EventPayload;
    let slots: Vec<String> = after
        .instances
        .iter()
        .filter(|i| i.type_name == "std.voice_alloc")
        .filter_map(|i| i.address.as_str().strip_suffix("/alloc").map(str::to_owned))
        .collect();
    if slots.is_empty() {
        return Vec::new();
    }
    let survivors: std::collections::BTreeSet<&str> = before
        .instances
        .iter()
        .map(|i| i.address.as_str())
        .collect();
    // The surviving voice an added one copies: `<slot>/v0<rest>`.
    let reference = |address: &str| -> Option<String> {
        slots.iter().find_map(|slot| {
            let rest = address.strip_prefix(slot.as_str())?.strip_prefix("/v")?;
            let digits = rest.find('/').unwrap_or(rest.len());
            let (index, rest) = rest.split_at(digits);
            (!index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()) && index != "0")
                .then(|| format!("{slot}/v0{rest}"))
        })
    };
    let added: Vec<(&signal_graph::PlanInstance, String)> = after
        .instances
        .iter()
        .filter(|i| !survivors.contains(i.address.as_str()))
        .filter_map(|i| {
            let r = reference(i.address.as_str())?;
            survivors.contains(r.as_str()).then_some((i, r))
        })
        .collect();
    if added.is_empty() {
        return Vec::new();
    }
    let history = live.recorder().edits();
    let mut sends = Vec::new();
    for (instance, reference) in added {
        let (Ok(at), Ok(from)) = (
            instance
                .address
                .as_str()
                .parse::<signal_core::InstanceAddress>(),
            reference.parse::<signal_core::InstanceAddress>(),
        ) else {
            continue;
        };
        for (name, planned) in &instance.parameters {
            let (Ok(port), Ok(source)) =
                (PortAddress::new(&at, name), PortAddress::new(&from, name))
            else {
                continue;
            };
            let events: Vec<(Frame, EventPayload)> = history
                .iter()
                .filter_map(|e| match &e.command {
                    crate::session::Command::Event { target, payload } if *target == source => {
                        Some((e.frame, *payload))
                    }
                    _ => None,
                })
                .collect();
            if let Some(&(frame, EventPayload::Ramp { to, frames, curve })) = events.last() {
                let end = frame.0.saturating_add(frames);
                if end > now.0 {
                    // The survivor's value now (`semantics.md` §6's ramp
                    // values), then the rest of its ramp: the added voice
                    // starts where the survivor is, not at its planned value.
                    let start = before
                        .instances
                        .iter()
                        .find(|i| i.address.as_str() == reference)
                        .and_then(|i| i.parameters.get(name))
                        .copied()
                        .unwrap_or(*planned);
                    sends.push(crate::ParameterChange {
                        port: port.clone(),
                        payload: EventPayload::Set(base_at(start, &events, now)),
                    });
                    sends.push(crate::ParameterChange {
                        port,
                        payload: EventPayload::Ramp {
                            to,
                            frames: end - now.0,
                            curve,
                        },
                    });
                    continue;
                }
            }
            if let Some(base) = bases(&source)
                && base.to_bits() != planned.to_bits()
            {
                sends.push(crate::ParameterChange {
                    port,
                    payload: EventPayload::Set(base),
                });
            }
        }
    }
    sends
}

/// A parameter's base at frame `at`, from `start` (its planned value) and
/// the `set`s and `ramp`s it was sent, in order (`semantics.md` §6: a ramp
/// of *N* frames from base *b* at *n₀* is *b* + (*to* − *b*)·*c*(*i*/*N*)
/// *i* frames on, exactly *to* from *N* on).
fn base_at(start: f64, events: &[(Frame, signal_core::EventPayload)], at: Frame) -> f64 {
    use signal_core::EventPayload;
    let value =
        |ramp: Option<(f64, f64, u64, u64, signal_core::Curve)>, base: f64, n: u64| match ramp {
            Some((b, to, frames, n0, curve)) => {
                let i = n.saturating_sub(n0);
                if i >= frames {
                    to
                } else {
                    b + (to - b) * curve.eval(i as f64 / frames as f64)
                }
            }
            None => base,
        };
    let mut base = start;
    let mut ramp = None;
    for (frame, payload) in events.iter().filter(|(f, _)| *f <= at) {
        let now = value(ramp, base, frame.0);
        match *payload {
            EventPayload::Set(v) => {
                base = v;
                ramp = None;
            }
            EventPayload::Ramp { to, frames: 0, .. } => {
                base = to;
                ramp = None;
            }
            EventPayload::Ramp { to, frames, curve } => {
                base = now;
                ramp = Some((now, to, frames, frame.0, curve));
            }
            _ => {}
        }
    }
    value(ramp, base, at.0)
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
    reload_with_bases(controller, live, observer, current, dir, registry, &|_| {
        None
    })
}

/// [`reload_observed`] with the live `bases` ([`send_edit_with_bases`]).
pub fn reload_with_bases(
    controller: &mut Controller,
    live: &mut LiveRecorder,
    observer: &mut impl ControlSink,
    current: &mut Project,
    dir: &Path,
    registry: &Registry,
    bases: Bases<'_>,
) -> Result<Applied, ReloadError> {
    let new = match Project::load(dir) {
        Ok(new) => new,
        Err(crate::LoadError::Invalid(diagnostics)) => {
            return Err(ReloadError::Invalid(crate::located(dir, diagnostics)));
        }
        Err(error) => return Err(ReloadError::Failed(error.to_string())),
    };
    match send_edit_with_bases(controller, live, observer, current, new, registry, bases) {
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
    use super::{FolderState, ProjectWatcher, is_presentation};
    use std::fs;
    use std::path::Path;

    /// ADR 0035 §7: a project included from outside the host's folder is
    /// watched too, its files named by the path the host reaches them by;
    /// an include added by an edit is watched from then on.
    #[test]
    fn the_watcher_follows_the_projects_it_includes() {
        let base = std::env::temp_dir().join(format!("signal-watch-inc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (host, inc) = (base.join("host"), base.join("inc"));
        fs::create_dir_all(inc.join("patterns")).unwrap();
        fs::create_dir_all(&host).unwrap();
        fs::write(inc.join("project.toml"), "x").unwrap();
        fs::write(host.join("project.toml"), "[instances]\n").unwrap();
        let mut watcher = ProjectWatcher::new(&host).unwrap();
        fs::write(
            host.join("project.toml"),
            "[instances]\ndrums = { project = \"../inc\" }\n",
        )
        .unwrap();
        assert_eq!(
            watcher.changes().unwrap(),
            vec![Path::new("project.toml").to_path_buf()]
        );
        fs::write(inc.join("patterns/beat.toml"), "y").unwrap();
        assert_eq!(
            watcher.changes().unwrap(),
            vec![Path::new("../inc/patterns/beat.toml").to_path_buf()]
        );
        let before = FolderState::read(&host).unwrap();
        fs::write(inc.join("patterns/beat.toml"), "yz").unwrap();
        assert_ne!(
            FolderState::read(&host).unwrap(),
            before,
            "a load's cache sees it"
        );
        let _ = fs::remove_dir_all(&base);
    }

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

    /// External review (Codex), item 3: the watch says which files changed
    /// (added, changed, removed), so a presentation-only change is told
    /// from one a load reads.
    #[test]
    fn the_watcher_names_the_changed_files() {
        let dir = std::env::temp_dir().join(format!("signal-changes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ui")).unwrap();
        fs::write(dir.join("project.toml"), "a").unwrap();
        fs::write(dir.join("ui/old.css"), "a").unwrap();
        let mut watcher = ProjectWatcher::new(&dir).unwrap();
        assert!(watcher.changes().unwrap().is_empty());
        fs::write(dir.join("ui/app.css"), "x").unwrap();
        fs::write(dir.join("ui/main.html"), "x").unwrap();
        fs::remove_file(dir.join("ui/old.css")).unwrap();
        let changed = watcher.changes().unwrap();
        assert_eq!(
            changed,
            [
                Path::new("ui/app.css"),
                Path::new("ui/main.html"),
                Path::new("ui/old.css")
            ]
        );
        assert!(changed.iter().all(|f| is_presentation(f)));
        fs::write(dir.join("project.toml"), "ab").unwrap();
        assert_eq!(watcher.changes().unwrap(), [Path::new("project.toml")]);
        assert!(!is_presentation(Path::new("project.toml")));
        assert!(!is_presentation(Path::new("composites/ui.toml")));
        assert!(!is_presentation(Path::new("uix/a.css")));
        // The presentation files are no part of the folder's state.
        let state = FolderState::read(&dir).unwrap();
        fs::write(dir.join("ui/app.css"), "changed").unwrap();
        assert_eq!(FolderState::read(&dir).unwrap(), state);
        fs::write(dir.join("project.toml"), "abc").unwrap();
        assert_ne!(FolderState::read(&dir).unwrap(), state);
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
