//! A project as content (ADR 0035): projects included as instances
//! (`drums = { project = "../studio" }`) or loaded into slots (ADR 0037).
//!
//! An included project is read and checked by every rule, in its own
//! folder, then **qualified**: its definitions are renamed
//! `<id>/<name>` (its root `<id>/project.toml`), where the id is its
//! canonical folder, so they never collide with the host's composites, with
//! `lib.` ones or with another project's, and two instances of one project
//! share them. Its files, spans and patterns are merged into the host's, so
//! a diagnostic is located in the included file, named relative to the
//! host's folder (`../studio/composites/kick909.toml:7:3`).
//!
//! The including instance becomes an ordinary composite instance of the
//! qualified root and a **scope root** (`signal_graph::ScopeDef`) with the
//! include's seed: the graph and the engine know nothing else. When the plan
//! is built, every sink inside an include is planned as its tap
//! (`Registry::sink_tap`, ADR 0035 §3), and a root without `[exports]
//! outputs` exports its sink's channels (`l`, `r`) from the tap.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use signal_core::{Direction, PortShape, Registry, fnv1a64};
use signal_graph::{CompositeDef, OutputExport, ScopeDef};

use crate::diagnostic::{Diagnostic, Severity};
use crate::library::{Library, is_library_file};
use crate::locate::DefSpans;
use crate::{PROJECT_FILE, Project, SourceFile, is_project_path, read_sources};

/// Includes and project loads nest this deep at most (ADR 0035 §8): a
/// guard, not a semantic limit.
pub const MAX_DEPTH: usize = 8;

/// File indices at or above this are an included project's files (in
/// [`Content::files`]); below it, the project's own, then its library's.
pub(crate) const INCLUDED: usize = usize::MAX / 4;

/// One instance that includes a project, or a slot that loads one.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    /// The definition naming it (`""`: the project root).
    pub(crate) def: String,
    /// Its instance name.
    pub(crate) instance: String,
    /// The path as written.
    pub(crate) written: String,
    /// The project's canonical folder.
    pub(crate) canonical: PathBuf,
    /// The seed scope: the entry's `seed`, else the project's own.
    pub(crate) seed: u64,
    /// Whether it is a slot's `load`.
    pub(crate) slot: bool,
    /// Its `set` overrides (`"<inner>.<parameter>"`, value); with any, the
    /// instance plays a copy of the root with them applied, named here.
    pub(crate) set: Vec<(String, f64)>,
    /// Where each `set` key is written: its file and offset.
    pub(crate) set_at: Vec<(usize, usize)>,
    /// The copy's definition name, when `set` is not empty.
    pub(crate) variant: Option<String>,
}

/// A project included directly.
#[derive(Clone)]
pub(crate) struct Included {
    /// The project, qualified.
    pub(crate) project: Box<Project>,
    /// Its folder relative to the including project's (`../studio`).
    pub(crate) shown: String,
}

/// What a project's includes bring.
#[derive(Clone, Default)]
pub(crate) struct Content {
    /// The project's own canonical folder, when it was loaded from one.
    pub(crate) dir: Option<PathBuf>,
    /// The project's qualifier, when it is itself included.
    pub(crate) id: Option<String>,
    /// Every entry of the project's own files, in definition then instance
    /// order.
    pub(crate) entries: Vec<Entry>,
    /// The projects included directly, by canonical folder.
    pub(crate) projects: BTreeMap<PathBuf, Included>,
    /// Every definition the includes bring, qualified, sinks as written:
    /// their roots and composites, the `set` copies, and their own
    /// includes'. In the order merged; no name twice.
    pub(crate) defs: Vec<CompositeDef>,
    /// Their files, named relative to this project's folder: file index
    /// [`INCLUDED`]` + i` is `files[i]`.
    pub(crate) files: Vec<SourceFile>,
    /// The qualifiers of every project included, at any depth, and this
    /// project's own when it is included.
    pub(crate) ids: Vec<String>,
    /// Warnings found while loading (an absolute path), shown with the
    /// plan's warnings.
    pub(crate) warnings: Vec<Diagnostic>,
}

/// An included project as `describe` and the manifest show it (ADR 0035
/// §8, §13).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IncludeInfo {
    /// The including instance: `drums`, or `<composite>.<instance>` for one
    /// inside a project composite.
    pub key: String,
    /// The path as written.
    pub path: String,
    /// Whether it is a slot's `load`.
    pub slot: bool,
    /// The included project's `[project] name`.
    pub name: String,
    /// Its seed scope.
    pub seed: u64,
    /// The rate it is written for (`[project] sample_rate`).
    pub sample_rate: u32,
}

impl IncludeInfo {
    /// ADR 0035 §8: the note when the include plays at another rate than
    /// it is written for; never an error.
    pub fn rate_note(&self, rate: u32) -> Option<String> {
        (self.sample_rate != rate).then(|| {
            format!(
                "{} is written for {} Hz and plays at {rate} Hz here",
                self.name, self.sample_rate
            )
        })
    }
}

/// The including instance's key: `drums`, or `<composite>.<instance>`.
fn key(def: &str, instance: &str) -> String {
    if def.is_empty() {
        instance.to_owned()
    } else {
        format!("{def}.{instance}")
    }
}

/// `path` with `.` and `a/..` removed, lexically (`../a/../b` is `../b`).
fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// `to` relative to `from`, both canonical folders, with `/` separators.
fn relative(from: &Path, to: &Path) -> String {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

/// A file of an included project, named from the including project's
/// folder; a library file keeps its name.
fn shown_path(shown: &str, path: &str) -> String {
    if is_library_file(path) {
        path.to_owned()
    } else {
        normalize(&format!("{shown}/{path}"))
    }
}

/// `diagnostics` of a project at `shown`, named from the including folder.
pub(crate) fn reroot(shown: &str, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|d| Diagnostic {
            file: shown_path(shown, &d.file),
            ..d
        })
        .collect()
}

/// A project's canonical folder, for a cycle's chain (ADR 0035 §8).
fn folder_name(path: &Path) -> String {
    path.display().to_string()
}

/// The root definition name of the project qualified `id`.
pub(crate) fn root_name(id: &str) -> String {
    format!("{id}/{PROJECT_FILE}")
}

/// Whether `name` is an included project's root (or a `set` copy of one).
pub(crate) fn is_root(name: &str) -> bool {
    name.ends_with(&format!("/{PROJECT_FILE}")) || name.contains(&format!("/{PROJECT_FILE}@"))
}

impl Content {
    /// Reads every project the definitions `defs` (the root first) include
    /// or load into slots, and points their instances at the qualified
    /// roots. Problems are diagnostics in `files` (the project's own).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn load<'d>(
        &mut self,
        defs: impl Iterator<Item = (&'d mut CompositeDef, &'d mut DefSpans)>,
        dir: Option<&Path>,
        chain: &[PathBuf],
        library: &Library,
        diagnostics: &mut Vec<Diagnostic>,
        files: &[SourceFile],
    ) {
        self.dir = dir.map(Path::to_owned);
        for (def, spans) in defs {
            let file = spans.file;
            let error = |diagnostics: &mut Vec<Diagnostic>, offset: usize, message: String| {
                if let Some(source) = files.get(file) {
                    diagnostics.push(Diagnostic::at(
                        Severity::Error,
                        &source.path,
                        &source.text,
                        offset,
                        message,
                    ));
                }
            };
            let names: Vec<String> = spans.contents.keys().cloned().collect();
            for name in names {
                let Some(content) = spans.contents.get(&name).cloned() else {
                    continue;
                };
                let (written, at, slot) = match (&content.project, &content.slot) {
                    (Some((path, span)), _) => (path.clone(), span.start, false),
                    (None, Some(slot)) => match &slot.load {
                        Some((load, span)) if is_project_path(load) => {
                            (load.clone(), span.start, true)
                        }
                        _ => continue,
                    },
                    (None, None) => continue,
                };
                if written.ends_with(".signal") {
                    error(
                        diagnostics,
                        at,
                        format!(
                            "{name}: `{written}`: a `.signal` file cannot be included yet (ADR 0035, phase 4); `signal unpack` it and name the folder"
                        ),
                    );
                    continue;
                }
                let Some(dir) = dir else {
                    error(
                        diagnostics,
                        at,
                        format!(
                            "{name}: `{written}`: a project read from files in memory cannot include another; load it from its folder"
                        ),
                    );
                    continue;
                };
                let path = Path::new(&written);
                if path.is_absolute()
                    && let Some(source) = files.get(file)
                {
                    self.warnings.push(Diagnostic::at(
                        Severity::Warning,
                        &source.path,
                        &source.text,
                        at,
                        format!(
                            "{name}: `{written}` is an absolute path: the project is not portable"
                        ),
                    ));
                }
                let canonical = match fs::canonicalize(dir.join(path)) {
                    Ok(canonical) if canonical.join(PROJECT_FILE).is_file() => canonical,
                    _ => {
                        error(
                            diagnostics,
                            at,
                            format!(
                                "{name}: `{written}` is not a project folder (no {PROJECT_FILE} there)"
                            ),
                        );
                        continue;
                    }
                };
                if let Some(start) = chain.iter().position(|p| *p == canonical) {
                    let mut cycle: Vec<String> =
                        chain[start..].iter().map(|p| folder_name(p)).collect();
                    cycle.push(folder_name(&canonical));
                    error(
                        diagnostics,
                        at,
                        format!(
                            "{name}: `{written}` includes a project that includes it: {}",
                            cycle.join(" → ")
                        ),
                    );
                    continue;
                }
                if chain.len() > MAX_DEPTH {
                    error(
                        diagnostics,
                        at,
                        format!(
                            "{name}: `{written}`: includes and project loads nest {MAX_DEPTH} deep at most"
                        ),
                    );
                    continue;
                }
                // A file of the include is named as the path was written:
                // relative to this folder, or absolute.
                let shown = if path.is_absolute() {
                    canonical.display().to_string()
                } else {
                    relative(dir, &canonical)
                };
                if !self.projects.contains_key(&canonical) {
                    match load_included(&canonical, chain, library) {
                        Ok(project) => {
                            self.projects.insert(
                                canonical.clone(),
                                Included {
                                    project: Box::new(project),
                                    shown: shown.clone(),
                                },
                            );
                        }
                        Err(found) => {
                            let count = found.len();
                            diagnostics.extend(reroot(&shown, found));
                            error(
                                diagnostics,
                                at,
                                format!(
                                    "{name}: `{written}` does not load ({count} problem{} in its files)",
                                    if count == 1 { "" } else { "s" }
                                ),
                            );
                            continue;
                        }
                    }
                }
                let Some(included) = self.projects.get(&canonical) else {
                    continue;
                };
                let project = &included.project;
                let id = canonical.display().to_string();
                // ADR 0035 §1: `set` keys name a top-level instance of the
                // included project.
                let mut set = Vec::new();
                let mut set_at = Vec::new();
                for (key, span, value) in &content.set {
                    let inner = key.split_once('.').map(|(i, _)| i);
                    let known = inner.is_some_and(|inner| {
                        project.root.instances.iter().any(|i| i.name == inner)
                    });
                    if known {
                        set.push((key.clone(), *value));
                        set_at.push((file, span.start));
                    } else {
                        error(
                            diagnostics,
                            span.start,
                            format!(
                                "{name}: `set` key `{key}` names no top-level instance's parameter of {} (`<instance>.<parameter>`)",
                                project.name
                            ),
                        );
                    }
                }
                // The copy is named by a hash of where its entry is written,
                // so no folder of this machine shows in a diagnostic.
                let variant = (!set.is_empty()).then(|| {
                    let owner = format!("{}\n{}", dir.display(), def.name);
                    format!(
                        "{}@{:016x}.{name}",
                        root_name(&id),
                        fnv1a64(owner.as_bytes())
                    )
                });
                let module = variant.clone().unwrap_or_else(|| root_name(&id));
                let seed = content
                    .seed
                    .as_ref()
                    .map_or(project.seed, |(seed, _)| *seed);
                if let Some(instance) = def.instances.iter_mut().find(|i| i.name == name) {
                    instance.module = module.clone();
                    instance.scope = Some(ScopeDef { seed });
                }
                if let Some(instance) = spans.instances.get_mut(&name) {
                    instance.module = module;
                }
                if let Some(content) = spans.contents.get_mut(&name) {
                    content.included = Some(canonical.clone());
                }
                self.entries.push(Entry {
                    def: def.name.clone(),
                    instance: name.clone(),
                    written,
                    canonical,
                    seed,
                    slot,
                    set,
                    set_at,
                    variant,
                });
            }
        }
    }
}

/// Loads the project in `canonical`, included from the end of `chain`, and
/// qualifies it.
fn load_included(
    canonical: &Path,
    chain: &[PathBuf],
    library: &Library,
) -> Result<Project, Vec<Diagnostic>> {
    let files = read_sources(canonical).map_err(|error| {
        vec![Diagnostic {
            severity: Severity::Error,
            file: PROJECT_FILE.to_owned(),
            line: 1,
            column: 1,
            message: error.to_string(),
        }]
    })?;
    let mut chain = chain.to_vec();
    chain.push(canonical.to_owned());
    let mut project = Project::assemble(files, library, Some(canonical), &chain)?;
    project.qualify(&canonical.display().to_string());
    Ok(project)
}

impl Project {
    /// Renames this project's own definitions `<id>/<name>` (its root
    /// `<id>/project.toml`) and its patterns `<id>/<name>`, with every
    /// reference to them: it is now content of another project.
    pub(crate) fn qualify(&mut self, id: &str) {
        let own: BTreeSet<String> = self.composites.iter().map(|c| c.name.clone()).collect();
        let patterns: BTreeSet<String> = self
            .patterns
            .keys()
            .filter(|name| !name.contains('/'))
            .cloned()
            .collect();
        let qualified = |name: &str| format!("{id}/{name}");
        let rename = |module: &mut String| {
            if own.contains(module.as_str()) {
                *module = qualified(module);
            }
        };
        self.root.name = root_name(id);
        for instance in &mut self.root.instances {
            rename(&mut instance.module);
        }
        for composite in &mut self.composites {
            composite.name = qualified(&composite.name);
            for instance in &mut composite.instances {
                rename(&mut instance.module);
            }
        }
        let respan = |spans: &mut DefSpans| {
            for instance in spans.instances.values_mut() {
                rename(&mut instance.module);
                if let Some((pattern, _)) = &mut instance.pattern
                    && patterns.contains(pattern.as_str())
                {
                    *pattern = qualified(pattern);
                }
            }
        };
        respan(&mut self.spans.root);
        let composites = std::mem::take(&mut self.spans.composites);
        for (name, mut spans) in composites {
            if own.contains(&name) {
                respan(&mut spans);
                self.spans.composites.insert(qualified(&name), spans);
            } else {
                self.spans.composites.insert(name, spans);
            }
        }
        let stored = std::mem::take(&mut self.patterns);
        for (name, pattern) in stored {
            let name = if patterns.contains(&name) {
                qualified(&name)
            } else {
                name
            };
            self.patterns.insert(name, pattern);
        }
        self.content.id = Some(id.to_owned());
        self.content.ids.push(id.to_owned());
    }

    /// Merges what every directly included project brings into this one:
    /// its definitions, files, spans, patterns and library composites, then
    /// makes the `set` copies.
    pub(crate) fn merge_included(&mut self) {
        let projects = std::mem::take(&mut self.content.projects);
        for included in projects.values() {
            let project = &included.project;
            let base = self.content.files.len();
            let (own, library) = (project.files.len(), project.library.len());
            let remap = |file: usize| {
                INCLUDED
                    + base
                    + if file >= INCLUDED {
                        own + library + (file - INCLUDED)
                    } else {
                        file
                    }
            };
            let shown = &included.shown;
            let files = project
                .files
                .iter()
                .chain(project.library.iter().map(|l| &l.source))
                .chain(&project.content.files);
            for file in files {
                self.content.files.push(SourceFile {
                    path: shown_path(shown, &file.path),
                    text: file.text.clone(),
                });
            }
            let mut root = project.spans.root.clone();
            root.file = remap(root.file);
            self.spans
                .composites
                .entry(project.root.name.clone())
                .or_insert(root);
            for (name, spans) in &project.spans.composites {
                let mut spans = spans.clone();
                spans.file = remap(spans.file);
                self.spans.composites.entry(name.clone()).or_insert(spans);
            }
            let defs = std::iter::once(&project.root)
                .chain(&project.composites)
                .chain(&project.content.defs);
            for def in defs {
                if !self.content.defs.iter().any(|d| d.name == def.name) {
                    self.content.defs.push(def.clone());
                }
            }
            for composite in &project.library {
                if !self.library.iter().any(|l| l.name == composite.name) {
                    self.library.push(composite.clone());
                }
            }
            for (name, (file, pattern)) in &project.patterns {
                self.patterns
                    .entry(name.clone())
                    .or_insert_with(|| (remap(*file), pattern.clone()));
            }
            for id in &project.content.ids {
                if !self.content.ids.contains(id) {
                    self.content.ids.push(id.clone());
                }
            }
            let warnings = project.content.warnings.clone();
            self.content.warnings.extend(reroot(shown, warnings));
        }
        self.content.projects = projects;

        // The `set` copies: the root with the overrides on its instances.
        let entries = self.content.entries.clone();
        for entry in entries {
            let Some(variant) = &entry.variant else {
                continue;
            };
            let id = entry.canonical.display().to_string();
            let root = root_name(&id);
            let Some(mut def) = self.content.defs.iter().find(|d| d.name == root).cloned() else {
                continue;
            };
            def.name = variant.clone();
            for (key, value) in &entry.set {
                let Some((inner, parameter)) = key.split_once('.') else {
                    continue;
                };
                if let Some(instance) = def.instances.iter_mut().find(|i| i.name == inner) {
                    instance.parameters.insert(parameter.to_owned(), *value);
                }
            }
            if let Some(spans) = self.spans.composites.get(&root).cloned() {
                self.spans.composites.insert(variant.clone(), spans);
            }
            self.content.defs.push(def);
        }
    }

    /// The projects this one includes or loads into slots, directly, in
    /// definition then instance order (ADR 0035 §8: each with the rate it
    /// is written for, to note when it plays at another).
    pub fn includes(&self) -> Vec<IncludeInfo> {
        self.content
            .entries
            .iter()
            .filter_map(|entry| {
                let project = &self.content.projects.get(&entry.canonical)?.project;
                Some(IncludeInfo {
                    key: key(&entry.def, &entry.instance),
                    path: entry.written.clone(),
                    slot: entry.slot,
                    name: project.name.clone(),
                    seed: entry.seed,
                    sample_rate: project.sample_rate,
                })
            })
            .collect()
    }

    /// ADR 0035 §8: a note for every include written for another rate than
    /// `rate`, the rate the host plays at. Never an error.
    pub fn rate_notes(&self, rate: u32) -> Vec<String> {
        self.includes()
            .iter()
            .filter_map(|include| include.rate_note(rate))
            .collect()
    }

    /// ADR 0035 §13: the files of every include, by
    /// `include:<key>/<path>` (an include's own includes nested the same
    /// way), with their FNV-1a 64: what the manifest's
    /// `project_file_hashes` adds. Empty without includes.
    pub fn include_file_hashes(&self) -> BTreeMap<String, String> {
        let mut hashes = BTreeMap::new();
        for entry in &self.content.entries {
            let Some(included) = self.content.projects.get(&entry.canonical) else {
                continue;
            };
            let prefix = format!("include:{}/", key(&entry.def, &entry.instance));
            for (path, hash) in included.project.file_hashes() {
                hashes.insert(format!("{prefix}{path}"), hash);
            }
        }
        hashes
    }

    /// Every project file's hash and every include's
    /// ([`include_file_hashes`](Project::include_file_hashes)).
    fn file_hashes(&self) -> BTreeMap<String, String> {
        let mut hashes: BTreeMap<String, String> = self
            .files
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    format!("{:016x}", fnv1a64(f.text.as_bytes())),
                )
            })
            .collect();
        hashes.extend(self.include_file_hashes());
        hashes
    }

    /// ADR 0035 §13: the manifest's `included_projects`, one per include
    /// entry; empty without includes.
    pub fn included_projects(&self) -> Vec<signal_engine::IncludedProject> {
        self.content
            .entries
            .iter()
            .filter_map(|entry| {
                let project = &self.content.projects.get(&entry.canonical)?.project;
                let lines: String = project
                    .file_hashes()
                    .iter()
                    .map(|(path, hash)| format!("{path} {hash}\n"))
                    .collect();
                Some(signal_engine::IncludedProject {
                    key: key(&entry.def, &entry.instance),
                    path: entry.written.clone(),
                    name: project.name.clone(),
                    seed: entry.seed,
                    files_fnv1a64: format!("{:016x}", fnv1a64(lines.as_bytes())),
                })
            })
            .collect()
    }

    /// ADR 0035 §1: every `set` key names a parameter of the included
    /// project's top-level instance (a primitive's declared parameter, a
    /// composite's exported one); an error at the key otherwise.
    pub(crate) fn check_set(
        &self,
        defs: &[CompositeDef],
        registry: &Registry,
        errors: &mut Vec<Diagnostic>,
    ) {
        for entry in &self.content.entries {
            let Some(included) = self.content.projects.get(&entry.canonical) else {
                continue;
            };
            for ((key, _), &(file, offset)) in entry.set.iter().zip(&entry.set_at) {
                let Some((inner, parameter)) = key.split_once('.') else {
                    continue;
                };
                let Some(instance) = included
                    .project
                    .root
                    .instances
                    .iter()
                    .find(|i| i.name == inner)
                else {
                    continue;
                };
                let known = match signal_graph::ports_of(&instance.module, defs, registry) {
                    Ok(ports) => ports.iter().any(|p| p.parameter && p.name == parameter),
                    // A broken module is reported where it is written.
                    Err(_) => true,
                };
                if !known {
                    let message = format!(
                        "{}: `set` key `{key}`: `{inner}` of {} has no parameter `{parameter}`",
                        entry.instance, included.project.name
                    );
                    let diagnostic = self.diagnostic(
                        Severity::Error,
                        crate::locate::Location { file, offset },
                        message,
                    );
                    if !errors.contains(&diagnostic) {
                        errors.push(diagnostic);
                    }
                }
            }
        }
    }

    /// The included definitions as the plan uses them (ADR 0035 §2–3):
    /// every sink planned as its tap, and a root without `[exports]
    /// outputs` exporting its sinks' channels from the taps. A sink type
    /// without a tap is an error at the instance, in its file.
    pub(crate) fn tapped(
        &self,
        registry: &Registry,
        errors: &mut Vec<Diagnostic>,
    ) -> Vec<CompositeDef> {
        let mut defs = self.content.defs.clone();
        for def in &mut defs {
            let root = is_root(&def.name);
            let mut defaults = Vec::new();
            for instance in &mut def.instances {
                let Some(module_type) = registry.get(&instance.module) else {
                    continue;
                };
                let descriptor = module_type.descriptor;
                if descriptor.sink.is_none() {
                    continue;
                }
                let Some(tap) = registry.sink_tap(descriptor.type_name) else {
                    let location = self
                        .spans
                        .composites
                        .get(&def.name)
                        .and_then(|spans| {
                            let at = spans.instances.get(&instance.name)?.module_span.start;
                            Some(crate::locate::Location {
                                file: spans.file,
                                offset: at,
                            })
                        })
                        .unwrap_or(crate::locate::Location { file: 0, offset: 0 });
                    let message = format!(
                        "{}: `{}` is a sink with no tap, so this project cannot play inside another (ADR 0035 §3)",
                        instance.name, descriptor.type_name
                    );
                    let diagnostic = self.diagnostic(Severity::Error, location, message);
                    if !errors.contains(&diagnostic) {
                        errors.push(diagnostic);
                    }
                    continue;
                };
                instance.module = tap.to_owned();
                if root {
                    let channels = descriptor
                        .ports
                        .iter()
                        .filter(|p| p.direction == Direction::In && p.shape == PortShape::Signal);
                    for port in channels {
                        defaults.push(OutputExport {
                            name: port.name.to_owned(),
                            from: format!("{}.{}_out", instance.name, port.name),
                        });
                    }
                }
            }
            // ADR 0035 §2: no `[exports] outputs`: the sinks' channels.
            if root && def.exports.outputs.is_empty() {
                def.exports.outputs = defaults;
            }
        }
        defs
    }
}

impl Project {
    /// ADR 0037 §3: the library interface templates this project's slots
    /// (and its includes') start from, with their text hashes: the
    /// manifest's `library_interface_hashes`. Empty without one.
    pub fn library_interface_hashes(&self) -> BTreeMap<String, String> {
        let defs = std::iter::once(&self.spans.root).chain(self.spans.composites.values());
        defs.flat_map(|d| d.contents.values())
            .filter_map(|c| c.slot.as_ref()?.interface.as_ref()?.from.clone())
            .filter_map(|from| {
                let name = from.strip_prefix(crate::slot::LIBRARY_TEMPLATE)?;
                Some((from.clone(), signal_lib::interface(name)?.hash()))
            })
            .collect()
    }
}

impl Project {
    /// What `describe` lists of the project's content (ADR 0035 §8, ADR
    /// 0037 §7), when the project plays at `rate`: `{"includes": [...],
    /// "slots": [...]}`, each include with its rate `note` when it plays at
    /// another rate than it is written for, each slot with its resolved
    /// interface, load, match table, dropped wires and unheard outputs.
    /// `null` for a project with neither. Builds the plan only when the
    /// project has slots.
    pub fn content_json(&self, registry: &Registry, rate: u32) -> serde_json::Value {
        let has_slots = std::iter::once(&self.spans.root)
            .chain(self.spans.composites.values())
            .any(|d| d.contents.values().any(|c| c.slot.is_some()));
        let includes = self.includes();
        if includes.is_empty() && !has_slots {
            return serde_json::Value::Null;
        }
        let includes: Vec<serde_json::Value> = includes
            .iter()
            .map(|include| {
                let mut value = serde_json::to_value(include).unwrap_or_default();
                if let (Some(note), Some(fields)) = (include.rate_note(rate), value.as_object_mut())
                {
                    fields.insert("note".to_owned(), serde_json::Value::String(note));
                }
                value
            })
            .collect();
        let slots = if has_slots {
            self.plan(registry).map(|p| p.slots).unwrap_or_default()
        } else {
            Vec::new()
        };
        serde_json::json!({ "includes": includes, "slots": slots })
    }
}

/// The projects `files` (a project's `project.toml` and composites) include
/// or load into slots, as written, without loading anything: what `signal
/// pack` cannot pack yet (ADR 0035 §13, phase 4). A file that does not
/// parse names none.
pub fn included_paths(files: &[SourceFile]) -> Vec<String> {
    let mut found = Vec::new();
    for file in files {
        let is_definition = file.path == PROJECT_FILE
            || (file.path.starts_with("composites/") && file.path.ends_with(".toml"));
        if !is_definition {
            continue;
        }
        let Ok(table) = toml::from_str::<toml::Table>(&file.text) else {
            continue;
        };
        let Some(instances) = table.get("instances").and_then(toml::Value::as_table) else {
            continue;
        };
        for entry in instances.values().filter_map(toml::Value::as_table) {
            let path = entry
                .get("project")
                .and_then(toml::Value::as_str)
                .or_else(|| {
                    entry.get("slot")?;
                    entry
                        .get("load")
                        .and_then(toml::Value::as_str)
                        .filter(|load| is_project_path(load))
                });
            if let Some(path) = path
                && !found.iter().any(|p| p == path)
            {
                found.push(path.to_owned());
            }
        }
    }
    found
}
