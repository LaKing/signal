//! `signal vendor` (ADR 0027 §4): a library composite copied into a project's
//! `composites/`, where it is the project's own text (ADR 0009).
//!
//! The composite and every library composite it uses, transitively, are
//! copied: the first under `--as` or its name without `lib.`, the others under
//! their names without `lib.`. In the copies and in the project's own files
//! (`project.toml`, `composites/*.toml`), every reference to a copied
//! composite is rewritten from `lib.<name>` to its new name with `toml_edit`,
//! so nothing else in the text changes. Nothing is written unless every copy
//! has a free file name, every file parses and the project as it would be
//! left still loads (a copy named like a library composite the project still
//! uses is refused): an existing file is never overwritten. A dependency
//! whose file the project already has with exactly the copy's text (vendored
//! before) is kept; any other file of a copy's name refuses the whole
//! operation. The writes are all or nothing as far as the file system allows:
//! every file is first written under a temporary name, the new files are then
//! linked to their names (a link never replaces a file) and the project's
//! rewritten files renamed over theirs last; the project is loaded again from
//! disk, and on any failure the new files are removed and the originals
//! written back (never over an edit made meanwhile). [`write`] holds the
//! project's writer lock (ADR 0040) and first checks that the project's
//! files are still exactly those [`prepare`] read: a change meanwhile is a
//! conflict ([`VendorError::Changed`]), never overwritten. Instance names do not change, so the project's addresses,
//! random streams and canonical source order are the same, and it renders
//! bit-identically.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use signal_core::LIBRARY_PREFIX;
use signal_graph::{CompositeDef, InstanceDef};
use toml_edit::{DocumentMut, Item, Value};

use crate::transaction::{self, ProjectLock};
use crate::{
    COMPOSITES, Diagnostic, Library, LoadError, PROJECT_FILE, Project, SourceFile, read_sources,
    valid_name,
};

/// What [`vendor`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vendored {
    /// The new composite files, by path relative to the project
    /// (`composites/<name>.toml`), in the order written.
    pub copied: Vec<String>,
    /// The dependencies already in the project with exactly the copy's
    /// text (vendored before), kept as they are.
    pub reused: Vec<String>,
    /// The project's files whose references were rewritten.
    pub rewritten: Vec<String>,
}

/// Why nothing was vendored.
#[derive(Debug)]
pub enum VendorError {
    /// The reference is not `lib.<name>` of a library composite.
    NotInLibrary(String),
    /// `--as` is not a composite name, or it is a dependency's name.
    InvalidName(String),
    /// A copy's file exists already (it is never overwritten).
    Exists(String),
    /// A file of the project or of the library does not parse.
    Syntax {
        /// Its path.
        path: String,
        /// The parser's message.
        message: String,
    },
    /// The project would not load after vendoring (nothing is written).
    Invalid(Vec<Diagnostic>),
    /// A file of the project changed after vendoring was prepared (it is
    /// never overwritten): its path.
    Changed(String),
    /// Reading or writing a file.
    Io {
        /// The path.
        path: PathBuf,
        /// The error.
        error: io::Error,
    },
}

impl fmt::Display for VendorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VendorError::NotInLibrary(reference) => write!(
                f,
                "`{reference}` is not a library composite (`signal modules` lists them as {LIBRARY_PREFIX}<name>)"
            ),
            VendorError::InvalidName(message) => f.write_str(message),
            VendorError::Exists(path) => write!(
                f,
                "{path} exists; vendoring never overwrites a file (give another name with --as, or rename the project's composite)"
            ),
            VendorError::Syntax { path, message } => write!(f, "{path} does not parse: {message}"),
            VendorError::Invalid(diagnostics) => {
                write!(
                    f,
                    "vendoring would leave a project that does not load; nothing was written"
                )?;
                for diagnostic in diagnostics {
                    write!(f, "\n{diagnostic}")?;
                }
                Ok(())
            }
            VendorError::Changed(path) => write!(
                f,
                "{path} changed after vendoring was prepared; nothing was vendored (prepare again from the files as they are now)"
            ),
            VendorError::Io { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for VendorError {}

/// Copies the library composite `reference` (`lib.<name>`) and the library
/// composites it uses into the project folder `dir`, the first as `name`
/// (default: its name without `lib.`), and rewrites the project's references
/// to them: [`prepare`], then [`write`].
pub fn vendor(
    dir: &Path,
    reference: &str,
    name: Option<&str>,
    library: &Library,
) -> Result<Vendored, VendorError> {
    write(dir, prepare(dir, reference, name, library)?, library)
}

/// What vendoring would write, checked and not written yet (`signal ui`'s
/// `vendor` message checks it again as an edit of the running project before
/// it [`write`]s).
#[derive(Debug)]
pub struct Prepared {
    copies: Vec<(String, String)>,
    edits: Vec<(String, String)>,
    reused: Vec<String>,
    files: Vec<SourceFile>,
    loaded_before: bool,
    /// The WAVs the copied sample patterns read (ADR 0056), new files.
    samples: Vec<(String, Vec<u8>)>,
}

impl Prepared {
    /// Files to write by [`write`]'s all-or-nothing writer: `copies` new
    /// (never over an existing file), `edits` replacing the project's own
    /// files as `files` (read now) hold them; checked to load if the
    /// project loaded before. For copies made elsewhere (ADR 0051 §10: a
    /// Stack imported) from the project in `source`, whose sample files
    /// come too ([`carry_samples`](Prepared::carry_samples), `prefix`).
    pub fn new(
        dir: &Path,
        copies: Vec<(String, String)>,
        edits: Vec<(String, String)>,
        (source, prefix): (&Path, &str),
        library: &Library,
    ) -> Result<Prepared, VendorError> {
        let files = project_sources(dir)?;
        for (path, _) in &copies {
            if dir.join(path).exists() {
                return Err(VendorError::Exists(path.clone()));
            }
        }
        let loaded_before = Project::from_sources_in_with(files.clone(), library, dir).is_ok();
        let mut prepared = Prepared {
            copies,
            edits,
            reused: Vec::new(),
            files,
            loaded_before,
            samples: Vec::new(),
        };
        prepared.carry_samples(dir, source, prefix)?;
        if let Err(diagnostics) = prepared.check(dir, library) {
            return Err(VendorError::Invalid(diagnostics));
        }
        Ok(prepared)
    }

    /// Every file it would write (new and rewritten), relative to the project.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.copies
            .iter()
            .chain(&self.edits)
            .map(|(path, _)| path.as_str())
            .chain(self.samples.iter().map(|(path, _)| path.as_str()))
    }

    /// ADR 0056: the WAVs the sample patterns copied from the project in
    /// `source` read, copied too, each under its own path. One the project
    /// in `dir` has with the same bytes is shared; one it has with other
    /// bytes is copied as `<prefix>_<name>` beside it, and the copied
    /// patterns reading it rewritten. A pattern the project kept as its own
    /// (the same text) reads the project's WAV, so a WAV of other bytes
    /// under it refuses the copy.
    fn carry_samples(
        &mut self,
        dir: &Path,
        source: &Path,
        prefix: &str,
    ) -> Result<(), VendorError> {
        let mut moved: BTreeMap<String, String> = BTreeMap::new();
        for file in project_sources(source)? {
            if !file.path.starts_with("patterns/") {
                continue;
            }
            let Some(wav) = crate::sample_file(&file.text) else {
                continue;
            };
            let kept = !self
                .copies
                .iter()
                .any(|(path, text)| path.starts_with("patterns/") && *text == file.text);
            if moved.contains_key(&wav) && !kept {
                continue;
            }
            let bytes = read_sample(&source.join(&wav))?;
            let target = match holds(dir, &self.samples, &wav, &bytes)? {
                Some(true) => wav.clone(),
                Some(false) if kept => return Err(VendorError::Exists(wav)),
                None => {
                    self.samples.push((wav.clone(), bytes));
                    wav.clone()
                }
                Some(false) => {
                    let other = match wav.rsplit_once('/') {
                        Some((folder, name)) => format!("{folder}/{prefix}_{name}"),
                        None => format!("{prefix}_{wav}"),
                    };
                    match holds(dir, &self.samples, &other, &bytes)? {
                        Some(true) => {}
                        Some(false) => return Err(VendorError::Exists(other)),
                        None => self.samples.push((other.clone(), bytes)),
                    }
                    other
                }
            };
            moved.insert(wav, target);
        }
        for (path, text) in &mut self.copies {
            if !path.starts_with("patterns/") {
                continue;
            }
            let Some(target) = crate::sample_file(text).and_then(|wav| moved.get(&wav).cloned())
            else {
                continue;
            };
            if crate::sample_file(text).as_deref() != Some(target.as_str()) {
                let mut document = parse(path, text)?;
                if let Some(item) = document.get_mut("file") {
                    replace(item, &target);
                }
                *text = document.to_string();
            }
        }
        Ok(())
    }

    /// The project's files as they were read when it was prepared: what
    /// [`write`] checks they still are.
    pub fn before(&self) -> &[SourceFile] {
        &self.files
    }

    /// The project in `dir` as vendoring would leave it, loaded, its new
    /// sample files read as if written.
    fn check(&self, dir: &Path, library: &Library) -> Result<Project, Vec<Diagnostic>> {
        Project::from_sources_planned(self.after(), library, dir, &self.samples)
    }

    /// The project's files as vendoring would leave them.
    pub fn after(&self) -> Vec<SourceFile> {
        let mut after: Vec<SourceFile> = self
            .files
            .iter()
            .map(|file| {
                let edited = self.edits.iter().find(|(path, _)| *path == file.path);
                SourceFile {
                    path: file.path.clone(),
                    text: edited.map_or_else(|| file.text.clone(), |(_, text)| text.clone()),
                }
            })
            .collect();
        after.extend(self.copies.iter().map(|(path, text)| SourceFile {
            path: path.clone(),
            text: text.clone(),
        }));
        after
    }
}

/// Checks what vendoring `reference` into `dir` would write, writing nothing
/// (the checks of [`vendor`]: names, existing files, parsing, and that the
/// project would still load).
pub fn prepare(
    dir: &Path,
    reference: &str,
    name: Option<&str>,
    library: &Library,
) -> Result<Prepared, VendorError> {
    let not_in_library = || VendorError::NotInLibrary(reference.to_owned());
    let bare = reference
        .strip_prefix(LIBRARY_PREFIX)
        .ok_or_else(not_in_library)?;
    library.get(bare).ok_or_else(not_in_library)?;
    let top = name.unwrap_or(bare);
    if !valid_name(top) {
        return Err(VendorError::InvalidName(format!(
            "invalid composite name `{top}`; names use a-z, 0-9, _ and start with a-z"
        )));
    }

    // The composite and its dependencies, and each one's new name.
    let user = CompositeDef {
        instances: vec![InstanceDef {
            name: top.to_owned(),
            module: reference.to_owned(),
            parameters: BTreeMap::new(),
            scope: None,
        }],
        ..CompositeDef::default()
    };
    let sources = library.closure(std::iter::once(&user));
    let mut renamed = BTreeMap::new();
    for source in &sources {
        let lib_name = crate::library::library_stem(&source.path).unwrap_or(&source.path);
        let new = if lib_name == bare { top } else { lib_name };
        renamed.insert(format!("{LIBRARY_PREFIX}{lib_name}"), new.to_owned());
    }
    if let Some((dependency, _)) = renamed
        .iter()
        .find(|(old, new)| *new == top && old.as_str() != reference)
    {
        return Err(VendorError::InvalidName(format!(
            "`{top}` is the name `{dependency}` is vendored under; give another name with --as"
        )));
    }

    // Every copy, checked before anything is written. A dependency whose
    // file exists with exactly the copy's text (vendored before) is kept;
    // any other existing file refuses the whole operation.
    let mut copies = Vec::new();
    let mut reused = Vec::new();
    for source in &sources {
        let old = crate::library::library_stem(&source.path)
            .map(|n| format!("{LIBRARY_PREFIX}{n}"))
            .unwrap_or_default();
        let new = &renamed[&old];
        let path = format!("{COMPOSITES}/{new}.toml");
        let mut document = parse(&source.path, &source.text)?;
        if let Some(section) = document
            .get_mut("composite")
            .and_then(Item::as_table_like_mut)
            && let Some(item) = section.get_mut("name")
        {
            replace(item, new);
        }
        rewrite(&mut document, &renamed);
        let text = document.to_string();
        let full = dir.join(&path);
        if full.exists() {
            let same = crate::read_regular(&full).is_ok_and(|bytes| bytes == text.as_bytes());
            if old != reference && same {
                reused.push(path);
                continue;
            }
            return Err(VendorError::Exists(path));
        }
        copies.push((path, text));
    }

    // The project's own references, rewritten.
    let files = project_sources(dir)?;
    let mut edits = Vec::new();
    for file in files
        .iter()
        .filter(|f| f.path == PROJECT_FILE || f.path.starts_with(&format!("{COMPOSITES}/")))
    {
        let mut document = parse(&file.path, &file.text)?;
        if rewrite(&mut document, &renamed) {
            edits.push((file.path.clone(), document.to_string()));
        }
    }

    // The project as vendoring would leave it must load if it loaded before
    // (e.g. `--as x` beside a remaining `lib.x` reference would not: §4).
    let loaded_before = Project::from_sources_in_with(files.clone(), library, dir).is_ok();
    let prepared = Prepared {
        copies,
        edits,
        reused,
        files,
        loaded_before,
        samples: Vec::new(),
    };
    if loaded_before
        && let Err(diagnostics) = Project::from_sources_in_with(prepared.after(), library, dir)
    {
        return Err(VendorError::Invalid(diagnostics));
    }
    Ok(prepared)
}

/// ADR 0037 phase 4, ADR 0051 §10 (import): slot `slot` of the project in
/// `dir`, which links a project (`load = "<path>"`), made to hold its own
/// copy of it: the linked project's `project.toml` becomes the slot's own
/// inside (`composites/<slot>_inside.toml`, `inside_of = "<slot>"`, its
/// instances, connections and exports), its composites and patterns are
/// copied into the host (one with a name the host has with another text
/// is copied as `<slot>_<name>`, and the references to it rewritten), its
/// `ui/main.html` becomes the slot's part (`ui/parts/<slot>.html`), each
/// binding on a port an export reaches rebound to the export (`.name`),
/// and the slot loads the copy with the project's seed scope as its
/// `seed`, so it plays what the link played, bit for bit.
pub fn prepare_slot(dir: &Path, slot: &str, library: &Library) -> Result<Prepared, VendorError> {
    let files = project_sources(dir)?;
    let host_text = files
        .iter()
        .find(|f| f.path == PROJECT_FILE)
        .map(|f| f.text.clone())
        .unwrap_or_default();
    let mut host = parse(PROJECT_FILE, &host_text)?;
    let invalid = |m: String| VendorError::InvalidName(m);
    let entry = host
        .get("instances")
        .and_then(|i| i.get(slot))
        .and_then(Item::as_value)
        .and_then(Value::as_inline_table)
        .ok_or_else(|| invalid(format!("no slot `{slot}` written inline at the top level")))?;
    if entry.get("slot").is_none() {
        return Err(invalid(format!("`{slot}` is not a slot")));
    }
    if entry.get("map").is_some() {
        return Err(invalid(format!(
            "`{slot}` has a `map`: copy it after removing the map"
        )));
    }
    let load = entry
        .get("load")
        .and_then(Value::as_str)
        .filter(|l| crate::is_project_path(l))
        .ok_or_else(|| invalid(format!("`{slot}` links no project (`load = \"<path>\"`)")))?
        .to_owned();
    let source_dir = dir.join(&load);
    let source = tap_composites(project_sources(&source_dir)?)?;
    let text = |path: &str| {
        source
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.text.clone())
    };
    let source_project = parse(
        &format!("{load}/{PROJECT_FILE}"),
        &text(PROJECT_FILE).unwrap_or_default(),
    )?;
    let seed = match entry.get("seed").and_then(Value::as_integer) {
        Some(seed) => seed,
        None => source_project
            .get("project")
            .and_then(|p| p.get("seed"))
            .and_then(Item::as_integer)
            .unwrap_or(0),
    };
    let have = |path: &str| {
        files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.text.clone())
    };
    let taken = |path: &str| have(path).is_some() || dir.join(path).exists();

    // The names: a composite or pattern the host has with another text is
    // copied under `<slot>_<name>`.
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    let mut patterns: BTreeMap<String, String> = BTreeMap::new();
    let mut copies: Vec<(String, String)> = Vec::new();
    let mut reused: Vec<String> = Vec::new();
    for file in &source {
        for (prefix, names) in [(COMPOSITES, &mut renamed), ("patterns", &mut patterns)] {
            let Some(name) = file
                .path
                .strip_prefix(&format!("{prefix}/"))
                .and_then(|n| n.strip_suffix(".toml"))
            else {
                continue;
            };
            let path = format!("{prefix}/{name}.toml");
            let new = match have(&path) {
                Some(existing) if existing == file.text => {
                    reused.push(path);
                    name.to_owned()
                }
                Some(_) => format!("{slot}_{name}"),
                None => name.to_owned(),
            };
            names.insert(name.to_owned(), new);
        }
    }
    let texts: BTreeMap<String, String> = source
        .iter()
        .filter_map(|f| {
            let name = f
                .path
                .strip_prefix(&format!("{COMPOSITES}/"))?
                .strip_suffix(".toml")?;
            Some((name.to_owned(), f.text.clone()))
        })
        .collect();
    settle_reuse(&mut renamed, &patterns, &texts, &mut reused, &|name| {
        format!("{slot}_{name}")
    });
    let inside = crate::inside::default_name("", slot, &|n| {
        taken(&format!("{COMPOSITES}/{n}.toml")) || renamed.values().any(|v| v == n)
    });
    let retarget = |document: &mut DocumentMut| {
        rewrite(document, &renamed);
        if let Some(instances) = document
            .get_mut("instances")
            .and_then(Item::as_table_like_mut)
        {
            for (_, entry) in instances.iter_mut() {
                if let Some(pattern) = entry.as_table_like_mut().and_then(|e| e.get_mut("pattern"))
                    && let Some(new) = pattern.as_str().and_then(|p| patterns.get(p)).cloned()
                {
                    replace(pattern, &new);
                }
            }
        }
    };
    for file in &source {
        for (prefix, names) in [(COMPOSITES, &renamed), ("patterns", &patterns)] {
            let Some(name) = file
                .path
                .strip_prefix(&format!("{prefix}/"))
                .and_then(|n| n.strip_suffix(".toml"))
            else {
                continue;
            };
            let new = names.get(name).cloned().unwrap_or_else(|| name.to_owned());
            let path = format!("{prefix}/{new}.toml");
            if reused.contains(&path) {
                continue;
            }
            let mut document = parse(&file.path, &file.text)?;
            if prefix == COMPOSITES {
                if let Some(item) = document
                    .get_mut("composite")
                    .and_then(Item::as_table_like_mut)
                    .and_then(|c| c.get_mut("name"))
                {
                    replace(item, &new);
                }
                retarget(&mut document);
            }
            if taken(&path) {
                return Err(VendorError::Exists(path));
            }
            copies.push((path, document.to_string()));
        }
    }

    // The inside: the linked project's text as a composite, its sinks as
    // the taps they planned as while it was linked (the audit of
    // 2026-11-15: left sinks, the host would have two main outputs).
    let mut document = source_project.clone();
    document.remove("schema_version");
    document.remove("project");
    retarget(&mut document);
    let exports = tap_root(&mut document, &format!("`{slot}`'s project"))?;
    let inside_text = format!(
        "schema_version = 1\n\n[composite]\nname = {}\ninside_of = {}\n\n{}{exports}",
        Value::from(inside.as_str()),
        Value::from(slot),
        document.to_string().trim_start()
    );
    copies.push((format!("{COMPOSITES}/{inside}.toml"), inside_text));

    // The part: its panel, rebound on the exports.
    // A part the slot has already is kept.
    let part = format!("ui/parts/{slot}.html");
    if let Ok(panel) = crate::read_regular_to_string(source_dir.join("ui/main.html"))
        && !dir.join(&part).exists()
    {
        copies.push((part, part_of(&panel, slot, &source_project)));
    }

    // The slot loads the copy, with the seed scope it had.
    let table = host
        .get_mut("instances")
        .and_then(|i| i.get_mut(slot))
        .and_then(Item::as_value_mut)
        .and_then(Value::as_inline_table_mut)
        .ok_or_else(|| invalid(format!("no slot `{slot}`")))?;
    table.insert("load", Value::from(inside.as_str()));
    table.insert("seed", Value::from(seed));
    let edits = vec![(PROJECT_FILE.to_owned(), host.to_string())];
    let loaded_before = Project::from_sources_in_with(files.clone(), library, dir).is_ok();
    let mut prepared = Prepared {
        copies,
        edits,
        reused,
        files,
        loaded_before,
        samples: Vec::new(),
    };
    prepared.carry_samples(dir, &source_dir, slot)?;
    if let Err(diagnostics) = prepared.check(dir, library) {
        return Err(VendorError::Invalid(diagnostics));
    }
    Ok(prepared)
}

/// The local composites (`module`, a slot's module `load`) and patterns a
/// definition's text refers to.
fn local_refs(text: &str) -> (Vec<String>, Vec<String>) {
    let mut modules = Vec::new();
    let mut patterns = Vec::new();
    let Ok(table) = text.parse::<toml::Table>() else {
        return (modules, patterns);
    };
    if let Some(instances) = table.get("instances").and_then(toml::Value::as_table) {
        for entry in instances.values() {
            if let Some(module) = entry.get("module").and_then(toml::Value::as_str) {
                modules.push(module.to_owned());
            }
            if entry.get("slot").is_some()
                && let Some(load) = entry.get("load").and_then(toml::Value::as_str)
                && !crate::is_project_path(load)
            {
                modules.push(load.to_owned());
            }
            if let Some(pattern) = entry.get("pattern").and_then(toml::Value::as_str) {
                patterns.push(pattern.to_owned());
            }
        }
    }
    (modules, patterns)
}

/// Settles which incoming composites the host's identical files may stand
/// for (the audit of 2026-11-14): `composites` and `patterns` map each
/// incoming name to the name it gets (its own when the host has the same
/// text, which `reused` lists by path, or none); `texts` are the incoming
/// composites by name. A reused composite one of whose local references
/// (`module`, a slot's `load`, `pattern`) gets another name would resolve
/// the host's file of that name, not its own: it is copied too, under
/// `fresh(name)`, and so on until nothing changes. The host's files are
/// never rewritten.
pub fn settle_reuse(
    composites: &mut BTreeMap<String, String>,
    patterns: &BTreeMap<String, String>,
    texts: &BTreeMap<String, String>,
    reused: &mut Vec<String>,
    fresh: &dyn Fn(&str) -> String,
) {
    loop {
        let mut changed = false;
        for (name, text) in texts {
            let path = format!("{COMPOSITES}/{name}.toml");
            if !reused.contains(&path) {
                continue;
            }
            let (modules, pats) = local_refs(text);
            let moved = modules
                .iter()
                .any(|m| composites.get(m).is_some_and(|n| n != m))
                || pats.iter().any(|p| patterns.get(p).is_some_and(|n| n != p));
            if moved {
                reused.retain(|r| *r != path);
                composites.insert(name.clone(), fresh(name));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// ADR 0035 §11: the top-level include `key` of the project in `dir`
/// (`key = { project = "<path>" }`) turned into the host's own text, as
/// `name` (default `key`): the included `project.toml` becomes
/// `composites/<name>.toml` with `[composite] seed` its seed scope (so the
/// copy draws the streams the include drew), its sinks their taps
/// (`std.audio_tap`, its channels exported as the include's were), its
/// composites and patterns copied (one with a name the host has with
/// another text as `<name>_<x>`, references rewritten), its `set` folded
/// into the copied entries, and the entry `module = "<name>"` with its
/// exported values. The host renders bit-identically. Its panel is not
/// copied (the host's panel shows an include's; a composite's has none).
pub fn prepare_include(
    dir: &Path,
    key: &str,
    name: Option<&str>,
    library: &Library,
) -> Result<Prepared, VendorError> {
    let files = project_sources(dir)?;
    let host_text = files
        .iter()
        .find(|f| f.path == PROJECT_FILE)
        .map(|f| f.text.clone())
        .unwrap_or_default();
    let mut host = parse(PROJECT_FILE, &host_text)?;
    let invalid = |m: String| VendorError::InvalidName(m);
    let top = name.unwrap_or(key).to_owned();
    if !valid_name(&top) {
        return Err(invalid(format!(
            "invalid composite name `{top}`; names use a-z, 0-9, _ and start with a-z"
        )));
    }
    let entry = host
        .get("instances")
        .and_then(|i| i.get(key))
        .and_then(Item::as_value)
        .and_then(Value::as_inline_table)
        .ok_or_else(|| {
            invalid(format!(
                "no include `{key}` written inline at the top level"
            ))
        })?
        .clone();
    let path = entry
        .get("project")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            invalid(format!(
                "`{key}` includes no project (`project = \"<path>\"`)"
            ))
        })?
        .to_owned();
    let source_dir = crate::include_folder(dir, &path)
        .ok_or_else(|| invalid(format!("`{key}`: `{path}` does not unpack")))?;
    let source = tap_composites(project_sources(&source_dir)?)?;
    let text = |path: &str| {
        source
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.text.clone())
    };
    let source_project = parse(
        &format!("{path}/{PROJECT_FILE}"),
        &text(PROJECT_FILE).unwrap_or_default(),
    )?;
    let seed = match entry.get("seed").and_then(Value::as_integer) {
        Some(seed) => seed,
        None => source_project
            .get("project")
            .and_then(|p| p.get("seed"))
            .and_then(Item::as_integer)
            .unwrap_or(0),
    };
    let have = |path: &str| {
        files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.text.clone())
    };
    let taken = |path: &str| have(path).is_some() || dir.join(path).exists();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    let mut patterns: BTreeMap<String, String> = BTreeMap::new();
    let mut copies: Vec<(String, String)> = Vec::new();
    let mut reused: Vec<String> = Vec::new();
    for file in &source {
        for (prefix, names) in [(COMPOSITES, &mut renamed), ("patterns", &mut patterns)] {
            let Some(stem) = file
                .path
                .strip_prefix(&format!("{prefix}/"))
                .and_then(|n| n.strip_suffix(".toml"))
            else {
                continue;
            };
            let at = format!("{prefix}/{stem}.toml");
            let new = match have(&at) {
                Some(existing) if existing == file.text => {
                    reused.push(at);
                    stem.to_owned()
                }
                Some(_) => format!("{top}_{stem}"),
                None => stem.to_owned(),
            };
            names.insert(stem.to_owned(), new);
        }
    }
    let texts: BTreeMap<String, String> = source
        .iter()
        .filter_map(|f| {
            let name = f
                .path
                .strip_prefix(&format!("{COMPOSITES}/"))?
                .strip_suffix(".toml")?;
            Some((name.to_owned(), f.text.clone()))
        })
        .collect();
    settle_reuse(&mut renamed, &patterns, &texts, &mut reused, &|stem| {
        format!("{top}_{stem}")
    });
    let composite_path = format!("{COMPOSITES}/{top}.toml");
    if taken(&composite_path) || renamed.values().any(|v| *v == top) {
        return Err(VendorError::Exists(composite_path));
    }
    let retarget = |document: &mut DocumentMut| {
        rewrite(document, &renamed);
        if let Some(instances) = document
            .get_mut("instances")
            .and_then(Item::as_table_like_mut)
        {
            for (_, entry) in instances.iter_mut() {
                if let Some(pattern) = entry.as_table_like_mut().and_then(|e| e.get_mut("pattern"))
                    && let Some(new) = pattern.as_str().and_then(|p| patterns.get(p)).cloned()
                {
                    replace(pattern, &new);
                }
            }
        }
    };
    for file in &source {
        for (prefix, names) in [(COMPOSITES, &renamed), ("patterns", &patterns)] {
            let Some(stem) = file
                .path
                .strip_prefix(&format!("{prefix}/"))
                .and_then(|n| n.strip_suffix(".toml"))
            else {
                continue;
            };
            let new = names.get(stem).cloned().unwrap_or_else(|| stem.to_owned());
            let at = format!("{prefix}/{new}.toml");
            if reused.contains(&at) {
                continue;
            }
            let mut document = parse(&file.path, &file.text)?;
            if prefix == COMPOSITES {
                if let Some(item) = document
                    .get_mut("composite")
                    .and_then(Item::as_table_like_mut)
                    .and_then(|c| c.get_mut("name"))
                {
                    replace(item, &new);
                }
                retarget(&mut document);
            }
            if taken(&at) {
                return Err(VendorError::Exists(at));
            }
            copies.push((at, document.to_string()));
        }
    }

    // The root as a composite: sinks as their taps, `set` folded in.
    let mut document = source_project.clone();
    document.remove("schema_version");
    document.remove("project");
    retarget(&mut document);
    if let Some(set) = entry.get("set").and_then(Value::as_inline_table) {
        for (target, value) in set.iter() {
            let Some((instance, parameter)) = target.split_once('.') else {
                continue;
            };
            let Some(x) = value
                .as_float()
                .or_else(|| value.as_integer().map(|i| i as f64))
            else {
                continue;
            };
            if let Some(table) = document
                .get_mut("instances")
                .and_then(|i| i.get_mut(instance))
                .and_then(Item::as_value_mut)
                .and_then(Value::as_inline_table_mut)
            {
                table.insert(parameter, crate::write::number(x));
            } else {
                return Err(invalid(format!(
                    "`{key}`'s set names `{target}`, not an inline entry of its project: fold it by hand"
                )));
            }
        }
    }
    let exports = tap_root(&mut document, &format!("`{key}`'s project"))?;
    let composite_text = format!(
        "schema_version = 1\n\n# Copied from the include `{key}` (`{path}`) by `signal vendor --include` (ADR 0035 §11).\n[composite]\nname = {}\nseed = {seed}\n\n{}{exports}",
        Value::from(top.as_str()),
        document.to_string().trim_start()
    );
    copies.push((composite_path, composite_text));

    // The entry: the composite, with the values set on it.
    let table = host
        .get_mut("instances")
        .and_then(|i| i.get_mut(key))
        .and_then(Item::as_value_mut)
        .and_then(Value::as_inline_table_mut)
        .ok_or_else(|| invalid(format!("no include `{key}`")))?;
    for reserved in ["project", "seed", "set"] {
        table.remove(reserved);
    }
    let mut rebuilt = toml_edit::InlineTable::new();
    rebuilt.insert("module", Value::from(top.as_str()));
    for (k, v) in table.iter() {
        rebuilt.insert(k, v.clone());
    }
    rebuilt.fmt();
    *table = rebuilt;
    let edits = vec![(PROJECT_FILE.to_owned(), host.to_string())];
    let loaded_before = Project::from_sources_in_with(files.clone(), library, dir).is_ok();
    let mut prepared = Prepared {
        copies,
        edits,
        reused,
        files,
        loaded_before,
        samples: Vec::new(),
    };
    prepared.carry_samples(dir, &source_dir, &top)?;
    if let Err(diagnostics) = prepared.check(dir, library) {
        return Err(VendorError::Invalid(diagnostics));
    }
    Ok(prepared)
}

/// A project's panel as the part of slot `slot` holding it: the marker
/// written, each binding on a port an export of `project` reaches rebound
/// to that export (`.name`), the others kept (paths inside the content).
fn part_of(panel: &str, slot: &str, project: &DocumentMut) -> String {
    let mut exports: BTreeMap<String, String> = BTreeMap::new();
    if let Some(roles) = project.get("exports").and_then(Item::as_table_like) {
        for (_, role) in roles.iter() {
            let Some(role) = role.as_table_like() else {
                continue;
            };
            for (name, value) in role.iter() {
                let targets: Vec<String> = match value.as_value() {
                    Some(Value::String(s)) => vec![s.value().clone()],
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                    _ => Vec::new(),
                };
                for target in targets {
                    exports
                        .entry(format!("/{target}"))
                        .or_insert_with(|| format!(".{name}"));
                }
            }
        }
    }
    let marked = panel.replacen(
        "<signal-panel",
        &format!("<signal-panel part=\"{slot}\""),
        1,
    );
    let mut out = String::with_capacity(marked.len());
    let mut rest = marked.as_str();
    while let Some(at) = rest.find("bind=\"") {
        let (before, after) = rest.split_at(at + "bind=\"".len());
        out.push_str(before);
        let end = after.find('"').unwrap_or(after.len());
        let rebound: Vec<String> = after[..end]
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|a| !a.is_empty())
            .map(|a| exports.get(a).cloned().unwrap_or_else(|| a.to_owned()))
            .collect();
        out.push_str(&rebound.join(" "));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// The first of `dir`'s `relative` path and its folders (below `dir`) that
/// is a symbolic link, if any.
fn linked_on_the_way(dir: &Path, relative: &str) -> Option<PathBuf> {
    let mut at = dir.to_owned();
    for part in Path::new(relative).components() {
        at.push(part);
        if fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return Some(at);
        }
    }
    None
}

/// Writes what [`prepare`] checked, all or nothing (see the module's notes),
/// holding the project's writer lock ([`ProjectLock`], ADR 0040) from the
/// conflict check to the last check of what was written. The project's files
/// must still be exactly those [`prepare`] read: any file changed, added or
/// removed meanwhile (an edit of `project.toml`, by Signal or a person) is a
/// [`VendorError::Changed`], and nothing is written.
pub fn write(dir: &Path, prepared: Prepared, library: &Library) -> Result<Vendored, VendorError> {
    let Prepared {
        copies,
        edits,
        reused,
        files,
        loaded_before,
        samples,
    } = prepared;
    let _lock = ProjectLock::acquire(dir).map_err(|error| VendorError::Io {
        path: dir.to_owned(),
        error,
    })?;
    unchanged(dir, &files)?;
    // Only the project's own files are written (review 2026-11-01): never
    // through a link, a linked `composites/` included. `signal ui`'s
    // `vendor` checks the same first; `signal vendor` relies on this.
    let sample_paths = samples.iter().map(|(path, _)| path);
    for path in copies
        .iter()
        .chain(edits.iter())
        .map(|(p, _)| p)
        .chain(sample_paths)
    {
        if let Some(linked) = linked_on_the_way(dir, path) {
            return Err(VendorError::Io {
                path: linked,
                error: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "reached through a symbolic link: vendoring writes only the project's own files, and nothing was written",
                ),
            });
        }
    }
    let mut vendored = Vendored {
        copied: Vec::new(),
        reused,
        rewritten: Vec::new(),
    };
    let mut written = Written::new(dir);
    match written.apply(&copies, &edits, &files, &samples) {
        Ok(()) => {}
        Err(error) => {
            written.undo();
            return Err(error);
        }
    }
    // The project as it is now on disk must load if it loaded before (the
    // same check as above, on the files written).
    if loaded_before {
        let check = read_sources(dir)
            .ok()
            .map(|files| Project::from_sources_in_with(files, library, dir));
        if let Some(Err(diagnostics)) = check {
            written.undo();
            return Err(VendorError::Invalid(diagnostics));
        }
    }
    vendored.copied = copies
        .into_iter()
        .map(|(path, _)| path)
        .chain(samples.into_iter().map(|(path, _)| path))
        .collect();
    vendored.rewritten = edits.into_iter().map(|(path, _)| path).collect();
    Ok(vendored)
}

/// A sample's bytes, read for a copy.
fn read_sample(full: &Path) -> Result<Vec<u8>, VendorError> {
    crate::read_regular(full).map_err(|error| VendorError::Io {
        path: full.to_owned(),
        error,
    })
}

/// Whether `path` of the project in `dir`, or a sample already planned
/// there, holds `bytes` (`None`: the path is free).
fn holds(
    dir: &Path,
    planned: &[(String, Vec<u8>)],
    path: &str,
    bytes: &[u8],
) -> Result<Option<bool>, VendorError> {
    if let Some((_, planned)) = planned.iter().find(|(p, _)| p == path) {
        return Ok(Some(planned == bytes));
    }
    let full = dir.join(path);
    if !full.exists() {
        return Ok(None);
    }
    Ok(Some(read_sample(&full)? == bytes))
}

/// The project's files as [`read_sources`] reads them.
fn project_sources(dir: &Path) -> Result<Vec<SourceFile>, VendorError> {
    read_sources(dir).map_err(|error| match error {
        LoadError::Io { path, error } => VendorError::Io { path, error },
        LoadError::Invalid(diagnostics) => VendorError::Syntax {
            path: diagnostics
                .first()
                .map_or_else(|| PROJECT_FILE.to_owned(), |d| d.file.clone()),
            message: "the file is not UTF-8".to_owned(),
        },
    })
}

/// The conflict check: the project's files on disk are exactly `read`, the
/// files [`prepare`] judged (the same paths, the same text).
fn unchanged(dir: &Path, read: &[SourceFile]) -> Result<(), VendorError> {
    let now = project_sources(dir).map_err(|error| match error {
        VendorError::Syntax { path, .. } => VendorError::Changed(path),
        error => error,
    })?;
    let then: BTreeMap<&str, &str> = read
        .iter()
        .map(|f| (f.path.as_str(), f.text.as_str()))
        .collect();
    let now: BTreeMap<&str, &str> = now
        .iter()
        .map(|f| (f.path.as_str(), f.text.as_str()))
        .collect();
    match then
        .keys()
        .chain(now.keys())
        .find(|path| then.get(*path) != now.get(*path))
    {
        Some(path) => Err(VendorError::Changed((*path).to_owned())),
        None => Ok(()),
    }
}

/// The files [`vendor`] wrote, so that a failure can take them back: each
/// file is staged first ([`transaction::stage`]), each new file then linked
/// to its own name (a link never replaces a file), and each rewritten
/// project file renamed over its original last. On a failure the new files
/// are removed and the originals written back, each only while it still
/// holds exactly what vendoring wrote: a file edited meanwhile (by a
/// person's editor, which takes no lock) is left as it is.
struct Written<'d> {
    dir: &'d Path,
    /// A new file and the bytes written to it.
    created: Vec<(PathBuf, Vec<u8>)>,
    /// A replaced file, its original text and the text written over it.
    replaced: Vec<(PathBuf, String, String)>,
}

impl<'d> Written<'d> {
    fn new(dir: &'d Path) -> Written<'d> {
        Written {
            dir,
            created: Vec::new(),
            replaced: Vec::new(),
        }
    }

    fn apply(
        &mut self,
        copies: &[(String, String)],
        edits: &[(String, String)],
        files: &[SourceFile],
        samples: &[(String, Vec<u8>)],
    ) -> Result<(), VendorError> {
        // Each copy's folder (`composites/`, and for `vendor --slot`
        // `patterns/` and `ui/parts/`).
        let new = copies
            .iter()
            .map(|(path, text)| (path, text.as_bytes()))
            .chain(samples.iter().map(|(path, bytes)| (path, bytes.as_slice())));
        for (path, _) in new.clone() {
            if let Some(folder) = self.dir.join(path).parent() {
                fs::create_dir_all(folder).map_err(|error| VendorError::Io {
                    path: folder.to_owned(),
                    error,
                })?;
            }
        }
        let staging = |full: &Path, bytes: &[u8]| {
            transaction::stage(full, bytes).map_err(|error| VendorError::Io {
                path: full.to_owned(),
                error,
            })
        };
        // Every file staged first; a staged file not committed is removed
        // when dropped, on any return.
        let mut staged = Vec::new();
        for (path, bytes) in new {
            let full = self.dir.join(path);
            staged.push((staging(&full, bytes)?, full, path, bytes));
        }
        let mut replacing = Vec::new();
        for (path, text) in edits {
            let full = self.dir.join(path);
            replacing.push((staging(&full, text.as_bytes())?, full, path, text));
        }
        // The new files: a hard link fails if the name was taken meanwhile.
        for (file, full, path, text) in staged {
            file.create(&full).map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => VendorError::Exists(path.clone()),
                _ => VendorError::Io {
                    path: full.clone(),
                    error,
                },
            })?;
            self.created.push((full, text.to_vec()));
        }
        // The project's own files last, each replaced at once.
        for (file, full, path, text) in replacing {
            let original = files
                .iter()
                .find(|f| f.path == *path)
                .map(|f| f.text.clone())
                .unwrap_or_default();
            file.replace(&full).map_err(|error| VendorError::Io {
                path: full.clone(),
                error,
            })?;
            self.replaced.push((full, original, text.clone()));
        }
        Ok(())
    }

    /// Takes back what was written, as far as the file system lets it, and
    /// never over an edit made since it was written.
    fn undo(&mut self) {
        let ours = |full: &Path, bytes: &[u8]| {
            transaction::current(full).is_ok_and(|now| now.as_deref() == Some(bytes))
        };
        for (full, original, text) in self.replaced.drain(..) {
            if ours(&full, text.as_bytes()) {
                let _ = transaction::write_atomic(&full, original.as_bytes());
            }
        }
        for (full, text) in self.created.drain(..) {
            if ours(&full, &text) {
                let _ = fs::remove_file(full);
            }
        }
    }
}

/// A project's root made a composite's text as the project planned inside
/// another (ADR 0035 §3, ADR 0038 §7): every play side's sink (`module`,
/// or a device slot's `play`) its tap, and without `[exports] outputs` the
/// one sink's channels exported from the tap; what to append to the text
/// for those exports (empty when they went into an `[exports]` table). A
/// discrete slot becomes `std.device_out_tap<N>`, its ports' wires moved to
/// `c1 … cN` and its ports exported by name, as an include plans it. A
/// capture side becomes its pass-through (`std.audio_in_pass`,
/// `std.audio_in_mono_pass`, `std.device_in_pass<N>`), each channel an
/// exported input `<slot>_<port>`, as an include plans it. Refused: a `dmx`
/// slot (a data sink has no tap), several sinks and no outputs named, a
/// capture input whose name is exported already. `what` names the project.
fn tap_root(document: &mut DocumentMut, what: &str) -> Result<String, VendorError> {
    let invalid = |m: String| VendorError::InvalidName(m);
    // Each sink's exports: the exported name and the tap's output.
    let mut sinks: Vec<(String, Vec<(String, String)>)> = Vec::new();
    // A discrete slot's ports, `<slot>.<port>`, as the tap's `<slot>.c<k>`.
    let mut ports: BTreeMap<String, String> = BTreeMap::new();
    // The capture sides' inputs: the exported name and the pass's input.
    let mut capture_inputs: Vec<(String, String)> = Vec::new();
    let channel_exports = |channels: &[&str]| {
        channels
            .iter()
            .map(|c| ((*c).to_owned(), format!("{c}_out")))
            .collect::<Vec<_>>()
    };
    if let Some(instances) = document
        .get_mut("instances")
        .and_then(Item::as_table_like_mut)
    {
        for (instance, item) in instances.iter_mut() {
            let name = instance.get().to_owned();
            let Some(entry) = item.as_table_like_mut() else {
                continue;
            };
            if let Some(module) = entry.get_mut("module") {
                let Some(sink) = module.as_str().map(str::to_owned) else {
                    continue;
                };
                if let Some(tap) = crate::device::tap_of_sink(&sink) {
                    replace(module, tap);
                    let layout = crate::device::layout_of_sink(&sink).unwrap_or("stereo");
                    let channels = crate::device::layout_channels(layout).unwrap_or(&[]);
                    sinks.push((name, channel_exports(channels)));
                } else if let Some(pass) = crate::device::pass_of_source(&sink) {
                    // ADR 0038 §7: a source written as a module, its pass.
                    replace(module, &pass);
                    for port in source_outputs(&sink) {
                        capture_inputs
                            .push((format!("{name}_{port}"), format!("{name}.{port}_in")));
                    }
                }
                continue;
            }
            let Some(device) = entry.get("device") else {
                continue;
            };
            let names_of = |item: &Item| -> Vec<String> {
                item.as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let tap = if let Some(capture) = device.get("capture") {
                // ADR 0038 §7: its pass-through, its channels inputs.
                let (pass, channels) = if let Some(layout) = capture.as_str() {
                    let source = crate::device::planned_source(layout)
                        .map_err(|e| invalid(format!("{what}: `{name}`: {e}")))?;
                    let channels: Vec<(String, String)> = crate::device::layout_channels(layout)
                        .unwrap_or(&[])
                        .iter()
                        .map(|c| ((*c).to_owned(), (*c).to_owned()))
                        .collect();
                    (crate::device::pass_of_source(source), channels)
                } else {
                    let names = names_of(capture);
                    if let Some(problem) = crate::device::discrete_problem(&names) {
                        return Err(invalid(format!("{what}: `{name}`: {problem}")));
                    }
                    let mut channels = Vec::new();
                    for (k, port) in names.iter().enumerate() {
                        ports.insert(format!("{name}.{port}"), format!("{name}.c{}", k + 1));
                        channels.push((port.clone(), format!("c{}", k + 1)));
                    }
                    let pass = format!("std.device_in{}", names.len());
                    (crate::device::pass_of_source(&pass), channels)
                };
                let pass = pass.ok_or_else(|| {
                    invalid(format!(
                        "{what}: `{name}` captures what no pass-through takes"
                    ))
                })?;
                for (port, input) in channels {
                    capture_inputs.push((format!("{name}_{port}"), format!("{name}.{input}_in")));
                }
                pass
            } else if let Some(play) = device.get("play")
                && let Some(layout) = play.as_str()
            {
                let sink = crate::device::planned_module(layout)
                    .map_err(|e| invalid(format!("{what}: `{name}`: {e}")))?;
                let channels = crate::device::layout_channels(layout).unwrap_or(&[]);
                sinks.push((name, channel_exports(channels)));
                crate::device::tap_of_sink(sink)
                    .unwrap_or("std.audio_tap")
                    .to_owned()
            } else if let Some(play) = device.get("play") {
                // ADR 0038 §2, §7: a discrete slot, its ports by name.
                if device.get("kind").and_then(Item::as_str) == Some(crate::device::DMX) {
                    return Err(invalid(format!(
                        "{what}: `{name}` sends data to the machine (a `dmx` slot), which a project inside another cannot (ADR 0057 §1)"
                    )));
                }
                let names = names_of(play);
                if let Some(problem) = crate::device::discrete_problem(&names) {
                    return Err(invalid(format!("{what}: `{name}`: {problem}")));
                }
                let mut exports = Vec::new();
                for (k, port) in names.iter().enumerate() {
                    ports.insert(format!("{name}.{port}"), format!("{name}.c{}", k + 1));
                    exports.push((port.clone(), format!("c{}_out", k + 1)));
                }
                let width = names.len();
                sinks.push((name, exports));
                format!("std.device_out_tap{width}")
            } else {
                continue;
            };
            let tap = tap.as_str();
            entry.remove("device");
            // A capture slot's offline `render` file feeds a source only.
            entry.remove("render");
            if let Some(table) = item.as_inline_table_mut() {
                let mut rebuilt = toml_edit::InlineTable::new();
                rebuilt.insert("module", Value::from(tap));
                for (k, v) in table.iter() {
                    rebuilt.insert(k, v.clone());
                }
                rebuilt.fmt();
                *table = rebuilt;
            } else if let Some(entry) = item.as_table_like_mut() {
                entry.insert("module", toml_edit::value(tap));
            }
        }
    }
    retarget_ports(document, &ports);
    let exported = |key: &str, name: &str| {
        document
            .get("exports")
            .and_then(|e| e.get(key))
            .and_then(Item::as_table_like)
            .is_some_and(|t| t.contains_key(name))
    };
    if let Some((name, _)) = capture_inputs.iter().find(|(n, _)| {
        ["inputs", "outputs", "parameters"]
            .iter()
            .any(|k| exported(k, n))
    }) {
        return Err(invalid(format!(
            "{what}: `{name}` is exported already, and a capture side becomes that input inside another project (ADR 0038 §7); rename the export"
        )));
    }
    let has_outputs = document
        .get("exports")
        .and_then(|e| e.get("outputs"))
        .is_some();
    let outputs = if has_outputs || sinks.is_empty() {
        None
    } else {
        let [(sink, channels)] = sinks.as_slice() else {
            return Err(invalid(format!(
                "{what} has several sinks: name its outputs in its [exports] first"
            )));
        };
        let mut outputs = toml_edit::InlineTable::new();
        for (export, port) in channels {
            outputs.insert(export, Value::from(format!("{sink}.{port}")));
        }
        Some(outputs)
    };
    let mut appended = String::new();
    if let Some(table) = document
        .get_mut("exports")
        .and_then(Item::as_table_like_mut)
    {
        if let Some(outputs) = outputs {
            table.insert("outputs", Item::Value(Value::InlineTable(outputs)));
        }
        if !capture_inputs.is_empty() {
            if table.get("inputs").is_none() {
                table.insert(
                    "inputs",
                    Item::Value(Value::InlineTable(toml_edit::InlineTable::new())),
                );
            }
            if let Some(inputs) = table.get_mut("inputs").and_then(Item::as_table_like_mut) {
                for (name, to) in &capture_inputs {
                    inputs.insert(name, toml_edit::value(to.as_str()));
                }
            }
            if let Some(inputs) = table
                .get_mut("inputs")
                .and_then(Item::as_value_mut)
                .and_then(Value::as_inline_table_mut)
            {
                inputs.fmt();
            }
        }
        return Ok(appended);
    }
    if outputs.is_some() || !capture_inputs.is_empty() {
        appended.push_str("\n[exports]\n");
    }
    if !capture_inputs.is_empty() {
        let mut inputs = toml_edit::InlineTable::new();
        for (name, to) in &capture_inputs {
            inputs.insert(name, Value::from(to.as_str()));
        }
        inputs.fmt();
        appended.push_str(&format!("inputs = {inputs}\n"));
    }
    if let Some(mut outputs) = outputs {
        outputs.fmt();
        appended.push_str(&format!("outputs = {outputs}\n"));
    }
    Ok(appended)
}

/// Moves every wire end, exported output and exported input that `ports`
/// maps (`<slot>.<port>` of a discrete slot) to the tap's or pass's port.
fn retarget_ports(document: &mut DocumentMut, ports: &BTreeMap<String, String>) {
    if ports.is_empty() {
        return;
    }
    let mut move_end = |value: &mut Value| {
        if let Some(new) = value.as_str().and_then(|v| ports.get(v)) {
            let decor = value.decor().clone();
            *value = Value::from(new.as_str());
            *value.decor_mut() = decor;
        }
    };
    if let Some(connections) = document
        .get_mut("wiring")
        .and_then(|w| w.get_mut("connections"))
        .and_then(Item::as_array_mut)
    {
        for connection in connections.iter_mut() {
            if let Some(table) = connection.as_inline_table_mut() {
                for key in ["from", "to"] {
                    if let Some(end) = table.get_mut(key) {
                        move_end(end);
                    }
                }
            }
        }
    }
    if let Some(outputs) = document
        .get_mut("exports")
        .and_then(|e| e.get_mut("outputs"))
        .and_then(Item::as_table_like_mut)
    {
        for (_, item) in outputs.iter_mut() {
            if let Some(value) = item.as_value_mut() {
                move_end(value);
            }
        }
    }
    if let Some(inputs) = document
        .get_mut("exports")
        .and_then(|e| e.get_mut("inputs"))
        .and_then(Item::as_table_like_mut)
    {
        for (_, item) in inputs.iter_mut() {
            match item.as_value_mut() {
                Some(Value::Array(targets)) => targets.iter_mut().for_each(&mut move_end),
                Some(value) => move_end(value),
                None => {}
            }
        }
    }
}

/// What a module of a copied project plans as inside another (ADR 0035 §3,
/// ADR 0038 §7): a sink's tap or a source's pass-through; `None` for any
/// other module.
fn planned_inside(module: &str) -> Option<String> {
    if let Some(tap) = crate::device::tap_of_sink(module) {
        return Some(tap.to_owned());
    }
    if let Some(width) = module
        .strip_prefix("std.device_out")
        .and_then(|w| w.parse::<usize>().ok())
    {
        return Some(format!("std.device_out_tap{width}"));
    }
    crate::device::pass_of_source(module)
}

/// The signal outputs of capture source `module`, in order.
fn source_outputs(module: &str) -> Vec<String> {
    match module {
        crate::device::STEREO_SOURCE => vec!["l".to_owned(), "r".to_owned()],
        crate::device::MONO_SOURCE => vec!["m".to_owned()],
        other => {
            let width = other
                .strip_prefix("std.device_in")
                .or_else(|| other.strip_prefix("std.control_in"))
                .and_then(|w| w.parse::<usize>().ok())
                .unwrap_or(0);
            (1..=width).map(|k| format!("c{k}")).collect()
        }
    }
}

/// A copied project's own composites as an include plans them (ADR 0035
/// §3, ADR 0038 §7): each sink its tap, each capture source its
/// pass-through, before their texts are compared with the host's (an
/// identical file is reused). A text with neither is kept byte for byte; a
/// data sink (`std.data_out<N>`, no tap) is refused.
fn tap_composites(files: Vec<SourceFile>) -> Result<Vec<SourceFile>, VendorError> {
    files
        .into_iter()
        .map(|mut file| {
            if !file.path.starts_with(&format!("{COMPOSITES}/")) {
                return Ok(file);
            }
            let mut document = parse(&file.path, &file.text)?;
            let mut changed = false;
            if let Some(instances) = document
                .get_mut("instances")
                .and_then(Item::as_table_like_mut)
            {
                for (name, entry) in instances.iter_mut() {
                    let Some(module) = entry.as_table_like_mut().and_then(|e| e.get_mut("module"))
                    else {
                        continue;
                    };
                    let Some(old) = module.as_str().map(str::to_owned) else {
                        continue;
                    };
                    if old.starts_with("std.data_out") {
                        return Err(VendorError::InvalidName(format!(
                            "{}: `{}` sends data to the machine (`{old}`), which a project inside another cannot (ADR 0057 §1)",
                            file.path,
                            name.get()
                        )));
                    }
                    if let Some(new) = planned_inside(&old) {
                        replace(module, &new);
                        changed = true;
                    }
                }
            }
            if changed {
                file.text = document.to_string();
            }
            Ok(file)
        })
        .collect()
}

fn parse(path: &str, text: &str) -> Result<DocumentMut, VendorError> {
    text.parse()
        .map_err(|e: toml_edit::TomlError| VendorError::Syntax {
            path: path.to_owned(),
            message: e.message().trim().to_owned(),
        })
}

/// Rewrites every instance's `module`, and a slot's local `load`, that
/// `renamed` maps; whether any was. A nested slot's `load` names a
/// composite as `module` does: left alone, it would resolve the host's file
/// of that name (the audit of 2026-11-15).
fn rewrite(document: &mut DocumentMut, renamed: &BTreeMap<String, String>) -> bool {
    let Some(instances) = document
        .get_mut("instances")
        .and_then(Item::as_table_like_mut)
    else {
        return false;
    };
    let mut changed = false;
    for (_, entry) in instances.iter_mut() {
        let Some(entry) = entry.as_table_like_mut() else {
            continue;
        };
        for key in ["module", "load"] {
            let Some(item) = entry.get_mut(key) else {
                continue;
            };
            if let Some(new) = item.as_str().and_then(|m| renamed.get(m)) {
                let new = new.clone();
                replace(item, &new);
                changed = true;
            }
        }
    }
    changed
}

/// Replaces a string value's text, keeping the spacing around it.
fn replace(item: &mut Item, text: &str) {
    if let Some(value) = item.as_value_mut() {
        let decor = value.decor().clone();
        *value = Value::from(text);
        *value.decor_mut() = decor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// audit 2026-10-23: taking back a vendoring never overwrites an edit
    /// made after it wrote (a person's editor takes no lock): a rewritten
    /// file edited since is left as edited, a new file edited since is kept;
    /// the files still as vendoring wrote them are taken back.
    #[test]
    fn undo_keeps_an_edit_made_meanwhile() {
        let dir = std::env::temp_dir().join(format!("signal-vendor-undo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(COMPOSITES)).unwrap();
        let project = dir.join(PROJECT_FILE);
        let copy = dir.join("composites/a.toml");
        let other = dir.join("composites/b.toml");
        fs::write(&project, "rewritten").unwrap();
        fs::write(&copy, "copy").unwrap();
        fs::write(&other, "copy b").unwrap();
        let mut written = Written::new(&dir);
        written
            .replaced
            .push((project.clone(), "original".into(), "rewritten".into()));
        written.created.push((copy.clone(), "copy".into()));
        written.created.push((other.clone(), "copy b".into()));
        // Edited after vendoring wrote: gain 0.25 -> 0.5, and one copy.
        fs::write(&project, "edited: gain = 0.5").unwrap();
        fs::write(&other, "copy b, edited").unwrap();
        written.undo();
        assert_eq!(fs::read_to_string(&project).unwrap(), "edited: gain = 0.5");
        assert!(!copy.exists(), "the untouched copy is taken back");
        assert_eq!(fs::read_to_string(&other).unwrap(), "copy b, edited");

        // Not edited: the original is written back.
        let mut written = Written::new(&dir);
        fs::write(&project, "rewritten").unwrap();
        written
            .replaced
            .push((project.clone(), "original".into(), "rewritten".into()));
        written.undo();
        assert_eq!(fs::read_to_string(&project).unwrap(), "original");
        fs::remove_dir_all(&dir).unwrap();
    }
}
