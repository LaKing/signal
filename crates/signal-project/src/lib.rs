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

mod diagnostic;
mod diff;
mod locate;
mod model;
mod schema;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use signal_core::{InstanceAddress, Registry};
use signal_graph::{
    CompositeDef, ConnectionDef, ExecutionPlan, Exports, InputExport, InstanceDef, OutputExport,
    ParameterExport, build_plan,
};
use toml::Spanned;

pub use diagnostic::{Diagnostic, Severity};
pub use diff::{DiffOptions, ParameterChange, ProjectEdit, diff, diff_with};
pub use schema::schema;

use locate::{
    ConnectionSpans, DefSpans, ExportKind, ExportSpans, InstanceSpans, Location, Locator, Root,
    SourceMap, Span,
};
use model::{CompositeFile, ExportTargets, Instances, ProjectFile, Targets, Wiring};

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

impl Project {
    /// Loads the project folder `dir`: `project.toml` and every
    /// `composites/*.toml`.
    pub fn load(dir: &Path) -> Result<Project, LoadError> {
        let mut paths = vec![PROJECT_FILE.to_owned()];
        let composites = dir.join(COMPOSITES);
        match fs::read_dir(&composites) {
            Ok(entries) => {
                let mut names = Vec::new();
                for entry in entries {
                    let entry = entry.map_err(|error| LoadError::Io {
                        path: composites.clone(),
                        error,
                    })?;
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "toml") && path.is_file() {
                        names.push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
                names.sort();
                paths.extend(names.into_iter().map(|n| format!("{COMPOSITES}/{n}")));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(LoadError::Io {
                    path: composites,
                    error,
                });
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
        Project::from_sources(files).map_err(LoadError::Invalid)
    }

    /// Loads a project from in-memory files: `project.toml` and any
    /// `composites/<name>.toml`.
    pub fn from_sources(mut files: Vec<SourceFile>) -> Result<Project, Vec<Diagnostic>> {
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
            diagnostics: Vec::new(),
        };
        let root = builder.project();
        let composites: Vec<_> = (1..files.len())
            .filter_map(|file| builder.composite(file))
            .collect();
        let Some((settings, root, root_spans)) = root else {
            return Err(builder.diagnostics);
        };
        if !builder.diagnostics.is_empty() {
            return Err(builder.diagnostics);
        }
        let (composites, spans): (Vec<_>, Vec<_>) = composites.into_iter().unzip();
        let spans = SourceMap {
            root: root_spans,
            composites: composites
                .iter()
                .map(|c: &CompositeDef| c.name.clone())
                .zip(spans)
                .collect(),
        };
        Ok(Project {
            name: settings.name,
            sample_rate: settings.sample_rate,
            seed: settings.seed,
            files,
            root,
            composites,
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
        let planned = match build_plan(&self.root, &self.composites, registry, PLAN_VERSION) {
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
        for composite in &self.composites {
            if reachable.contains(composite.name.as_str()) {
                continue;
            }
            let synthetic = CompositeDef {
                name: String::new(),
                instances: vec![InstanceDef {
                    name: composite.name.clone(),
                    module: composite.name.clone(),
                    parameters: BTreeMap::new(),
                }],
                ..CompositeDef::default()
            };
            if let Err(graph_errors) =
                build_plan(&synthetic, &self.composites, registry, PLAN_VERSION)
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

        match planned {
            Some(planned) if errors.is_empty() => Ok(planned),
            _ => Err(errors),
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

    /// The composites reachable from the project root through instance module
    /// references.
    fn reachable_composites(&self) -> BTreeSet<&str> {
        let by_name: BTreeMap<&str, &CompositeDef> = self
            .composites
            .iter()
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
        match self.files.get(location.file) {
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
        let stem = path
            .strip_prefix(&format!("{COMPOSITES}/"))
            .and_then(|p| p.strip_suffix(".toml"))
            .unwrap_or(path)
            .to_owned();
        let name = parsed.composite.name;
        let name_span = name.span();
        let name = name.into_inner();
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
                format!("composite `{name}` must be defined in composites/{name}.toml, not {path}"),
            );
        }
        Some(definition(
            file,
            name,
            name_span,
            parsed.instances,
            parsed.wiring,
            parsed.exports,
        ))
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
