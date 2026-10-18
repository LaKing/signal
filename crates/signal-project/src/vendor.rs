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
//! written back. Instance names do not change, so the project's addresses,
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
            VendorError::Io { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for VendorError {}

/// Copies the library composite `reference` (`lib.<name>`) and the library
/// composites it uses into the project folder `dir`, the first as `name`
/// (default: its name without `lib.`), and rewrites the project's references
/// to them.
pub fn vendor(
    dir: &Path,
    reference: &str,
    name: Option<&str>,
    library: &Library,
) -> Result<Vendored, VendorError> {
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
            let same = fs::read(&full).is_ok_and(|bytes| bytes == text.as_bytes());
            if old != reference && same {
                reused.push(path);
                continue;
            }
            return Err(VendorError::Exists(path));
        }
        copies.push((path, text));
    }

    // The project's own references, rewritten.
    let files = read_sources(dir).map_err(|error| match error {
        LoadError::Io { path, error } => VendorError::Io { path, error },
        LoadError::Invalid(diagnostics) => VendorError::Syntax {
            path: diagnostics
                .first()
                .map_or_else(|| PROJECT_FILE.to_owned(), |d| d.file.clone()),
            message: "the file is not UTF-8".to_owned(),
        },
    })?;
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
    let loaded_before = Project::from_sources_with(files.clone(), library).is_ok();
    if loaded_before {
        let mut after: Vec<SourceFile> = files
            .iter()
            .map(|file| {
                let edited = edits.iter().find(|(path, _)| *path == file.path);
                SourceFile {
                    path: file.path.clone(),
                    text: edited.map_or_else(|| file.text.clone(), |(_, text)| text.clone()),
                }
            })
            .collect();
        after.extend(copies.iter().map(|(path, text)| SourceFile {
            path: path.clone(),
            text: text.clone(),
        }));
        if let Err(diagnostics) = Project::from_sources_with(after, library) {
            return Err(VendorError::Invalid(diagnostics));
        }
    }

    let mut vendored = Vendored {
        copied: Vec::new(),
        reused,
        rewritten: Vec::new(),
    };
    let mut written = Written::new(dir);
    match written.apply(&copies, &edits, &files) {
        Ok(()) => {}
        Err(error) => {
            written.undo();
            return Err(error);
        }
    }
    // The project as it is now on disk must load if it loaded before (the
    // same check as above, on the files written: nothing changed meanwhile).
    if loaded_before {
        let check = read_sources(dir)
            .ok()
            .map(|files| Project::from_sources_with(files, library));
        if let Some(Err(diagnostics)) = check {
            written.undo();
            return Err(VendorError::Invalid(diagnostics));
        }
    }
    vendored.copied = copies.into_iter().map(|(path, _)| path).collect();
    vendored.rewritten = edits.into_iter().map(|(path, _)| path).collect();
    Ok(vendored)
}

/// The files [`vendor`] wrote, so that a failure can take them back: each
/// new file is written under a temporary name and linked to its own name
/// only once every one is written (a link never replaces a file), then
/// each rewritten project file replaces its original by a rename. On a
/// failure the new files are removed and the originals written back.
struct Written<'d> {
    dir: &'d Path,
    temporary: Vec<PathBuf>,
    created: Vec<PathBuf>,
    replaced: Vec<(PathBuf, String)>,
}

impl<'d> Written<'d> {
    fn new(dir: &'d Path) -> Written<'d> {
        Written {
            dir,
            temporary: Vec::new(),
            created: Vec::new(),
            replaced: Vec::new(),
        }
    }

    /// A temporary file beside `path` (`.<name>.vendor`) holding `text`,
    /// created new.
    fn temporary(&mut self, path: &Path, text: &str) -> Result<PathBuf, VendorError> {
        let name = path
            .file_name()
            .map_or_else(Default::default, |n| n.to_string_lossy());
        let temporary = path.with_file_name(format!(".{name}.vendor"));
        let io = |error| VendorError::Io {
            path: temporary.clone(),
            error,
        };
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&temporary).map_err(io)?;
        self.temporary.push(temporary.clone());
        io::Write::write_all(&mut file, text.as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
        Ok(temporary)
    }

    fn apply(
        &mut self,
        copies: &[(String, String)],
        edits: &[(String, String)],
        files: &[SourceFile],
    ) -> Result<(), VendorError> {
        if !copies.is_empty() {
            let folder = self.dir.join(COMPOSITES);
            fs::create_dir_all(&folder).map_err(|error| VendorError::Io {
                path: folder,
                error,
            })?;
        }
        let mut staged = Vec::new();
        for (path, text) in copies {
            let full = self.dir.join(path);
            staged.push((self.temporary(&full, text)?, full, path));
        }
        let mut replacing = Vec::new();
        for (path, text) in edits {
            let full = self.dir.join(path);
            replacing.push((self.temporary(&full, text)?, full, path));
        }
        // The new files: a hard link fails if the name was taken meanwhile.
        for (temporary, full, path) in staged {
            fs::hard_link(&temporary, &full).map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => VendorError::Exists(path.clone()),
                _ => VendorError::Io {
                    path: full.clone(),
                    error,
                },
            })?;
            self.created.push(full);
        }
        // The project's own files last, each replaced at once.
        for (temporary, full, path) in replacing {
            let original = files
                .iter()
                .find(|f| f.path == *path)
                .map(|f| f.text.clone())
                .unwrap_or_default();
            fs::rename(&temporary, &full).map_err(|error| VendorError::Io {
                path: full.clone(),
                error,
            })?;
            self.temporary.retain(|t| *t != temporary);
            self.replaced.push((full, original));
        }
        self.clean();
        Ok(())
    }

    /// Removes the temporary files left.
    fn clean(&mut self) {
        for temporary in self.temporary.drain(..) {
            let _ = fs::remove_file(temporary);
        }
    }

    /// Takes back what was written, as far as the file system lets it.
    fn undo(&mut self) {
        self.clean();
        for (full, original) in self.replaced.drain(..) {
            let _ = fs::write(full, original);
        }
        for full in self.created.drain(..) {
            let _ = fs::remove_file(full);
        }
    }
}

fn parse(path: &str, text: &str) -> Result<DocumentMut, VendorError> {
    text.parse()
        .map_err(|e: toml_edit::TomlError| VendorError::Syntax {
            path: path.to_owned(),
            message: e.message().trim().to_owned(),
        })
}

/// Rewrites every instance's `module` that `renamed` maps; whether any was.
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
        let Some(module) = entry.get_mut("module") else {
            continue;
        };
        if let Some(new) = module.as_str().and_then(|m| renamed.get(m)) {
            let new = new.clone();
            replace(module, &new);
            changed = true;
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
