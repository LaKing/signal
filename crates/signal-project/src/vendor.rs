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
}

impl Prepared {
    /// Every file it would write (new and rewritten), relative to the project.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.copies
            .iter()
            .chain(&self.edits)
            .map(|(path, _)| path.as_str())
    }

    /// The project's files as they were read when it was prepared: what
    /// [`write`] checks they still are.
    pub fn before(&self) -> &[SourceFile] {
        &self.files
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
    };
    if loaded_before
        && let Err(diagnostics) = Project::from_sources_in_with(prepared.after(), library, dir)
    {
        return Err(VendorError::Invalid(diagnostics));
    }
    Ok(prepared)
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
    } = prepared;
    let _lock = ProjectLock::acquire(dir).map_err(|error| VendorError::Io {
        path: dir.to_owned(),
        error,
    })?;
    unchanged(dir, &files)?;
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
    vendored.copied = copies.into_iter().map(|(path, _)| path).collect();
    vendored.rewritten = edits.into_iter().map(|(path, _)| path).collect();
    Ok(vendored)
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
    /// A new file and the text written to it.
    created: Vec<(PathBuf, String)>,
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
    ) -> Result<(), VendorError> {
        if !copies.is_empty() {
            let folder = self.dir.join(COMPOSITES);
            fs::create_dir_all(&folder).map_err(|error| VendorError::Io {
                path: folder,
                error,
            })?;
        }
        let staging = |full: &Path, text: &str| {
            transaction::stage(full, text.as_bytes()).map_err(|error| VendorError::Io {
                path: full.to_owned(),
                error,
            })
        };
        // Every file staged first; a staged file not committed is removed
        // when dropped, on any return.
        let mut staged = Vec::new();
        for (path, text) in copies {
            let full = self.dir.join(path);
            staged.push((staging(&full, text)?, full, path, text));
        }
        let mut replacing = Vec::new();
        for (path, text) in edits {
            let full = self.dir.join(path);
            replacing.push((staging(&full, text)?, full, path, text));
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
            self.created.push((full, text.clone()));
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
        let ours = |full: &Path, text: &str| {
            transaction::current(full).is_ok_and(|now| now.as_deref() == Some(text.as_bytes()))
        };
        for (full, original, text) in self.replaced.drain(..) {
            if ours(&full, &text) {
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
