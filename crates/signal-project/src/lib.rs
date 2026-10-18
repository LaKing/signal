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
mod structure;
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
pub use library::{LIBRARY_FOLDER, Library, LibraryComposite, is_library_file};
pub use pattern::{
    MAX_LANES, MAX_LENGTH, MAX_NOTES, MAX_STEP, MAX_STEPS, NOTES, PATTERNS, STEPS, pattern_kind,
};
pub use schema::schema;
pub use structure::{library_modules, library_parameters};
pub use write::{WriteError, write_parameter};

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
    /// Every pattern in `patterns/` by name, with the index of its file.
    patterns: BTreeMap<String, (usize, Pattern)>,
    spans: SourceMap,
    /// The byte offsets in `project.toml` of the `sample_rate` and `seed`
    /// values (0 for a key left out).
    settings_at: (usize, usize),
}

/// A validated plan and its warnings.
pub struct Planned {
    /// The execution plan.
    pub plan: ExecutionPlan,
    /// Graph warnings, located in the files.
    pub warnings: Vec<Diagnostic>,
}

/// A project folder's files as read: `project.toml`, every
/// `composites/*.toml` and every `patterns/*.toml`, each folder in name
/// order, without checking what they say (a project that does not load can
/// still be read and patched).
pub fn read_sources(dir: &Path) -> Result<Vec<SourceFile>, LoadError> {
    let mut paths = vec![PROJECT_FILE.to_owned()];
    for folder in [COMPOSITES, pattern::PATTERNS] {
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
    /// `composites/*.toml`.
    pub fn load(dir: &Path) -> Result<Project, LoadError> {
        Project::from_sources(read_sources(dir)?).map_err(LoadError::Invalid)
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

    /// [`from_sources`](Project::from_sources) with `library` as the library.
    pub fn from_sources_with(
        mut files: Vec<SourceFile>,
        library: &Library,
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
        let mut builder = Builder {
            files: &files,
            library,
            diagnostics: Vec::new(),
        };
        let root = builder.project();
        let pattern_prefix = format!("{}/", pattern::PATTERNS);
        let is_pattern = |file: usize| {
            files
                .get(file)
                .is_some_and(|f| f.path.starts_with(&pattern_prefix))
        };
        let composites: Vec<_> = (1..files.len())
            .filter(|&file| !is_pattern(file))
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
        let Some((settings, root, root_spans)) = root else {
            return Err(builder.diagnostics);
        };
        let names: BTreeSet<&str> = composites.iter().map(|(c, _)| c.name.as_str()).collect();
        builder.check_pins(&root_spans, &names);
        builder.check_patterns(&root_spans, &names, &pattern_names);
        builder.check_shadowing(&root_spans, &names);
        for (_, spans) in &composites {
            builder.check_pins(spans, &names);
            builder.check_patterns(spans, &names, &pattern_names);
            builder.check_shadowing(spans, &names);
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
        Ok(Project {
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
        })
    }

    /// Builds the execution plan with the module types of `registry`.
    ///
    /// Every composite file is validated: those the project reaches through
    /// its instances as part of the project, every other one as if it were
    /// instantiated once under its own name (its warnings are dropped). Errors
    /// of the first failing graph stage are reported, as `build_plan` does.
    pub fn plan(&self, registry: &Registry) -> Result<Planned, Vec<Diagnostic>> {
        let mut errors = Vec::new();
        let root = Locator {
            map: &self.spans,
            root: Root::Project,
        };
        // The project's composites and the library composites it references
        // (ADR 0027 §4), resolved alike by their module references.
        let definitions = self.definitions();
        let planned = match build_plan(&self.root, &definitions, registry, PLAN_VERSION) {
            Ok(plan) => {
                let mut warnings = Vec::new();
                for warning in &plan.warnings {
                    let location = root.warning(warning);
                    push_unique(
                        &mut warnings,
                        self.diagnostic(Severity::Warning, location, warning.to_string()),
                    );
                }
                Some(Planned { plan, warnings })
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
            if reachable.contains(composite.name.as_str()) {
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
                }],
                ..CompositeDef::default()
            };
            if let Err(graph_errors) = build_plan(&synthetic, &definitions, registry, PLAN_VERSION)
            {
                let locator = Locator {
                    map: &self.spans,
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
        // Library files are numbered after the project's.
        let library = location
            .file
            .checked_sub(self.files.len())
            .and_then(|i| self.library.get(i))
            .map(|l| &l.source);
        match self.files.get(location.file).or(library) {
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
        let (def, spans) = definition(
            0,
            String::new(),
            0..0,
            parsed.instances,
            parsed.wiring,
            Default::default(),
        );
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
        let (def, mut spans) = definition(
            file,
            defined,
            name_span,
            parsed.instances,
            parsed.wiring,
            parsed.exports,
        );
        spans.category = parsed
            .composite
            .category
            .map(|c| (c.get_ref().clone(), c.span()));
        spans.description = parsed.composite.description;
        Some((def, spans))
    }
}

/// A graph definition and its spans from the parsed tables of `file`.
fn definition(
    file: usize,
    name: String,
    name_span: Span,
    instances: Option<Spanned<Instances>>,
    wiring: Wiring,
    exports: model::ExportsSection,
) -> (CompositeDef, DefSpans) {
    let mut spans = DefSpans {
        file,
        name: name_span,
        instances_table: instances.as_ref().map(Spanned::span),
        instances: BTreeMap::new(),
        connections: Vec::new(),
        exports: Vec::new(),
        category: None,
        description: None,
    };
    let mut def = CompositeDef {
        name,
        ..CompositeDef::default()
    };

    for (key, entry) in instances.map(Spanned::into_inner).unwrap_or_default() {
        let key_span = key.span();
        let instance = key.into_inner();
        let entry = entry.into_inner();
        let module_span = entry.module.span();
        let module = entry.module.into_inner();
        let gpu = entry.gpu.map(|g| (g.get_ref().clone(), g.span()));
        let pattern = entry.pattern.map(|p| (p.get_ref().clone(), p.span()));
        let mut parameters = BTreeMap::new();
        let mut parameter_spans = BTreeMap::new();
        for (key, value) in entry.parameters {
            let spans = (key.span(), value.span());
            let key = key.into_inner();
            parameters.insert(key.clone(), value.into_inner());
            parameter_spans.insert(key, spans);
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
            },
        );
        def.instances.push(InstanceDef {
            name: instance,
            module,
            parameters,
        });
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
    for (name, targets) in fan_out(exports.inputs) {
        exported.inputs.push(InputExport {
            name: name.0.clone(),
            to: targets.iter().map(|(t, _)| t.clone()).collect(),
        });
        spans
            .exports
            .push(export_spans(ExportKind::Input, name, targets));
    }
    for (name, target) in exports.outputs {
        let (key, target) = ((name.get_ref().clone(), name.span()), target);
        let targets = vec![(target.get_ref().clone(), target.span())];
        exported.outputs.push(OutputExport {
            name: key.0.clone(),
            from: target.into_inner(),
        });
        spans
            .exports
            .push(export_spans(ExportKind::Output, key, targets));
    }
    for (name, targets) in fan_out(exports.parameters) {
        exported.parameters.push(ParameterExport {
            name: name.0.clone(),
            to: targets.iter().map(|(t, _)| t.clone()).collect(),
        });
        spans
            .exports
            .push(export_spans(ExportKind::Parameter, name, targets));
    }
    def.exports = exported;
    (def, spans)
}

/// An export name and its targets, each with its span.
type FanOut = ((String, Span), Vec<(String, Span)>);

/// Export tables as names and target lists; a single target string is a list
/// of one at the string's span.
fn fan_out(table: ExportTargets) -> Vec<FanOut> {
    table
        .into_iter()
        .map(|(name, targets)| {
            let span = targets.span();
            let targets = match targets.into_inner() {
                Targets::One(target) => vec![(target, span)],
                Targets::Many(list) => list
                    .into_iter()
                    .map(|t| (t.get_ref().clone(), t.span()))
                    .collect(),
            };
            ((name.get_ref().clone(), name.span()), targets)
        })
        .collect()
}

fn export_spans(
    kind: ExportKind,
    name: (String, Span),
    targets: Vec<(String, Span)>,
) -> ExportSpans {
    ExportSpans {
        kind,
        name: name.0,
        key: name.1,
        targets,
    }
}
