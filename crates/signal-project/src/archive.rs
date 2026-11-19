//! Project files (`.signal`, ADR 0025): a project folder as one zip, a
//! container for the same text files and panel assets, never a format of its
//! own (`docs/project-format.md` §10).
//!
//! [`pack`] takes what a project reads (`project.toml`, `composites/*.toml`,
//! `patterns/*.toml`, `interfaces/*.toml`, the files under `ui/`, `snapshots.toml`, and the capture slots' `render` WAV files, ADR 0038 §3) and nothing else, in sorted path order, deflated,
//! with a fixed timestamp and fixed permissions, so the same folder always
//! gives the same bytes. [`unpack`] checks every entry before it writes one:
//! relative paths only, regular files only, at most [`Limits::DEFAULT`]
//! entries and unpacked bytes, and `project.toml` at the root. A refused
//! archive leaves nothing behind, and the [`ArchiveError`] names the reason.
//!
//! The archive's comment records the built-in library's version (ADR 0027
//! §4) of the build that packed it, `signal-library <MAJOR.MINOR>`
//! ([`library_version`]): provenance only, never a refusal, and no file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipArchive, ZipWriter};

use crate::PROJECT_FILE;

/// A project file's extension.
pub const EXTENSION: &str = "signal";

/// What the archive comment says before the library version.
const LIBRARY_COMMENT: &str = "signal-library ";

/// What the archive comment's second line says before the project id.
const PROJECT_COMMENT: &str = "project ";

/// The file type bits of a Unix mode.
const TYPE_BITS: u32 = 0o170_000;
const REGULAR: u32 = 0o100_000;
const DIRECTORY: u32 = 0o040_000;
const SYMLINK: u32 = 0o120_000;

/// An archive file larger than the unpacked bound by more than this is
/// refused before it is read.
const ARCHIVE_SLACK: u64 = 64 << 20;

/// The bounds reading enforces (`docs/project-format.md` §10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Entries in the archive, directories included.
    pub entries: usize,
    /// Bytes once unpacked, every file together.
    pub bytes: u64,
}

impl Limits {
    /// 10 000 entries and 1 GiB.
    pub const DEFAULT: Limits = Limits {
        entries: 10_000,
        bytes: 1 << 30,
    };
}

/// Why an archive was not written or not read.
#[derive(Debug)]
pub enum ArchiveError {
    /// Reading or writing a file.
    Io {
        /// The file.
        path: PathBuf,
        /// What failed.
        error: io::Error,
    },
    /// The folder has no `project.toml`, or the archive none at its root.
    NoProjectFile,
    /// The project includes projects from outside its folder (ADR 0035
    /// §13), which [`pack`] cannot hold ([`pack_embedded`] embeds them), or
    /// an embedded one's own outside includes: their paths as written.
    Includes(Vec<String>),
    /// The bytes are not a zip archive (or one this reader cannot read).
    NotAnArchive(String),
    /// More entries than [`Limits::entries`].
    TooManyEntries {
        /// The archive's.
        count: usize,
        /// The bound.
        max: usize,
    },
    /// More unpacked bytes than [`Limits::bytes`].
    TooLarge {
        /// The bound.
        max: u64,
    },
    /// An entry's path is absolute, has `..`, `.` or an empty component, a
    /// backslash or a control character.
    UnsafePath(String),
    /// An entry, or a file in the folder, is a symbolic link.
    Symlink(String),
    /// An entry is a device file, a FIFO or another special file.
    SpecialFile(String),
    /// An entry is encrypted.
    Encrypted(String),
    /// Two entries name the same file (APFS does not tell case apart).
    Duplicate(String),
    /// A file name in the folder is not UTF-8.
    NotUtf8(PathBuf),
    /// The folder to unpack into exists and is not empty.
    NotEmpty(PathBuf),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchiveError::Io { path, error } => write!(f, "{}: {error}", path.display()),
            ArchiveError::NoProjectFile => write!(f, "no {PROJECT_FILE} at the project's root"),
            ArchiveError::Includes(paths) => write!(
                f,
                "the project includes projects from outside its folder ({}): pack it with --embed (ADR 0035 §13), or keep it as a folder",
                paths.join(", ")
            ),
            ArchiveError::NotAnArchive(reason) => write!(f, "not a .signal archive: {reason}"),
            ArchiveError::TooManyEntries { count, max } => {
                write!(f, "{count} entries, more than the {max} allowed")
            }
            ArchiveError::TooLarge { max } => {
                write!(f, "more than {max} bytes once unpacked")
            }
            ArchiveError::UnsafePath(name) => {
                write!(f, "'{name}' is not a relative path inside the project")
            }
            ArchiveError::Symlink(name) => write!(f, "'{name}' is a symbolic link"),
            ArchiveError::SpecialFile(name) => {
                write!(f, "'{name}' is a device or other special file")
            }
            ArchiveError::Encrypted(name) => write!(f, "'{name}' is encrypted"),
            ArchiveError::Duplicate(name) => write!(f, "'{name}' is in the archive twice"),
            ArchiveError::NotUtf8(path) => write!(f, "{}: the name is not UTF-8", path.display()),
            ArchiveError::NotEmpty(path) => {
                write!(f, "{}: exists and is not empty", path.display())
            }
        }
    }
}

impl std::error::Error for ArchiveError {}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> ArchiveError + '_ {
    move |error| ArchiveError::Io {
        path: path.to_owned(),
        error,
    }
}

fn zip_error(error: zip::result::ZipError) -> ArchiveError {
    ArchiveError::NotAnArchive(error.to_string())
}

/// Whether `path` names a project file (`*.signal`).
pub fn is_project_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == EXTENSION)
}

/// Whether a project reads the file at `relative` (`/`-separated): its
/// `project.toml`, a composite in `composites/`, a pattern in `patterns/`,
/// an interface template in `interfaces/` (ADR 0037 §3), a file under
/// `ui/`, or its `snapshots.toml` (ADR 0041).
fn read_by_project(relative: &str) -> bool {
    let parts: Vec<&str> = relative.split('/').collect();
    match parts.as_slice() {
        [PROJECT_FILE] | [crate::snapshot::SNAPSHOTS_FILE] => true,
        ["composites" | "patterns" | "interfaces", name] => name.ends_with(".toml"),
        ["ui", _, ..] => true,
        _ => false,
    }
}

/// The folder's files a project reads, by their `/`-separated path.
fn project_files(dir: &Path) -> Result<BTreeMap<String, PathBuf>, ArchiveError> {
    let mut files = BTreeMap::new();
    let mut pending = vec![(dir.to_owned(), String::new())];
    while let Some((folder, prefix)) = pending.pop() {
        for entry in fs::read_dir(&folder).map_err(io_error(&folder))? {
            let entry = entry.map_err(io_error(&folder))?;
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(ArchiveError::NotUtf8(path));
            };
            // Hidden files and folders (.DS_Store, .git) are never read.
            if name.starts_with('.') {
                continue;
            }
            let relative = format!("{prefix}{name}");
            let kind = entry.file_type().map_err(io_error(&path))?;
            if kind.is_symlink()
                && prefix.is_empty()
                && (name == "composites"
                    || name == "patterns"
                    || name == "interfaces"
                    || name == "ui")
            {
                return Err(ArchiveError::Symlink(relative));
            }
            if kind.is_dir() {
                // Only composites/, patterns/, interfaces/ and ui/ hold files
                // a project reads.
                let top = relative.split('/').next().unwrap_or_default();
                let flat = matches!(top, "composites" | "patterns" | "interfaces");
                if flat && prefix.is_empty() || top == "ui" {
                    pending.push((path, format!("{relative}/")));
                }
            } else if read_by_project(&relative) {
                if kind.is_symlink() {
                    return Err(ArchiveError::Symlink(relative));
                }
                if kind.is_file() {
                    files.insert(relative, path);
                }
            }
        }
    }
    if !files.contains_key(PROJECT_FILE) {
        return Err(ArchiveError::NoProjectFile);
    }
    // ADR 0038 §3: a capture slot's render file is a project file wherever
    // in the folder it lies; one missing is left out (the render says so).
    let mut extra: Vec<String> = fs::read_to_string(dir.join(PROJECT_FILE))
        .map(|text| crate::render_paths(&text))
        .unwrap_or_default();
    // ADR 0056 §1: a sample pattern's WAV is a project file too.
    for (relative, path) in &files {
        if relative.starts_with("patterns/")
            && relative.ends_with(".toml")
            && let Ok(text) = crate::read_regular_to_string(path)
            && let Some(file) = crate::sample_file(&text)
            && !extra.contains(&file)
        {
            extra.push(file);
        }
    }
    {
        for relative in extra {
            // Every component, folders included, as the walk above judges
            // them: a link anywhere on the way is refused, so no file
            // outside the folder is read.
            let mut path = dir.to_owned();
            let mut regular = true;
            for part in relative.split('/') {
                path.push(part);
                match fs::symlink_metadata(&path) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(ArchiveError::Symlink(relative));
                    }
                    Ok(_) => {}
                    Err(_) => {
                        regular = false;
                        break;
                    }
                }
            }
            if regular && fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                files.insert(relative, path);
            }
        }
    }
    Ok(files)
}

/// ADR 0035 §13: the files of the projects `dir`'s project includes (or
/// loads into slots) from inside its own folder, and theirs in turn, by
/// their names in the archive (their paths relative to `root`).
fn inside_includes(dir: &Path, root: &Path) -> Result<BTreeMap<String, PathBuf>, ArchiveError> {
    let mut found = BTreeMap::new();
    let mut todo = vec![dir.to_owned()];
    let mut seen: Vec<PathBuf> = Vec::new();
    while let Some(folder) = todo.pop() {
        let Ok(sources) = crate::read_sources(&folder) else {
            continue;
        };
        for written in crate::include::included_paths(&sources) {
            let Ok(canonical) = fs::canonicalize(folder.join(&written)) else {
                continue;
            };
            if !canonical.starts_with(root) || canonical == root || seen.contains(&canonical) {
                continue;
            }
            seen.push(canonical.clone());
            let Ok(relative) = canonical.strip_prefix(root) else {
                continue;
            };
            let prefix = relative.to_string_lossy().replace('\\', "/");
            if canonical.is_file() {
                // A `.signal` include inside the folder: the file itself.
                found.insert(prefix, canonical);
                continue;
            }
            for (name, path) in project_files(&canonical)? {
                found.insert(format!("{prefix}/{name}"), path);
            }
            todo.push(canonical);
        }
    }
    Ok(found)
}

/// `dir` and the folders of the projects it includes (or loads into slots)
/// from inside `root`, and theirs in turn: each folder whose own includes a
/// pack must hold.
fn inside_folders(dir: &Path, root: &Path) -> Vec<PathBuf> {
    let mut folders = vec![dir.to_owned()];
    let mut todo = vec![dir.to_owned()];
    while let Some(folder) = todo.pop() {
        let Ok(sources) = crate::read_sources(&folder) else {
            continue;
        };
        for written in crate::include::included_paths(&sources) {
            let Ok(canonical) = fs::canonicalize(folder.join(&written)) else {
                continue;
            };
            if canonical.is_dir()
                && canonical.starts_with(root)
                && !folders.contains(&canonical)
                && canonical != *root
            {
                folders.push(canonical.clone());
                todo.push(canonical);
            }
        }
    }
    folders
}

/// The includes of `dir`'s project and of its inside includes (the audit of
/// 2026-11-14: a child's own includes too) that lie outside `root`: their
/// paths as written, the folder naming one other than `dir` shown after it.
fn outside_includes(dir: &Path, root: &Path) -> Vec<String> {
    let mut outside = Vec::new();
    for folder in inside_folders(dir, root) {
        let Ok(sources) = crate::read_sources(&folder) else {
            continue;
        };
        for written in crate::include::included_paths(&sources) {
            // An absolute path reads the original even from an unpacked copy
            // (the review of 4d37bf8): --embed writes it relative.
            let absolute = Path::new(&written).is_absolute();
            let inside = fs::canonicalize(folder.join(&written)).is_ok_and(|f| f.starts_with(root));
            if absolute && inside {
                outside.push(format!("{written} (an absolute path)"));
            } else if !inside {
                let canonical_dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
                outside.push(match folder.strip_prefix(&canonical_dir) {
                    Ok(rel) if !rel.as_os_str().is_empty() => {
                        format!("{written} (in {})", rel.display())
                    }
                    _ => written,
                });
            }
        }
    }
    outside
}

/// The files [`pack`] would take from `dir`, by their paths: what a
/// project reads, never hidden files, `.git` or anything else in the
/// folder.
pub fn project_file_paths(dir: &Path) -> Result<Vec<PathBuf>, ArchiveError> {
    let mut files = project_files(dir)?;
    let root = fs::canonicalize(dir).map_err(io_error(dir))?;
    files.extend(inside_includes(dir, &root)?);
    Ok(files.into_values().collect())
}

/// The project folder `dir` as a `.signal` archive.
pub fn pack(dir: &Path) -> Result<Vec<u8>, ArchiveError> {
    pack_identified(dir, None)
}

/// Whether `id` is a project id: 32 lowercase hex digits (a name safe as a
/// folder's, since it comes from a file that may come from anywhere).
pub fn valid_project_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// [`pack`], its comment also recording the project id `id` (ADR 0038 §6,
/// amendment of 2026-11-01): the same entries, byte for byte, as [`pack`]
/// makes; only the comment differs. An id that is not
/// [`valid_project_id`] is left out.
pub fn pack_identified(dir: &Path, id: Option<&str>) -> Result<Vec<u8>, ArchiveError> {
    in_memory(&plain_entries(dir)?, id)
}

/// [`pack_identified`] (or [`pack_embedded`] when `embed`) written into a
/// staging file beside `file` ([`write_atomic`]'s), each file streamed into
/// it from the folder rather than held in memory, and the limits checked on
/// the files' sizes before anything is written (the review of 4d37bf8). The
/// caller renames it over `file` ([`Staged::replace`]); dropped, it is
/// removed.
///
/// [`Staged::replace`]: crate::transaction::Staged::replace
pub fn pack_staged(
    dir: &Path,
    id: Option<&str>,
    embed: bool,
    file: &Path,
) -> Result<crate::transaction::Staged, ArchiveError> {
    let entries = if embed {
        embedded_entries(dir)?
    } else {
        plain_entries(dir)?
    };
    check_limits(&entries)?;
    let staged = crate::transaction::stage_with(file, |out: &mut fs::File| {
        write_zip(&entries, id, io::BufWriter::new(out))
            .and_then(|w| {
                w.into_inner()
                    .map(drop)
                    .map_err(|e| io_error(file)(e.into_error()))
            })
            .map_err(|e| io::Error::other(e.to_string()))
    })
    .map_err(io_error(file))?;
    Ok(staged)
}

/// What an entry holds: a file of the folder, read as it is written, or
/// text rewritten here.
enum Content {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// [`pack_identified`]'s entries: the folder's files and its inside
/// includes', refused when it includes from outside.
fn plain_entries(dir: &Path) -> Result<Vec<(String, Content)>, ArchiveError> {
    let mut files = project_files(dir)?;
    let root = fs::canonicalize(dir).map_err(io_error(dir))?;
    files.extend(inside_includes(dir, &root)?);
    // ADR 0035 §13: an include outside the folder is not the folder's;
    // packing the folder alone would make an archive that does not load
    // ([`pack_embedded`] embeds it). One inside is packed at its path.
    let outside = outside_includes(dir, &root);
    if !outside.is_empty() {
        return Err(ArchiveError::Includes(outside));
    }
    Ok(files
        .into_iter()
        .map(|(name, path)| (name, Content::File(path)))
        .collect())
}

/// ADR 0035 §13: the project folder `dir` as a `.signal` archive with every
/// project it includes (or loads into a slot) from outside the folder
/// **embedded**, under `includes/<key>/` (`<key>` the including instance,
/// `<composite>_<instance>` in a composite's file, numbered when taken).
/// Every project in the archive is walked, the included ones too (the
/// review of 4d37bf8: a child's own outside include was left out), and
/// each `project` (or `load`) value naming one is rewritten (only those
/// values, with `toml_edit`) to its path in the archive, relative, so the
/// archive renders as the folder does and never reads the folder it came
/// from (an absolute path to a project inside is rewritten too). One
/// folder included twice is embedded once. An include inside the folder is
/// packed at its path (a `.signal` one as its file); a `.signal` file from
/// outside is embedded unpacked.
pub fn pack_embedded(dir: &Path, id: Option<&str>) -> Result<Vec<u8>, ArchiveError> {
    in_memory(&embedded_entries(dir)?, id)
}

/// `rest` under the archive folder `at` (`""`: the root).
fn under(at: &str, rest: &str) -> String {
    if at.is_empty() {
        rest.to_owned()
    } else {
        format!("{at}/{rest}")
    }
}

/// The path from the archive folder `from` to `to`, `/`-separated.
fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    let to: Vec<&str> = to.split('/').filter(|p| !p.is_empty()).collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

/// `written` as [`relative`] would spell it: no `./`, no trailing `/`.
fn spelled(written: &str) -> &str {
    let mut text = written.trim_end_matches('/');
    while let Some(rest) = text.strip_prefix("./") {
        text = rest;
    }
    text
}

/// [`pack_embedded`]'s entries.
fn embedded_entries(dir: &Path) -> Result<Vec<(String, Content)>, ArchiveError> {
    let root = fs::canonicalize(dir).map_err(io_error(dir))?;
    // Each project (or `.signal` file) the archive holds, by its canonical
    // path, at its folder in the archive (`""`: the root).
    let mut placed: BTreeMap<PathBuf, String> = BTreeMap::from([(root.clone(), String::new())]);
    let mut todo = vec![(root.clone(), String::new())];
    let mut entries: BTreeMap<String, Content> = BTreeMap::new();
    let mut refused: Vec<String> = Vec::new();
    while let Some((folder, at)) = todo.pop() {
        let files = project_files(&folder)?;
        for (name, path) in &files {
            entries.insert(under(&at, name), Content::File(path.clone()));
        }
        for (name, path) in &files {
            let definition = name == PROJECT_FILE
                || (name.starts_with("composites/") && name.ends_with(".toml"));
            if !definition {
                continue;
            }
            let Ok(text) = crate::read_regular_to_string(path) else {
                continue;
            };
            let Ok(mut document) = text.parse::<toml_edit::DocumentMut>() else {
                continue;
            };
            let prefix = name
                .strip_prefix("composites/")
                .and_then(|n| n.strip_suffix(".toml"))
                .map(|c| format!("{c}_"))
                .unwrap_or_default();
            let mut changed = false;
            let Some(instances) = document
                .get_mut("instances")
                .and_then(toml_edit::Item::as_table_like_mut)
            else {
                continue;
            };
            for (instance, entry) in instances.iter_mut() {
                let Some(entry) = entry.as_table_like_mut() else {
                    continue;
                };
                let key = if entry.get("project").is_some() {
                    "project"
                } else if entry.get("slot").is_some() && entry.get("load").is_some() {
                    "load"
                } else {
                    continue;
                };
                let Some(written) = entry.get(key).and_then(|v| v.as_str()).map(str::to_owned)
                else {
                    continue;
                };
                if !crate::is_project_path(&written) && key == "load" {
                    continue;
                }
                // One that does not resolve does not render from the folder
                // either; the loader names it.
                let Ok(found) = fs::canonicalize(folder.join(&written)) else {
                    continue;
                };
                let place = if let Some(place) = placed.get(&found) {
                    place.clone()
                } else if let Ok(place) = found
                    .strip_prefix(&root)
                    .map(|rest| under("", &rest.to_string_lossy()))
                    .or_else(|_| {
                        found
                            .strip_prefix(&folder)
                            .map(|rest| under(&at, &rest.to_string_lossy()))
                    })
                {
                    // Inside the folder (or inside the embedded project
                    // naming it): at its own path.
                    placed.insert(found.clone(), place.clone());
                    if found.is_file() {
                        entries.insert(place.clone(), Content::File(found.clone()));
                    } else {
                        todo.push((found.clone(), place.clone()));
                    }
                    place
                } else {
                    // ADR 0035 §13: a `.signal` include is embedded unpacked.
                    let found = if found.is_file() {
                        match unpacked_include(&found)
                            .and_then(|f| fs::canonicalize(&f).map_err(io_error(&f)))
                        {
                            Ok(unpacked) => unpacked,
                            Err(why) => {
                                refused.push(format!("{written} ({why})"));
                                continue;
                            }
                        }
                    } else {
                        found
                    };
                    match placed.get(&found) {
                        Some(place) => place.clone(),
                        None => {
                            let base = format!("includes/{prefix}{}", instance.get());
                            let mut place = base.clone();
                            let mut n = 2;
                            let taken = |place: &str| {
                                placed.values().any(|v| v == place)
                                    || entries.keys().any(|e| e.starts_with(&format!("{place}/")))
                                    || root.join(place).exists()
                            };
                            while taken(&place) {
                                place = format!("{base}_{n}");
                                n += 1;
                            }
                            placed.insert(found.clone(), place.clone());
                            todo.push((found, place.clone()));
                            place
                        }
                    }
                };
                let wanted = relative(&at, &place);
                if spelled(&written) == wanted {
                    continue;
                }
                if let Some(value) = entry.get_mut(key).and_then(toml_edit::Item::as_value_mut) {
                    let decor = value.decor().clone();
                    *value = toml_edit::Value::from(wanted.as_str());
                    *value.decor_mut() = decor;
                    changed = true;
                }
            }
            if changed {
                entries.insert(
                    under(&at, name),
                    Content::Bytes(document.to_string().into_bytes()),
                );
            }
        }
    }
    if !refused.is_empty() {
        return Err(ArchiveError::Includes(refused));
    }
    Ok(entries.into_iter().collect())
}

/// The bytes `content` holds: a file's length as the folder says now.
fn content_len(content: &Content) -> Result<u64, ArchiveError> {
    match content {
        Content::File(path) => Ok(fs::metadata(path).map_err(io_error(path))?.len()),
        Content::Bytes(bytes) => Ok(bytes.len() as u64),
    }
}

/// [`Limits::DEFAULT`] checked on the entries' sizes, before any is read.
fn check_limits(entries: &[(String, Content)]) -> Result<(), ArchiveError> {
    let limits = Limits::DEFAULT;
    if entries.len() > limits.entries {
        return Err(ArchiveError::TooManyEntries {
            count: entries.len(),
            max: limits.entries,
        });
    }
    let mut total = 0u64;
    for (_, content) in entries {
        total = total.saturating_add(content_len(content)?);
        if total > limits.bytes {
            return Err(ArchiveError::TooLarge { max: limits.bytes });
        }
    }
    Ok(())
}

/// The archive of `entries` as bytes.
fn in_memory(entries: &[(String, Content)], id: Option<&str>) -> Result<Vec<u8>, ArchiveError> {
    check_limits(entries)?;
    Ok(write_zip(entries, id, Cursor::new(Vec::new()))?.into_inner())
}

/// The archive of `entries` (name, content), in order, with the library
/// version (and project id `id`, when valid) in its comment, written into
/// `out`; each file streamed from the folder, never more bytes in all than
/// [`Limits::DEFAULT`] (a file grown since its size was checked included).
fn write_zip<W: Write + io::Seek>(
    entries: &[(String, Content)],
    id: Option<&str>,
    out: W,
) -> Result<W, ArchiveError> {
    let limits = Limits::DEFAULT;
    let options = SimpleFileOptions::DEFAULT
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(DateTime::DEFAULT)
        .unix_permissions(0o644)
        .system(System::Unix);
    let mut writer = ZipWriter::new(out);
    writer
        .set_comment(match id.filter(|id| valid_project_id(id)) {
            Some(id) => format!(
                "{LIBRARY_COMMENT}{}\n{PROJECT_COMMENT}{id}",
                crate::Library::VERSION
            ),
            None => format!("{LIBRARY_COMMENT}{}", crate::Library::VERSION),
        })
        .map_err(zip_error)?;
    let mut total = 0u64;
    for (name, content) in entries {
        writer.start_file(name, options).map_err(zip_error)?;
        let room = limits.bytes - total;
        let copied = match content {
            Content::File(path) => {
                let file = fs::File::open(path).map_err(io_error(path))?;
                io::copy(&mut file.take(room + 1), &mut writer).map_err(io_error(path))?
            }
            Content::Bytes(bytes) => {
                writer.write_all(bytes).map_err(io_error(Path::new(name)))?;
                bytes.len() as u64
            }
        };
        if copied > room {
            return Err(ArchiveError::TooLarge { max: limits.bytes });
        }
        total += copied;
    }
    writer.finish().map_err(zip_error)
}

/// Whether the project folder `dir` packs to `packed`, an archive [`pack`]
/// wrote, without packing it: the same files a project reads, each with the
/// same bytes as its entry once inflated. Nothing is compressed, so this
/// costs a read of the folder and an inflate of the archive (M2.8-12, a
/// `.signal` document's edited state). A folder that would not pack is an
/// error, as from [`pack`].
pub fn packs_to(dir: &Path, packed: &[u8]) -> Result<bool, ArchiveError> {
    let mut files = project_files(dir)?;
    let root = fs::canonicalize(dir).map_err(io_error(dir))?;
    files.extend(inside_includes(dir, &root)?);
    let mut archive = ZipArchive::new(Cursor::new(packed)).map_err(zip_error)?;
    if archive.len() != files.len() {
        return Ok(false);
    }
    // As many entries as files, each file found by its name: the same names.
    for (name, path) in &files {
        let mut entry = match archive.by_name(name) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => return Ok(false),
            Err(error) => return Err(zip_error(error)),
        };
        let file = fs::File::open(path).map_err(io_error(path))?;
        let length = file.metadata().map_err(io_error(path))?.len();
        if entry.size() != length {
            return Ok(false);
        }
        if !same_bytes(&mut entry, file).map_err(io_error(path))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether `a` and `b` read to the same bytes, 64 KiB at a time.
fn same_bytes(mut a: impl Read, mut b: impl Read) -> io::Result<bool> {
    const CHUNK: usize = 64 << 10;
    let (mut x, mut y) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    loop {
        let n = fill(&mut a, &mut x)?;
        if fill(&mut b, &mut y)? != n || x[..n] != y[..n] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

/// Reads into `buf` until it is full or the reader ends; the bytes read.
fn fill(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// What [`unpack`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unpacked {
    /// The files, by their `/`-separated path.
    pub files: Vec<String>,
    /// Their bytes together.
    pub bytes: u64,
    /// The library version the archive records ([`library_version`]).
    pub library_version: Option<String>,
}

/// The built-in library's version (the app's MAJOR.MINOR, ADR 0027 §4) that
/// the archive `bytes` records in its comment; `None` when it records none
/// (an archive packed before it was recorded, or by another tool).
pub fn library_version(bytes: &[u8]) -> Result<Option<String>, ArchiveError> {
    let archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
    Ok(comment_version(archive.comment()))
}

fn comment_version(comment: &[u8]) -> Option<String> {
    let comment = std::str::from_utf8(comment).ok()?;
    let line = comment.lines().next()?;
    let version = line.strip_prefix(LIBRARY_COMMENT)?.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// The project id the archive `bytes` records in its comment
/// ([`pack_identified`]); `None` when it records none, or one that is not
/// [`valid_project_id`].
pub fn project_id(bytes: &[u8]) -> Result<Option<String>, ArchiveError> {
    let archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
    Ok(comment_project_id(archive.comment()))
}

fn comment_project_id(comment: &[u8]) -> Option<String> {
    let comment = std::str::from_utf8(comment).ok()?;
    comment
        .lines()
        .filter_map(|line| line.strip_prefix(PROJECT_COMMENT))
        .map(str::trim)
        .find(|id| valid_project_id(id))
        .map(str::to_owned)
}

/// Why an entry's name is refused, if it is.
fn check_name(name: &str) -> Result<(), ArchiveError> {
    let unsafe_path = || ArchiveError::UnsafePath(name.to_owned());
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    if trimmed.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || trimmed
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(unsafe_path());
    }
    Ok(())
}

/// [`unpack_with`] with [`Limits::DEFAULT`].
pub fn unpack(bytes: &[u8], into: &Path) -> Result<Unpacked, ArchiveError> {
    unpack_with(bytes, into, Limits::DEFAULT)
}

/// Unpacks the archive `bytes` into the folder `into`, which must not exist
/// or be empty. Every entry is checked first; a refusal, or a failure while
/// writing, leaves `into` as it was (removed if this made it).
pub fn unpack_with(bytes: &[u8], into: &Path, limits: Limits) -> Result<Unpacked, ArchiveError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
    if archive.len() > limits.entries {
        return Err(ArchiveError::TooManyEntries {
            count: archive.len(),
            max: limits.entries,
        });
    }
    // Pass 1: every entry's name, kind and declared size.
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut declared = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(zip_error)?;
        let name = entry.name().to_owned();
        check_name(&name)?;
        let kind = entry.unix_mode().map_or(0, |mode| mode & TYPE_BITS);
        let directory = name.ends_with('/') || kind == DIRECTORY;
        match kind {
            SYMLINK => return Err(ArchiveError::Symlink(name)),
            0 | REGULAR | DIRECTORY => {}
            _ => return Err(ArchiveError::SpecialFile(name)),
        }
        if entry.encrypted() {
            return Err(ArchiveError::Encrypted(name));
        }
        let key = name.trim_end_matches('/').to_lowercase();
        if !seen.insert(key) {
            return Err(ArchiveError::Duplicate(name));
        }
        if !directory {
            declared = declared.saturating_add(entry.size());
            if declared > limits.bytes {
                return Err(ArchiveError::TooLarge { max: limits.bytes });
            }
            files.push((index, name));
        }
    }
    if !files.iter().any(|(_, name)| name == PROJECT_FILE) {
        return Err(ArchiveError::NoProjectFile);
    }
    let created = match fs::read_dir(into) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(ArchiveError::NotEmpty(into.to_owned()));
            }
            false
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(into).map_err(io_error(into))?;
            true
        }
        Err(error) => return Err(io_error(into)(error)),
    };
    // Pass 2: the files, never more bytes than the bound however the
    // entries' sizes were declared.
    let written = extract(&mut archive, &files, into, limits);
    if written.is_err() {
        if created {
            let _ = fs::remove_dir_all(into);
        } else if let Ok(entries) = fs::read_dir(into) {
            for entry in entries.flatten() {
                let _ = fs::remove_dir_all(entry.path()).or_else(|_| fs::remove_file(entry.path()));
            }
        }
    }
    written
}

fn extract(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    files: &[(usize, String)],
    into: &Path,
    limits: Limits,
) -> Result<Unpacked, ArchiveError> {
    let mut total = 0u64;
    for (index, name) in files {
        let path = into.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error(parent))?;
        }
        let mut entry = archive.by_index(*index).map_err(zip_error)?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error(&path))?;
        let room = limits.bytes - total;
        let copied = io::copy(&mut (&mut entry).take(room + 1), &mut out).map_err(|error| {
            match error.kind() {
                io::ErrorKind::InvalidData => {
                    ArchiveError::NotAnArchive(format!("{name}: {error}"))
                }
                _ => io_error(&path)(error),
            }
        })?;
        if copied > room {
            return Err(ArchiveError::TooLarge { max: limits.bytes });
        }
        total += copied;
    }
    Ok(Unpacked {
        files: files.iter().map(|(_, name)| name.clone()).collect(),
        bytes: total,
        library_version: comment_version(archive.comment()),
    })
}

/// The bytes of the archive at `path`; one larger than any archive within
/// [`Limits::DEFAULT`] could be is refused before it is read.
pub fn read_archive(path: &Path) -> Result<Vec<u8>, ArchiveError> {
    let length = fs::metadata(path).map_err(io_error(path))?.len();
    if length > Limits::DEFAULT.bytes + ARCHIVE_SLACK {
        return Err(ArchiveError::TooLarge {
            max: Limits::DEFAULT.bytes,
        });
    }
    fs::read(path).map_err(io_error(path))
}

/// Writes `bytes` to `path` atomically: to a hidden staging file of this
/// writer's beside it, synced, then renamed over it. A failure leaves the old
/// file as it was. The staging file is always a new one (created
/// exclusively, which never follows a link): an entry already at its name is
/// left alone and another name is taken ([`crate::transaction::stage`]).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ArchiveError> {
    crate::transaction::write_atomic(path, bytes).map_err(io_error(path))
}

/// A new folder of this process's under `parent`, named after `stem`.
pub fn unique_folder(parent: &Path, stem: &str) -> Result<PathBuf, ArchiveError> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    fs::create_dir_all(parent).map_err(io_error(parent))?;
    loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!("{stem}-{}-{n}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(&dir)(error)),
        }
    }
}

/// ADR 0035 §7: the folder a `.signal` include `file` plays from, unpacked
/// read-only under the system's temporary folder in a folder named after
/// the archive's FNV-1a 64 (`signal-include-<hash>`), so the same bytes
/// reuse one folder and a changed file unpacks into another. Left for the
/// system to clean, since any number of hosts may play it.
pub fn unpacked_include(file: &Path) -> Result<PathBuf, ArchiveError> {
    let bytes = read_archive(file)?;
    let parent = std::env::temp_dir();
    let at = parent.join(format!(
        "signal-include-{:016x}",
        signal_core::fnv1a64(&bytes)
    ));
    if at.join(PROJECT_FILE).is_file() {
        return Ok(at);
    }
    let fresh = unique_folder(&parent, "signal-include-new")?;
    let unpacked = unpack(&bytes, &fresh).and_then(|unpacked| {
        for name in &unpacked.files {
            let path = fresh.join(name);
            let mut permissions = fs::metadata(&path).map_err(io_error(&path))?.permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&path, permissions).map_err(io_error(&path))?;
        }
        Ok(())
    });
    if let Err(error) = unpacked {
        let _ = fs::remove_dir_all(&fresh);
        return Err(error);
    }
    // Another host may have unpacked the same bytes meanwhile: keep its.
    if fs::rename(&fresh, &at).is_err() {
        let _ = fs::remove_dir_all(&fresh);
        if !at.join(PROJECT_FILE).is_file() {
            return Err(io_error(&at)(io::Error::other("could not unpack here")));
        }
    }
    Ok(at)
}

/// A `.signal` file unpacked into a temporary folder for one run (`signal
/// render`, `play`, `ui`); the folder is removed when this is dropped.
#[derive(Debug)]
pub struct TempProject {
    dir: PathBuf,
}

impl TempProject {
    /// Unpacks the project file `file` under the system's temporary folder.
    pub fn unpack(file: &Path) -> Result<TempProject, ArchiveError> {
        let bytes = read_archive(file)?;
        let stem = file.file_stem().map_or_else(
            || "project".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let dir = unique_folder(&std::env::temp_dir(), &format!("signal-{stem}"))?;
        let temp = TempProject { dir };
        unpack(&bytes, &temp.dir)?;
        Ok(temp)
    }

    /// The project folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_relative_paths_inside_the_project() {
        for good in ["project.toml", "ui/assets/knob.svg", "ui/"] {
            assert!(check_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "ui/../../x",
            "ui//x",
            "./x",
            "ui\\x",
            "a\u{0}b",
        ] {
            assert!(
                matches!(check_name(bad), Err(ArchiveError::UnsafePath(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_project_reads_its_toml_composites_patterns_and_ui() {
        for read in [
            "project.toml",
            "snapshots.toml",
            "composites/kick.toml",
            "patterns/melody.toml",
            "interfaces/deck.toml",
            "ui/main.html",
            "ui/assets/a.png",
        ] {
            assert!(read_by_project(read), "{read}");
        }
        for not in [
            "notes.txt",
            "composites/a.txt",
            "composites/x/a.toml",
            "patterns/a.txt",
            "patterns/x/a.toml",
            "interfaces/x/a.toml",
            "renders/a.wav",
            "ui",
        ] {
            assert!(!read_by_project(not), "{not}");
        }
    }

    /// ADR 0038 §6 (amendment of 2026-11-01): the project id is the
    /// comment's second line, never an entry: the same entries as `pack`'s,
    /// the library version still read, an id that is not 32 lowercase hex
    /// digits never read (nor written).
    #[test]
    fn the_project_id_is_in_the_comment_only() {
        let kick = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick");
        let plain = pack(&kick).unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let identified = pack_identified(&kick, Some(id)).unwrap();
        assert_ne!(plain, identified);
        assert!(packs_to(&kick, &identified).unwrap());
        assert_eq!(project_id(&identified).unwrap().as_deref(), Some(id));
        assert_eq!(project_id(&plain).unwrap(), None);
        assert_eq!(
            library_version(&identified).unwrap().as_deref(),
            Some(crate::Library::VERSION)
        );
        for bad in ["../../x", "0123456789ABCDEF0123456789ABCDEF", "abc"] {
            assert_eq!(pack_identified(&kick, Some(bad)).unwrap(), plain, "{bad}");
            assert!(
                comment_project_id(format!("signal-library 0.3\nproject {bad}").as_bytes())
                    .is_none()
            );
        }
    }
}
