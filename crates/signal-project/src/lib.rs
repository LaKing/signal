//! Signal project loading: project and composite TOML files, validation with
//! file/line errors, and JSON Schema export (ADR 0009). The format is
//! normative in `docs/project-format.md`.
//!
//! - [`Project::load`] reads `project.toml` and `composites/*.toml` into the
//!   `signal-graph` description model ([`CompositeDef`]). Syntax errors and
//!   unknown keys are [`Diagnostic`]s with file, line and column.
//! - [`Project::plan`] builds the execution plan and maps every graph error and
//!   warning back to the instance or connection it concerns. Composites the
//!   project does not use are validated too.
//! - [`diff`]: the edit commands that turn a running project into a changed
//!   one (ADR 0006): parameter `set`s, or a structural swap.
//! - [`schema`]: the JSON Schema of both file kinds.
//! - [`session`]: session directories (`docs/session-format.md`): recording
//!   ([`session::SessionRecorder`], and [`session::LiveRecorder`] for a
//!   real-time run), reading and replaying.
//! - [`live`]: live edits from project files: [`live::ProjectWatcher`] polls a
//!   project folder, [`live::send_edit`] sends the diff to a real-time
//!   `Controller` and records it.
//! - [`provenance`]: the build's provenance for render manifests and sessions.
//! - [`write_parameter`]: a top-level instance's parameter written into
//!   `project.toml` text with `toml_edit`, keeping everything else.
//! - [`archive`]: project files (`.signal`, ADR 0025): a project folder
//!   packed into one deterministic zip, and unpacked with every entry checked.
//! - [`Project::structure`]: the structure as written, for the UI protocol's
//!   `graph` (ADR 0026 §1); [`schematic`]: the schematic's positions in
//!   `ui/schematic.toml` (ADR 0026 §3); [`patch`]: patching from the
//!   schematic, written as text (ADR 0026 §6).
//! - [`Library`]: the built-in library (ADR 0027 §4) that `lib.<name>`
//!   module references resolve to; [`vendor`]: `signal vendor`, a library
//!   composite copied into a project.

pub mod archive;
mod diagnostic;
mod diff;
mod include;
mod library;
pub mod live;
mod locate;
mod model;
pub mod patch;
mod pattern;
pub mod provenance;
mod schema;
pub mod schematic;
pub mod session;
mod slot;
pub mod snapshot;
mod structure;
pub mod transaction;
pub mod vendor;
mod write;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use signal_core::{InstanceAddress, LIBRARY_PREFIX, Registry, Table};
use signal_graph::{
    CompositeDef, ConnectionDef, ExecutionPlan, Exports, InputExport, InstanceDef, OutputExport,
    ParameterExport, build_plan,
};
use toml::Spanned;

pub use diagnostic::{Diagnostic, Severity, located};
pub use diff::{DiffOptions, ParameterChange, ProjectEdit, diff, diff_with};
pub use include::{IncludeInfo, MAX_DEPTH, included_paths};
pub use library::{LIBRARY_FOLDER, Library, LibraryComposite, is_library_file};
pub use pattern::{
    MAX_LANES, MAX_LENGTH, MAX_NOTES, MAX_STEP, MAX_STEPS, NOTES, PATTERNS, STEPS, pattern_kind,
};
pub use schema::schema;
pub use slot::{
    Interface, InterfacePort, LibraryInterface, Match, Role, SlotResolution, library_interfaces,
};
pub use structure::{library_modules, library_parameters};
pub use write::{
    WriteError, read_parameter, write_parameter, write_parameters, write_snapshot_exclude,
};

use locate::{
    ConnectionSpans, DefSpans, ExportKind, ExportSpans, InstanceSpans, Location, Locator, Root,
    SourceMap, Span,
};
use model::{CompositeFile, ExportTargets, Instances, ProjectFile, Targets, Wiring};
use pattern::Pattern;

/// The project file's path in a project folder.
pub const PROJECT_FILE: &str = "project.toml";

/// The folder of composite files in a project folder.
const COMPOSITES: &str = "composites";

/// The folder of slot interface templates in a project folder (ADR 0037 §3).
pub const INTERFACES: &str = "interfaces";

/// The plan version of a loaded project (ADR 0006).
const PLAN_VERSION: u64 = 1;

/// A project file's path (relative to the project folder, `/` separators) and
/// its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    /// E.g. `project.toml` or `composites/voice.toml`.
    pub path: String,
    /// The file's content.
    pub text: String,
}

/// Why a project folder could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    /// A file or folder could not be read.
    Io {
        /// The path.
        path: PathBuf,
        /// The error.
        error: io::Error,
    },
    /// The files are not a valid project.
    Invalid(Vec<Diagnostic>),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io { path, error } => write!(f, "{}: {error}", path.display()),
            LoadError::Invalid(diagnostics) => {
                for (i, diagnostic) in diagnostics.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{diagnostic}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// A loaded project: its settings and its graph description.
#[derive(Clone)]
pub struct Project {
    /// `[project] name`.
    pub name: String,
    /// `[project] sample_rate`.
    pub sample_rate: u32,
    /// `[project] seed`.
    pub seed: u64,
    /// Every project file as read: `project.toml` first, then the composites
    /// in path order.
    pub files: Vec<SourceFile>,
    /// The project root as a composite (named `""`, so no composite can be
    /// mistaken for it).
    pub root: CompositeDef,
    /// Every composite in `composites/`, in name order.
    pub composites: Vec<CompositeDef>,
    /// The library composites (`lib.<name>`, ADR 0027 §4) the project's
    /// files reference, transitively, in name order. Not project files:
    /// they are in neither `files` nor `composites`.
    pub library: Vec<LibraryComposite>,
    /// Every pattern in `patterns/` by name, with the index of its file
    /// (and, qualified, every included project's).
    patterns: BTreeMap<String, (usize, Pattern)>,
    spans: SourceMap,
    /// The byte offsets in `project.toml` of the `sample_rate` and `seed`
    /// values (0 for a key left out).
    settings_at: (usize, usize),
    /// ADR 0035, 0037: the projects this one includes or loads into slots,
    /// and what they bring.
    content: include::Content,
}

/// A validated plan and its warnings.
pub struct Planned {
    /// The execution plan.
    pub plan: ExecutionPlan,
    /// Graph warnings, located in the files.
    pub warnings: Vec<Diagnostic>,
    /// ADR 0037 §5: what the build decided for every slot (the match
    /// tables, the dropped wires, the unheard outputs), in definition then
    /// instance order. The plan carries no record of it.
    pub slots: Vec<slot::SlotResolution>,
}

/// A project folder's files as read: `project.toml`, every
/// `composites/*.toml`, every `patterns/*.toml` and every
/// `interfaces/*.toml`, each folder in name order, without checking what
/// they say (a project that does not load can still be read and patched).
pub fn read_sources(dir: &Path) -> Result<Vec<SourceFile>, LoadError> {
    let mut paths = vec![PROJECT_FILE.to_owned()];
    for folder in [COMPOSITES, pattern::PATTERNS, INTERFACES] {
        let full = dir.join(folder);
        match fs::read_dir(&full) {
            Ok(entries) => {
                let mut names = Vec::new();
                for entry in entries {
                    let entry = entry.map_err(|error| LoadError::Io {
                        path: full.clone(),
                        error,
                    })?;
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "toml") && path.is_file() {
                        names.push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
                names.sort();
                paths.extend(names.into_iter().map(|n| format!("{folder}/{n}")));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(LoadError::Io { path: full, error }),
        }
    }

    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    for path in paths {
        let full = dir.join(&path);
        let bytes = fs::read(&full).map_err(|error| LoadError::Io { path: full, error })?;
        match String::from_utf8(bytes) {
            Ok(text) => files.push(SourceFile { path, text }),
            Err(_) => diagnostics.push(Diagnostic {
                severity: Severity::Error,
                file: path,
                line: 1,
                column: 1,
                message: "the file is not UTF-8".to_owned(),
            }),
        }
    }
    if !diagnostics.is_empty() {
        return Err(LoadError::Invalid(diagnostics));
    }
    Ok(files)
}

impl Project {
    /// The patterns of `patterns/` (`project-format.md` §12), by name, with
    /// their kind ([`NOTES`] or [`STEPS`]).
    pub fn patterns(&self) -> impl Iterator<Item = (&str, &'static str)> {
        self.patterns
            .iter()
            .map(|(name, (_, pattern))| (name.as_str(), pattern.kind))
    }

    /// Loads the project folder `dir`: `project.toml` and every
    /// `composites/*.toml`, `patterns/*.toml` and `interfaces/*.toml`, and
    /// every project it includes (ADR 0035), each read and checked by the
    /// same rules in its own folder.
    pub fn load(dir: &Path) -> Result<Project, LoadError> {
        let files = read_sources(dir)?;
        let canonical = fs::canonicalize(dir).map_err(|error| LoadError::Io {
            path: dir.to_owned(),
            error,
        })?;
        let chain = [canonical.clone()];
        Project::assemble(files, Library::builtin(), Some(&canonical), &chain)
            .map_err(LoadError::Invalid)
    }

    /// The addresses of every instance of composite `name` in the project
    /// (`/drums/kick`), sorted.
    pub fn instances_of(&self, name: &str) -> Vec<String> {
        let by_name: BTreeMap<&str, &CompositeDef> = self
            .composites
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        let mut found = Vec::new();
        let mut pending = vec![(&self.root, String::new(), 0usize)];
        while let Some((def, prefix, depth)) = pending.pop() {
            // A cycle does not plan; the depth only keeps this finite.
            if depth > by_name.len() {
                continue;
            }
            for instance in def.instances.iter().rev() {
                let address = format!("{prefix}/{}", instance.name);
                if let Some(inner) = by_name.get(instance.module.as_str()) {
                    if instance.module == name {
                        found.push(address.clone());
                    }
                    pending.push((inner, address, depth + 1));
                }
            }
        }
        found.sort();
        found
    }

    /// Loads a project from in-memory files: `project.toml` and any
    /// `composites/<name>.toml`, its `lib.<name>` references resolved in the
    /// built-in library.
    pub fn from_sources(files: Vec<SourceFile>) -> Result<Project, Vec<Diagnostic>> {
        Project::from_sources_with(files, Library::builtin())
    }

    /// [`from_sources`](Project::from_sources) for files of the project
    /// folder `dir` (as they are, or as an edit would leave them): the
    /// projects they include (ADR 0035) are found from `dir`, as
    /// [`load`](Project::load) finds them.
    pub fn from_sources_in(files: Vec<SourceFile>, dir: &Path) -> Result<Project, Vec<Diagnostic>> {
        Project::from_sources_in_with(files, Library::builtin(), dir)
    }

    /// [`from_sources_in`](Project::from_sources_in) with `library` as the
    /// library.
    pub(crate) fn from_sources_in_with(
        files: Vec<SourceFile>,
        library: &Library,
        dir: &Path,
    ) -> Result<Project, Vec<Diagnostic>> {
        let canonical = fs::canonicalize(dir).map_err(|error| {
            vec![Diagnostic {
                severity: Severity::Error,
                file: PROJECT_FILE.to_owned(),
                line: 1,
                column: 1,
                message: format!(
                    "the project folder {} cannot be read: {error}",
                    dir.display()
                ),
            }]
        })?;
        let chain = [canonical.clone()];
        Project::assemble(files, library, Some(&canonical), &chain)
    }

    /// [`from_sources`](Project::from_sources) with `library` as the library.
    /// Files in memory have no folder, so a project that includes another
    /// (ADR 0035) is refused here: [`load`](Project::load) it from its folder.
    pub fn from_sources_with(
        files: Vec<SourceFile>,
        library: &Library,
    ) -> Result<Project, Vec<Diagnostic>> {
        Project::assemble(files, library, None, &[])
    }

    /// Loads a project from its files; `dir` is its canonical folder, where
    /// the projects it includes are found, and `chain` the canonical
    /// folders of the projects including it, outermost first, itself last
    /// (ADR 0035 §8: cycles and depth).
    fn assemble(
        mut files: Vec<SourceFile>,
        library: &Library,
        dir: Option<&Path>,
        chain: &[PathBuf],
    ) -> Result<Project, Vec<Diagnostic>> {
        files.sort_by(|a, b| {
            (a.path != PROJECT_FILE, &a.path).cmp(&(b.path != PROJECT_FILE, &b.path))
        });
        if files.first().is_none_or(|f| f.path != PROJECT_FILE) {
            return Err(vec![Diagnostic {
                severity: Severity::Error,
                file: PROJECT_FILE.to_owned(),
                line: 1,
                column: 1,
                message: "the project folder has no project.toml".to_owned(),
            }]);
        }
        let interface_prefix = format!("{INTERFACES}/");
        let is_interface = |file: usize| {
            files
                .get(file)
                .is_some_and(|f| f.path.starts_with(&interface_prefix))
        };
        let no_templates = BTreeMap::new();
        let mut builder = Builder {
            files: &files,
            library,
            templates: &no_templates,
            diagnostics: Vec::new(),
        };
        let templates: BTreeMap<String, slot::Interface> = (1..files.len())
            .filter(|&file| is_interface(file))
            .filter_map(|file| builder.interface(file))
            .collect();
        let mut builder = Builder {
            files: &files,
            library,
            templates: &templates,
            diagnostics: builder.diagnostics,
        };
        let root = builder.project();
        let pattern_prefix = format!("{}/", pattern::PATTERNS);
        let is_pattern = |file: usize| {
            files
                .get(file)
                .is_some_and(|f| f.path.starts_with(&pattern_prefix))
        };
        let composites: Vec<_> = (1..files.len())
            .filter(|&file| !is_pattern(file) && !is_interface(file))
            .filter_map(|file| builder.composite(file))
            .collect();
        let patterns: BTreeMap<String, (usize, Pattern)> = (1..files.len())
            .filter(|&file| is_pattern(file))
            .filter_map(|file| builder.pattern(file))
            .collect();
        // Every pattern file there is, read or refused (its own errors say why).
        let pattern_names: BTreeSet<&str> = files
            .iter()
            .filter_map(|f| f.path.strip_prefix(&pattern_prefix)?.strip_suffix(".toml"))
            .collect();
        let Some((settings, mut root, mut root_spans)) = root else {
            return Err(builder.diagnostics);
        };
        let mut composites = composites;
        let names: BTreeSet<&str> = composites.iter().map(|(c, _)| c.name.as_str()).collect();
        builder.check_pins(&root_spans, &names);
        builder.check_reserved(&root_spans);
        builder.check_patterns(&root_spans, &names, &pattern_names);
        builder.check_shadowing(&root_spans, &names);
        for (_, spans) in &composites {
            builder.check_pins(spans, &names);
            builder.check_reserved(spans);
            builder.check_snapshot_files(spans);
            builder.check_patterns(spans, &names, &pattern_names);
            builder.check_shadowing(spans, &names);
        }

        // ADR 0035, 0037: the projects the files include or load, each
        // read and checked in its own folder.
        let mut content = include::Content::default();
        {
            let defs = std::iter::once((&mut root, &mut root_spans))
                .chain(composites.iter_mut().map(|(c, s)| (c, s)));
            content.load(defs, dir, chain, library, &mut builder.diagnostics, &files);
        }

        // ADR 0027 §4: the library composites the files reference,
        // transitively, read after the project's files (their diagnostics'
        // file indices follow the project's).
        let referenced = std::iter::once(&root).chain(composites.iter().map(|(c, _)| c));
        let mut combined = files.clone();
        combined.extend(library.closure(referenced));
        let mut builder = Builder {
            files: &combined,
            library,
            templates: &templates,
            diagnostics: builder.diagnostics,
        };
        let used: Vec<_> = (files.len()..combined.len())
            .filter_map(|file| builder.composite(file))
            .collect();
        for (def, spans) in &used {
            builder.check_library(def, spans);
        }
        if !builder.diagnostics.is_empty() {
            return Err(builder.diagnostics);
        }
        let library_sources = combined.split_off(files.len());
        let (composites, spans): (Vec<_>, Vec<_>) = composites.into_iter().chain(used).unzip();
        let spans = SourceMap {
            root: root_spans,
            composites: composites
                .iter()
                .map(|c: &CompositeDef| c.name.clone())
                .zip(spans)
                .collect(),
        };
        let (library, composites): (Vec<_>, Vec<_>) = composites
            .into_iter()
            .partition(|c| c.name.starts_with(LIBRARY_PREFIX));
        let library = library
            .into_iter()
            .zip(library_sources)
            .map(|(def, source)| LibraryComposite {
                name: def.name.clone(),
                hash: format!("{:016x}", signal_core::fnv1a64(source.text.as_bytes())),
                source,
                def,
            })
            .collect();
        let mut project = Project {
            name: settings.name,
            sample_rate: settings.sample_rate,
            seed: settings.seed,
            files,
            root,
            composites,
            library,
            patterns,
            spans,
            settings_at: (settings.sample_rate_at, settings.seed_at),
            content: include::Content::default(),
        };
        project.content = content;
        project.merge_included();
        Ok(project)
    }

    /// Builds the execution plan with the module types of `registry`.
    ///
    /// Every composite file is validated: those the project reaches through
    /// its instances as part of the project, every other one as if it were
    /// instantiated once under its own name (its warnings are dropped). Errors
    /// of the first failing graph stage are reported, as `build_plan` does.
    ///
    /// Every included project (ADR 0035) is first planned alone, by every
    /// rule; its problems are located in its files. Inside the host its
    /// sinks are their taps, and slots (ADR 0037) are resolved into
    /// ordinary instances of their content before the graph sees them.
    pub fn plan(&self, registry: &Registry) -> Result<Planned, Vec<Diagnostic>> {
        let mut errors = Vec::new();
        for (canonical, included) in &self.content.projects {
            let Err(found) = included.project.plan(registry) else {
                continue;
            };
            let count = found.len();
            for diagnostic in include::reroot(&included.shown, found) {
                push_unique(&mut errors, diagnostic);
            }
            for entry in self
                .content
                .entries
                .iter()
                .filter(|e| e.canonical == *canonical)
            {
                let location = self.entry_location(&entry.def, &entry.instance);
                let message = format!(
                    "{}: `{}` does not plan ({count} problem{} in its files)",
                    entry.instance,
                    entry.written,
                    if count == 1 { "" } else { "s" }
                );
                push_unique(
                    &mut errors,
                    self.diagnostic(Severity::Error, location, message),
                );
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        // The project's composites and the library composites it references
        // (ADR 0027 §4), resolved alike by their module references, then
        // what its includes bring, their sinks as taps (ADR 0035 §3).
        let mut definitions = self.definitions();
        definitions.extend(self.tapped(registry, &mut errors));
        self.check_set(&definitions, registry, &mut errors);
        self.check_snapshot_exclude(&definitions, registry, &mut errors);
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut root_def = self.root.clone();
        // ADR 0035 §2: alone, an export of a sink's tap output
        // (`out.l_out`) names what the sink becomes when the project is
        // included; it is checked against the tap, and left out here.
        root_def.exports.outputs.retain(|export| {
            let Some((instance, port)) = export.from.split_once('.') else {
                return true;
            };
            let tap = self
                .root
                .instances
                .iter()
                .find(|i| i.name == instance)
                .and_then(|i| registry.get(&i.module))
                .filter(|t| t.descriptor.sink.is_some())
                .and_then(|t| registry.sink_tap(t.descriptor.type_name))
                .and_then(|tap| registry.get(tap));
            match tap {
                Some(tap) => !tap
                    .descriptor
                    .ports
                    .iter()
                    .any(|p| p.name == port && p.direction == signal_core::Direction::Out),
                None => true,
            }
        });
        let mut spans = self.spans.clone();
        let mut extra_warnings = self.content.warnings.clone();
        let slots = self.resolve_slots(
            &mut root_def,
            &mut definitions,
            &mut spans,
            registry,
            &mut errors,
            &mut extra_warnings,
        );
        if !errors.is_empty() {
            return Err(errors);
        }
        let root = Locator {
            map: &spans,
            root: Root::Project,
        };
        let planned = match build_plan(&root_def, &definitions, registry, PLAN_VERSION) {
            Ok(plan) => {
                let mut warnings = Vec::new();
                for warning in extra_warnings {
                    push_unique(&mut warnings, warning);
                }
                for warning in &plan.warnings {
                    let location = root.warning(warning);
                    push_unique(
                        &mut warnings,
                        self.diagnostic(Severity::Warning, location, warning.to_string()),
                    );
                }
                Some(Planned {
                    plan,
                    warnings,
                    slots,
                })
            }
            Err(graph_errors) => {
                for error in &graph_errors {
                    let location = root.error(error);
                    push_unique(
                        &mut errors,
                        self.diagnostic(Severity::Error, location, error.to_string()),
                    );
                }
                None
            }
        };

        let reachable = self.reachable_composites();
        for composite in &definitions {
            // An include's own composites were validated when it was
            // planned alone.
            if reachable.contains(composite.name.as_str()) || composite.name.contains('/') {
                continue;
            }
            // A library composite's instance is named by its name without
            // `lib.` (an instance name has no dot).
            let synthetic = CompositeDef {
                name: String::new(),
                instances: vec![InstanceDef {
                    name: locate::instance_name(&composite.name).to_owned(),
                    module: composite.name.clone(),
                    parameters: BTreeMap::new(),
                    scope: None,
                }],
                ..CompositeDef::default()
            };
            if let Err(graph_errors) = build_plan(&synthetic, &definitions, registry, PLAN_VERSION)
            {
                let locator = Locator {
                    map: &spans,
                    root: Root::Composite(&composite.name),
                };
                for error in &graph_errors {
                    let location = locator.error(error);
                    push_unique(
                        &mut errors,
                        self.diagnostic(Severity::Error, location, error.to_string()),
                    );
                }
            }
        }

        self.check_categories(registry, &mut errors);
        let tables = self.tables(registry, &mut errors);
        match planned {
            Some(mut planned) if errors.is_empty() => {
                for instance in &mut planned.plan.instances {
                    instance.table = root
                        .pattern(&instance.address)
                        .and_then(|name| tables.get(&(name, instance.type_name)))
                        .cloned();
                }
                Ok(planned)
            }
            _ => Err(errors),
        }
    }

    /// Where the instance `instance` of the definition `def` (`""`: the
    /// root) is written: its module, project or load value.
    fn entry_location(&self, def: &str, instance: &str) -> Location {
        let spans = if def.is_empty() {
            Some(&self.spans.root)
        } else {
            self.spans.composites.get(def)
        };
        spans
            .and_then(|spans| {
                let at = spans.instances.get(instance)?.module_span.start;
                Some(Location {
                    file: spans.file,
                    offset: at,
                })
            })
            .unwrap_or(Location { file: 0, offset: 0 })
    }

    /// The project's composites, then the library composites it references.
    fn definitions(&self) -> Vec<CompositeDef> {
        let library = self.library.iter().map(|l| &l.def);
        self.composites.iter().chain(library).cloned().collect()
    }

    /// The library composites the plan instantiates, transitively, with
    /// their content hashes (ADR 0027 §4, ADR 0008): the render manifest's
    /// `library_composite_hashes`. A library composite only an unused
    /// project composite references is not listed.
    pub fn library_hashes(&self) -> BTreeMap<String, String> {
        let reached = self.reachable_composites();
        self.library
            .iter()
            .filter(|l| reached.contains(l.name.as_str()))
            .map(|l| (l.name.clone(), l.hash.clone()))
            .collect()
    }

    /// The `category` a composite (of the project or the library) declares.
    pub fn category(&self, composite: &str) -> Option<&str> {
        let spans = self.spans.composites.get(composite)?;
        spans.category.as_ref().map(|(c, _)| c.as_str())
    }

    /// The `description` a composite (of the project or the library) gives.
    pub fn description(&self, composite: &str) -> Option<&str> {
        self.spans.composites.get(composite)?.description.as_deref()
    }

    /// ADR 0027 §1: a composite's `category` is one of the palette groups,
    /// those the registry's types carry (the vocabulary is the library's
    /// that registers them, never this crate's).
    fn check_categories(&self, registry: &Registry, errors: &mut Vec<Diagnostic>) {
        let known: BTreeSet<&str> = registry
            .iter()
            .filter_map(|t| t.descriptor.category)
            .collect();
        for def in self.spans.composites.values() {
            let Some((category, span)) = &def.category else {
                continue;
            };
            if !known.contains(category.as_str()) {
                let list: Vec<&str> = known.iter().copied().collect();
                let location = Location {
                    file: def.file,
                    offset: span.start,
                };
                let message = format!(
                    "unknown category `{category}`; categories: {}",
                    list.join(", ")
                );
                push_unique(errors, self.diagnostic(Severity::Error, location, message));
            }
        }
    }

    /// ADR 0028 §3, §4: every instance that names a pattern, in the project
    /// and in every composite (used or not), takes one of the kind its type
    /// declares; each such pattern is compiled by the type's library. Returns
    /// the tables by (pattern, type); refusals go to `errors`, at the
    /// instance's `pattern` value.
    fn tables<'s>(
        &'s self,
        registry: &Registry,
        errors: &mut Vec<Diagnostic>,
    ) -> BTreeMap<(&'s str, &'static str), Table> {
        let mut tables = BTreeMap::new();
        let defs = std::iter::once(&self.spans.root).chain(self.spans.composites.values());
        for def in defs {
            for (name, instance) in &def.instances {
                let Some((pattern, span)) = &instance.pattern else {
                    continue;
                };
                // Unknown modules and composites are reported elsewhere.
                let (Some(module_type), Some((_, source))) =
                    (registry.get(&instance.module), self.patterns.get(pattern))
                else {
                    continue;
                };
                let type_name = module_type.descriptor.type_name;
                let file = format!("{}/{pattern}.toml", pattern::PATTERNS);
                let location = Location {
                    file: def.file,
                    offset: span.start,
                };
                let refused = match registry.table_rule(type_name) {
                    None => Some(format!(
                        "{name}: `{type_name}` takes no pattern, so it cannot take {file}"
                    )),
                    Some(rule) if rule.kind != source.kind => Some(format!(
                        "{name}: `{type_name}` takes a `{}` pattern; {file} is `{}`",
                        rule.kind, source.kind
                    )),
                    Some(_) if tables.contains_key(&(pattern.as_str(), type_name)) => None,
                    Some(rule) => match (rule.compile)(&source.header, &source.rows) {
                        Ok(table) => {
                            tables.insert((pattern.as_str(), type_name), table);
                            None
                        }
                        Err(why) => Some(format!("{name}: {file}: {why}")),
                    },
                };
                if let Some(message) = refused {
                    push_unique(errors, self.diagnostic(Severity::Error, location, message));
                }
            }
        }
        tables
    }

    /// The `gpu` pins (M2.7-2) of `plan`'s instances (a plan of this
    /// project), by address: what a GPU host places them by.
    pub fn gpu_pins(&self, plan: &ExecutionPlan) -> BTreeMap<String, String> {
        let locator = Locator {
            map: &self.spans,
            root: Root::Project,
        };
        plan.instances
            .iter()
            .filter_map(|i| Some((i.address.to_string(), locator.pin(&i.address)?.to_owned())))
            .collect()
    }

    /// ADR 0041: the parameters of the top-level instance `instance` that
    /// its `snapshot_exclude` leaves out of snapshots (empty: all go in).
    pub fn snapshot_excluded(&self, instance: &str) -> BTreeSet<String> {
        self.spans
            .root
            .instances
            .get(instance)
            .map(|i| i.snapshot_exclude.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default()
    }

    /// ADR 0041: each name in a top-level instance's `snapshot_exclude` is
    /// one of its parameters (a composite's exported one), as a parameter
    /// key is.
    fn check_snapshot_exclude(
        &self,
        defs: &[CompositeDef],
        registry: &Registry,
        errors: &mut Vec<Diagnostic>,
    ) {
        for instance in &self.root.instances {
            let Some(spans) = self.spans.root.instances.get(&instance.name) else {
                continue;
            };
            if spans.snapshot_exclude.is_empty() {
                continue;
            }
            // A module that does not resolve is reported where it is written.
            let Ok(ports) = signal_graph::ports_of(&instance.module, defs, registry) else {
                continue;
            };
            for (name, span) in &spans.snapshot_exclude {
                if !ports.iter().any(|p| p.parameter && p.name == *name) {
                    let location = locate::Location {
                        file: self.spans.root.file,
                        offset: span.start,
                    };
                    let diagnostic = self.diagnostic(
                        Severity::Error,
                        location,
                        format!(
                            "{}: `snapshot_exclude` names `{name}`, which is no parameter of `{}`",
                            instance.name, instance.module
                        ),
                    );
                    push_unique(errors, diagnostic);
                }
            }
        }
    }

    /// An error located at the declaration of the instance at `address`, or at
    /// the start of `project.toml` if there is none.
    pub fn instance_error(&self, address: &InstanceAddress, message: String) -> Diagnostic {
        let locator = Locator {
            map: &self.spans,
            root: Root::Project,
        };
        self.diagnostic(Severity::Error, locator.declaration(address), message)
    }

    /// An error about the project's instances as a whole, located at the
    /// `[instances]` table (or the start of `project.toml`).
    pub fn instances_error(&self, message: String) -> Diagnostic {
        let offset = self
            .spans
            .root
            .instances_table
            .as_ref()
            .map_or(0, |s| s.start);
        self.diagnostic(Severity::Error, Location { file: 0, offset }, message)
    }

    /// The composites, of the project and the library, reachable from the
    /// project root through instance module references.
    fn reachable_composites(&self) -> BTreeSet<&str> {
        let library = self.library.iter().map(|l| &l.def);
        let by_name: BTreeMap<&str, &CompositeDef> = self
            .composites
            .iter()
            .chain(library)
            .chain(&self.content.defs)
            .map(|c| (c.name.as_str(), c))
            .collect();
        let mut reached = BTreeSet::new();
        let mut pending: Vec<&str> = self
            .root
            .instances
            .iter()
            .map(|i| i.module.as_str())
            .collect();
        while let Some(module) = pending.pop() {
            if let Some(def) = by_name.get(module)
                && reached.insert(module)
            {
                pending.extend(def.instances.iter().map(|i| i.module.as_str()));
            }
        }
        reached
    }

    fn diagnostic(&self, severity: Severity, location: Location, message: String) -> Diagnostic {
        // Library files are numbered after the project's, included
        // projects' files from `include::INCLUDED` on.
        let library = location
            .file
            .checked_sub(self.files.len())
            .and_then(|i| self.library.get(i))
            .map(|l| &l.source);
        let included = location
            .file
            .checked_sub(include::INCLUDED)
            .and_then(|i| self.content.files.get(i));
        // Qualified names read as written (`kick909`, not
        // `/…/studio/kick909`).
        let mut message = message;
        for id in &self.content.ids {
            message = message.replace(&format!("{id}/"), "");
        }
        let file = if location.file >= include::INCLUDED {
            included
        } else {
            self.files.get(location.file).or(library)
        };
        match file {
            Some(file) => {
                Diagnostic::at(severity, &file.path, &file.text, location.offset, message)
            }
            None => Diagnostic {
                severity,
                file: PROJECT_FILE.to_owned(),
                line: 1,
                column: 1,
                message,
            },
        }
    }
}

fn push_unique(list: &mut Vec<Diagnostic>, diagnostic: Diagnostic) {
    if !list.contains(&diagnostic) {
        list.push(diagnostic);
    }
}

/// Whether `name` is `[a-z][a-z0-9_]*`.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `[project]` after checks.
struct Settings {
    name: String,
    sample_rate: u32,
    /// The byte offset of the `sample_rate` value in `project.toml` (0 if absent).
    sample_rate_at: usize,
    seed: u64,
    /// The byte offset of the `seed` value in `project.toml` (0 if absent).
    seed_at: usize,
}

/// Parses files into definitions and span tables, collecting diagnostics.
struct Builder<'f> {
    files: &'f [SourceFile],
    library: &'f Library,
    /// The project's interface templates (ADR 0037 §3), by name.
    templates: &'f BTreeMap<String, slot::Interface>,
    diagnostics: Vec<Diagnostic>,
}

impl Builder<'_> {
    fn error(&mut self, file: usize, offset: usize, message: String) {
        if let Some(source) = self.files.get(file) {
            self.diagnostics.push(Diagnostic::at(
                Severity::Error,
                &source.path,
                &source.text,
                offset,
                message,
            ));
        }
    }

    fn parse<T: serde::de::DeserializeOwned>(&mut self, file: usize) -> Option<T> {
        let text = &self.files.get(file)?.text;
        match toml::from_str(text) {
            Ok(value) => Some(value),
            Err(error) => {
                let offset = error.span().map_or(0, |s| s.start);
                self.error(file, offset, error.message().trim().to_owned());
                None
            }
        }
    }

    /// M2.7-2: a `gpu` pin is a device id or index on a primitive instance;
    /// a composite cannot export a parameter named `gpu` (the key is
    /// reserved in instance tables).
    fn check_pins(&mut self, def: &DefSpans, composites: &BTreeSet<&str>) {
        for (name, instance) in &def.instances {
            let Some((pin, span)) = &instance.gpu else {
                continue;
            };
            if pin.is_empty() {
                self.error(
                    def.file,
                    span.start,
                    format!("{name}: `gpu` is empty; give a device id or index (`signal devices --compute`)"),
                );
            } else if composites.contains(instance.module.as_str()) {
                self.error(
                    def.file,
                    span.start,
                    format!(
                        "{name}: `gpu` pins a primitive instance; `{}` is a composite: pin the instances inside it",
                        instance.module
                    ),
                );
            }
        }
        for export in &def.exports {
            if export.kind == ExportKind::Parameter && export.name == "gpu" {
                self.error(
                    def.file,
                    export.key.start,
                    "a composite cannot export a parameter named `gpu`: the key is reserved in instance tables".to_owned(),
                );
            }
        }
    }

    /// ADR 0035 §1, ADR 0037 §2: the instance keys `module`, `project`,
    /// `seed`, `set`, `slot`, `load`, `map` and `snapshot_exclude` (ADR 0041)
    /// are reserved, so no
    /// composite or project may export a parameter of those names (`gpu`
    /// and `pattern` are checked with their own rules).
    fn check_reserved(&mut self, def: &DefSpans) {
        const RESERVED: [&str; 8] = [
            "module",
            "project",
            "seed",
            "set",
            "slot",
            "load",
            "map",
            "snapshot_exclude",
        ];
        for export in &def.exports {
            if export.kind == ExportKind::Parameter && RESERVED.contains(&export.name.as_str()) {
                self.error(
                    def.file,
                    export.key.start,
                    format!(
                        "a composite or project cannot export a parameter named `{}`: the key is reserved in instance tables",
                        export.name
                    ),
                );
            }
        }
    }

    /// ADR 0041: `snapshot_exclude` says which controls of the project go
    /// into its snapshots, so it belongs in `project.toml` only: in a
    /// composite's file it would name parameters of every instance of it.
    fn check_snapshot_files(&mut self, def: &DefSpans) {
        for (name, instance) in &def.instances {
            if let Some((_, span)) = instance.snapshot_exclude.first() {
                self.error(
                    def.file,
                    span.start,
                    format!(
                        "{name}: `snapshot_exclude` belongs in project.toml (it says which of the project's controls go into its snapshots)"
                    ),
                );
            }
        }
    }

    /// ADR 0028 §3: `pattern` names a pattern file of the project, on a
    /// primitive instance; a composite cannot export a parameter named
    /// `pattern` (the key is reserved in instance tables).
    fn check_patterns(
        &mut self,
        def: &DefSpans,
        composites: &BTreeSet<&str>,
        patterns: &BTreeSet<&str>,
    ) {
        for (name, instance) in &def.instances {
            let Some((pattern, span)) = &instance.pattern else {
                continue;
            };
            if composites.contains(instance.module.as_str()) {
                self.error(
                    def.file,
                    span.start,
                    format!(
                        "{name}: `pattern` gives a primitive instance its data; `{}` is a composite, which takes no pattern",
                        instance.module
                    ),
                );
            } else if !valid_name(pattern) {
                self.error(
                    def.file,
                    span.start,
                    format!(
                        "{name}: invalid pattern name `{pattern}`; names use a-z, 0-9, _ and start with a-z"
                    ),
                );
            } else if !patterns.contains(pattern.as_str()) {
                self.error(
                    def.file,
                    span.start,
                    format!(
                        "{name}: pattern `{pattern}`: {}/{pattern}.toml does not exist",
                        pattern::PATTERNS
                    ),
                );
            }
        }
        for export in &def.exports {
            if export.kind == ExportKind::Parameter && export.name == "pattern" {
                self.error(
                    def.file,
                    export.key.start,
                    "a composite cannot export a parameter named `pattern`: the key is reserved in instance tables".to_owned(),
                );
            }
        }
    }

    /// ADR 0027 §4: a project composite does not shadow a library one. A
    /// direct reference to `lib.x` where the project also has a composite
    /// `x` is an error naming both (references inside library composites do
    /// not count: `signal vendor` may copy one of their dependencies alone).
    fn check_shadowing(&mut self, def: &DefSpans, composites: &BTreeSet<&str>) {
        for (name, instance) in &def.instances {
            let Some(bare) = instance.module.strip_prefix(LIBRARY_PREFIX) else {
                continue;
            };
            if composites.contains(bare) && self.library.get(bare).is_some() {
                self.error(
                    def.file,
                    instance.module_span.start,
                    format!(
                        "{name}: `{LIBRARY_PREFIX}{bare}` is a library composite, and composites/{bare}.toml is a project composite named like it; a project composite may not shadow a library one: use `{bare}` (the project's) or rename composites/{bare}.toml"
                    ),
                );
            }
        }
    }

    /// ADR 0027 §4: a library composite uses `std.*` and `lib.*` modules
    /// only, never a project composite, and takes no `gpu` pin or `pattern`
    /// (both name things of a project or a machine); it declares its
    /// category and gives a description.
    fn check_library(&mut self, def: &CompositeDef, spans: &DefSpans) {
        if spans.category.is_none() {
            self.error(
                spans.file,
                spans.name.start,
                format!("library composite `{}` declares no `category`", def.name),
            );
        }
        if spans.description.is_none() {
            self.error(
                spans.file,
                spans.name.start,
                format!("library composite `{}` gives no `description`", def.name),
            );
        }
        for (name, instance) in &spans.instances {
            let module = instance.module.as_str();
            if !module.starts_with("std.") && !module.starts_with(LIBRARY_PREFIX) {
                self.error(
                    spans.file,
                    instance.module_span.start,
                    format!(
                        "{name}: `{module}`: the library composite `{}` may use std.* and {LIBRARY_PREFIX}* modules only, never a project composite",
                        def.name
                    ),
                );
            }
            for (key, value) in [("gpu", &instance.gpu), ("pattern", &instance.pattern)] {
                if let Some((_, span)) = value {
                    self.error(
                        spans.file,
                        span.start,
                        format!(
                            "{name}: a library composite takes no `{key}`: it names something of a project or a machine"
                        ),
                    );
                }
            }
        }
    }

    /// An interface template file (ADR 0037 §3): its name and interface.
    fn interface(&mut self, file: usize) -> Option<(String, slot::Interface)> {
        let source = self.files.get(file)?;
        let stem = source
            .path
            .strip_prefix(&format!("{INTERFACES}/"))
            .and_then(|p| p.strip_suffix(".toml"))
            .unwrap_or(&source.path)
            .to_owned();
        match slot::parse_interface(&source.text, &stem) {
            Ok(interface) => Some((stem, interface)),
            Err(problems) => {
                for (offset, message) in problems {
                    self.error(file, offset, message);
                }
                None
            }
        }
    }

    /// A pattern file (ADR 0028 §1–2): its name and what it holds.
    fn pattern(&mut self, file: usize) -> Option<(String, (usize, Pattern))> {
        let source = self.files.get(file)?;
        let stem = source
            .path
            .strip_prefix(&format!("{}/", pattern::PATTERNS))
            .and_then(|p| p.strip_suffix(".toml"))
            .unwrap_or(&source.path)
            .to_owned();
        if !valid_name(&stem) {
            self.error(
                file,
                0,
                format!(
                    "invalid pattern name `{stem}`; a pattern file is patterns/<name>.toml, and names use a-z, 0-9, _ and start with a-z"
                ),
            );
            return None;
        }
        match pattern::parse(&source.text) {
            Ok(parsed) => Some((stem, (file, parsed))),
            Err(problems) => {
                for (offset, message) in problems {
                    self.error(file, offset, message);
                }
                None
            }
        }
    }

    fn schema_version(&mut self, file: usize, version: &Spanned<i64>) {
        if *version.get_ref() != 1 {
            self.error(
                file,
                version.span().start,
                format!(
                    "unsupported schema_version {}; this version of Signal reads 1",
                    version.get_ref()
                ),
            );
        }
    }

    fn project(&mut self) -> Option<(Settings, CompositeDef, DefSpans)> {
        let parsed: ProjectFile = self.parse(0)?;
        self.schema_version(0, &parsed.schema_version);
        let sample_rate = parsed.project.sample_rate;
        if *sample_rate.get_ref() == 0 {
            self.error(
                0,
                sample_rate.span().start,
                "sample_rate must be at least 1".to_owned(),
            );
        }
        let seed = parsed.project.seed;
        let settings = Settings {
            name: parsed.project.name,
            sample_rate_at: sample_rate.span().start,
            sample_rate: sample_rate.into_inner(),
            seed_at: seed.span().start,
            seed: seed.into_inner(),
        };
        let templates = self.templates;
        let (def, spans, problems) = definition(
            0,
            String::new(),
            0..0,
            parsed.instances,
            parsed.wiring,
            parsed.exports,
            templates,
        );
        for (offset, message) in problems {
            self.error(0, offset, message);
        }
        Some((settings, def, spans))
    }

    fn composite(&mut self, file: usize) -> Option<(CompositeDef, DefSpans)> {
        let parsed: CompositeFile = self.parse(file)?;
        self.schema_version(file, &parsed.schema_version);
        let path = &self.files.get(file)?.path;
        // A library composite (`signal-lib:lib/<name>.toml`) is defined as
        // `lib.<name>`.
        let (folder, in_library) = match library::library_stem(path) {
            Some(_) => (library::LIBRARY_FOLDER, true),
            None => (COMPOSITES, false),
        };
        let stem = path
            .strip_prefix(&format!("{folder}/"))
            .and_then(|p| p.strip_suffix(".toml"))
            .unwrap_or(path)
            .to_owned();
        let name = parsed.composite.name;
        let name_span = name.span();
        let name = name.into_inner();
        // ADR 0027 §4: a project composite named as a library one.
        let shadowed = [name.as_str(), stem.as_str()]
            .into_iter()
            .filter_map(|n| n.strip_prefix(LIBRARY_PREFIX))
            .find(|bare| self.library.get(bare).is_some())
            .filter(|_| !in_library)
            .map(str::to_owned);
        if let Some(bare) = shadowed {
            self.error(
                file,
                name_span.start,
                format!(
                    "{path}: a project composite cannot be named `{LIBRARY_PREFIX}{bare}`: that is the library composite `{LIBRARY_PREFIX}{bare}`, which a project composite may not shadow; `signal vendor <project> {LIBRARY_PREFIX}{bare}` copies it into composites/{bare}.toml"
                ),
            );
            return None;
        }
        if !valid_name(&name) {
            self.error(
                file,
                name_span.start,
                format!(
                    "invalid composite name `{name}`; names use a-z, 0-9, _ and start with a-z"
                ),
            );
        } else if name != stem {
            self.error(
                file,
                name_span.start,
                format!("composite `{name}` must be defined in {folder}/{name}.toml, not {path}"),
            );
        }
        let defined = if in_library {
            format!("{LIBRARY_PREFIX}{name}")
        } else {
            name
        };
        let templates = self.templates;
        let (def, mut spans, problems) = definition(
            file,
            defined,
            name_span,
            parsed.instances,
            parsed.wiring,
            parsed.exports,
            templates,
        );
        for (offset, message) in problems {
            self.error(file, offset, message);
        }
        spans.category = parsed
            .composite
            .category
            .map(|c| (c.get_ref().clone(), c.span()));
        spans.description = parsed.composite.description;
        Some((def, spans))
    }
}

/// Whether a `load` value names a project (ADR 0037 §2): it contains `/` or
/// ends in `.signal`; every other value is a module reference.
pub(crate) fn is_project_path(load: &str) -> bool {
    load.contains('/') || load.ends_with(".signal")
}

/// A graph definition and its spans from the parsed tables of `file`, and
/// the problems found in its instances' project and slot keys (offsets in
/// `file`). `templates` are the project's interface templates.
fn definition(
    file: usize,
    name: String,
    name_span: Span,
    instances: Option<Spanned<Instances>>,
    wiring: Wiring,
    exports: model::ExportsSection,
    templates: &BTreeMap<String, slot::Interface>,
) -> (CompositeDef, DefSpans, Vec<slot::Problem>) {
    let mut problems = Vec::new();
    let mut spans = DefSpans {
        file,
        name: name_span,
        instances_table: instances.as_ref().map(Spanned::span),
        instances: BTreeMap::new(),
        connections: Vec::new(),
        exports: Vec::new(),
        category: None,
        description: None,
        contents: BTreeMap::new(),
    };
    let mut def = CompositeDef {
        name,
        ..CompositeDef::default()
    };

    for (key, entry) in instances.map(Spanned::into_inner).unwrap_or_default() {
        let key_span = key.span();
        let instance = key.into_inner();
        let entry_span = entry.span();
        let entry = entry.into_inner();
        let gpu = entry.gpu.map(|g| (g.get_ref().clone(), g.span()));
        let pattern = entry.pattern.map(|p| (p.get_ref().clone(), p.span()));
        let mut snapshot_exclude: Vec<(String, Span)> = Vec::new();
        for name in entry
            .snapshot_exclude
            .map(Spanned::into_inner)
            .unwrap_or_default()
        {
            let span = name.span();
            let name = name.into_inner();
            if snapshot_exclude.iter().any(|(n, _)| *n == name) {
                problems.push((
                    span.start,
                    format!("{instance}: `{name}` is in `snapshot_exclude` twice"),
                ));
            } else {
                snapshot_exclude.push((name, span));
            }
        }
        let mut parameters = BTreeMap::new();
        let mut parameter_spans = BTreeMap::new();
        for (key, value) in entry.parameters {
            let spans = (key.span(), value.span());
            let key = key.into_inner();
            parameters.insert(key.clone(), value.into_inner());
            parameter_spans.insert(key, spans);
        }
        // What the instance is: a module (its reference), an included
        // project or a loaded slot (a placeholder until the include is
        // read: an empty `module`), or an empty slot (no instance).
        let (module, module_span, planned) = match (&entry.module, &entry.project, &entry.slot) {
            (Some(module), _, _) => (module.get_ref().clone(), module.span(), true),
            (None, Some(project), _) => (String::new(), project.span(), true),
            (None, None, Some(slot)) => match &entry.load {
                Some(load) if is_project_path(load.get_ref()) => (String::new(), load.span(), true),
                Some(load) => (load.get_ref().clone(), load.span(), true),
                None => (String::new(), slot.span(), false),
            },
            (None, None, None) => (String::new(), entry_span.clone(), false),
        };
        if entry.module.is_none() {
            for (key, given) in [("gpu", &gpu), ("pattern", &pattern)] {
                if let Some((_, span)) = given {
                    problems.push((
                        span.start,
                        format!(
                            "{instance}: `{key}` belongs on a primitive module instance, not on a project or a slot"
                        ),
                    ));
                }
            }
            let slot = entry.slot.as_ref().map(|slot| {
                let span = slot.span();
                let interface =
                    match slot::resolve(slot.get_ref(), span.start, templates) {
                        Ok(interface) => Some(interface),
                        Err(found) => {
                            problems.extend(found.into_iter().map(|(offset, message)| {
                                (offset, format!("{instance}: {message}"))
                            }));
                            None
                        }
                    };
                let map = entry
                    .map
                    .as_ref()
                    .map(|m| {
                        m.get_ref()
                            .iter()
                            .map(|(k, v)| {
                                (k.get_ref().clone(), k.span(), v.get_ref().clone(), v.span())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                locate::SlotSpans {
                    span,
                    interface,
                    load: entry.load.as_ref().map(|l| (l.get_ref().clone(), l.span())),
                    map,
                }
            });
            let seed = entry.seed.as_ref().map(|s| (*s.get_ref(), s.span()));
            if let (Some(slot), Some((_, span))) = (&slot, &seed)
                && slot
                    .load
                    .as_ref()
                    .is_none_or(|(load, _)| !is_project_path(load))
            {
                problems.push((
                    span.start,
                    format!(
                        "{instance}: `seed` is a project's seed scope; this slot loads no project"
                    ),
                ));
            }
            let set = entry
                .set
                .as_ref()
                .map(|set| {
                    set.get_ref()
                        .iter()
                        .map(|(k, v)| (k.get_ref().clone(), k.span(), *v.get_ref()))
                        .collect()
                })
                .unwrap_or_default();
            spans.contents.insert(
                instance.clone(),
                locate::ContentSpans {
                    project: entry
                        .project
                        .as_ref()
                        .map(|p| (p.get_ref().clone(), p.span())),
                    slot,
                    seed,
                    set,
                    included: None,
                },
            );
        }
        spans.instances.insert(
            instance.clone(),
            InstanceSpans {
                key: key_span,
                module: module.clone(),
                module_span,
                parameters: parameter_spans,
                gpu,
                pattern,
                snapshot_exclude,
            },
        );
        if planned {
            def.instances.push(InstanceDef {
                name: instance,
                module,
                parameters,
                scope: None,
            });
        }
    }

    for connection in wiring.connections {
        let entry = connection.span();
        let connection = connection.into_inner();
        let from = (connection.from.get_ref().clone(), connection.from.span());
        let to = (connection.to.get_ref().clone(), connection.to.span());
        def.connections.push(ConnectionDef {
            from: from.0.clone(),
            to: to.0.clone(),
            scale: connection.scale.as_ref().map(|s| *s.get_ref()),
        });
        spans.connections.push(ConnectionSpans {
            entry,
            from,
            to,
            scale: connection.scale.as_ref().map(|s| (*s.get_ref(), s.span())),
        });
    }

    let mut exported = Exports::default();
    for (name, targets, group) in fan_out(exports.inputs) {
        exported.inputs.push(InputExport {
            name: name.0.clone(),
            to: targets.iter().map(|(t, _)| t.clone()).collect(),
        });
        spans
            .exports
            .push(export_spans(ExportKind::Input, name, targets, group));
    }
    for (name, target) in exports.outputs {
        let (key, span) = ((name.get_ref().clone(), name.span()), target.span());
        let target = target.into_inner();
        let targets = vec![(target.from.clone(), span)];
        exported.outputs.push(OutputExport {
            name: key.0.clone(),
            from: target.from,
        });
        spans
            .exports
            .push(export_spans(ExportKind::Output, key, targets, target.group));
    }
    for (name, targets, group) in fan_out(exports.parameters) {
        exported.parameters.push(ParameterExport {
            name: name.0.clone(),
            to: targets.iter().map(|(t, _)| t.clone()).collect(),
        });
        spans
            .exports
            .push(export_spans(ExportKind::Parameter, name, targets, group));
    }
    def.exports = exported;
    (def, spans, problems)
}

/// An export name and its targets, each with its span, and its schematic group.
type FanOut = ((String, Span), Vec<(String, Span)>, Option<String>);

/// Export tables as names and target lists; a single target string is a list
/// of one at the string's span (the table form's: the table's span).
fn fan_out(table: ExportTargets) -> Vec<FanOut> {
    fn list(targets: Targets, span: Span) -> Vec<(String, Span)> {
        match targets {
            Targets::One(target) => vec![(target, span)],
            Targets::Many(list) => list
                .into_iter()
                .map(|t| (t.get_ref().clone(), t.span()))
                .collect(),
            Targets::Grouped(inner, _) => list(*inner, span),
        }
    }
    table
        .into_iter()
        .map(|(name, targets)| {
            let span = targets.span();
            let targets = targets.into_inner();
            let group = targets.group().map(str::to_owned);
            (
                (name.get_ref().clone(), name.span()),
                list(targets, span),
                group,
            )
        })
        .collect()
}

fn export_spans(
    kind: ExportKind,
    name: (String, Span),
    targets: Vec<(String, Span)>,
    group: Option<String>,
) -> ExportSpans {
    ExportSpans {
        kind,
        name: name.0,
        key: name.1,
        targets,
        group,
    }
}
