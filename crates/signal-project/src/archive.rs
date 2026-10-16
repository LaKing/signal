//! Project files (`.signal`, ADR 0025): a project folder as one zip, a
//! container for the same text files and panel assets, never a format of its
//! own (`docs/project-format.md` §10).
//!
//! [`pack`] takes what a project reads (`project.toml`, `composites/*.toml`,
//! the files under `ui/`) and nothing else, in sorted path order, deflated,
//! with a fixed timestamp and fixed permissions, so the same folder always
//! gives the same bytes. [`unpack`] checks every entry before it writes one:
//! relative paths only, regular files only, at most [`Limits::DEFAULT`]
//! entries and unpacked bytes, and `project.toml` at the root. A refused
//! archive leaves nothing behind, and the [`ArchiveError`] names the reason.

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
/// `project.toml`, a composite in `composites/` or a file under `ui/`.
fn read_by_project(relative: &str) -> bool {
    let parts: Vec<&str> = relative.split('/').collect();
    match parts.as_slice() {
        [PROJECT_FILE] => true,
        ["composites", name] => name.ends_with(".toml"),
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
            if kind.is_symlink() && prefix.is_empty() && (name == "composites" || name == "ui") {
                return Err(ArchiveError::Symlink(relative));
            }
            if kind.is_dir() {
                // Only composites/ and ui/ hold files a project reads.
                let top = relative.split('/').next().unwrap_or_default();
                if top == "composites" && prefix.is_empty() || top == "ui" {
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
    Ok(files)
}

/// The project folder `dir` as a `.signal` archive.
pub fn pack(dir: &Path) -> Result<Vec<u8>, ArchiveError> {
    let files = project_files(dir)?;
    let limits = Limits::DEFAULT;
    if files.len() > limits.entries {
        return Err(ArchiveError::TooManyEntries {
            count: files.len(),
            max: limits.entries,
        });
    }
    let options = SimpleFileOptions::DEFAULT
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(DateTime::DEFAULT)
        .unix_permissions(0o644)
        .system(System::Unix);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut total = 0u64;
    for (name, path) in &files {
        let bytes = fs::read(path).map_err(io_error(path))?;
        total += bytes.len() as u64;
        if total > limits.bytes {
            return Err(ArchiveError::TooLarge { max: limits.bytes });
        }
        writer.start_file(name, options).map_err(zip_error)?;
        writer.write_all(&bytes).map_err(io_error(path))?;
    }
    Ok(writer.finish().map_err(zip_error)?.into_inner())
}

/// Whether the project folder `dir` packs to `packed`, an archive [`pack`]
/// wrote, without packing it: the same files a project reads, each with the
/// same bytes as its entry once inflated. Nothing is compressed, so this
/// costs a read of the folder and an inflate of the archive (M2.8-12, a
/// `.signal` document's edited state). A folder that would not pack is an
/// error, as from [`pack`].
pub fn packs_to(dir: &Path, packed: &[u8]) -> Result<bool, ArchiveError> {
    let files = project_files(dir)?;
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

/// Writes `bytes` to `path` atomically: to a hidden temporary file beside
/// it, synced, then renamed over it. A failure leaves the old file as it was.
/// The temporary file is always a new one (created exclusively, which never
/// follows a link): an entry already at its name is left alone and another
/// name is taken.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ArchiveError> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let pid = std::process::id();
    let mut attempt = 0u32;
    let (temporary, mut file) = loop {
        let temporary = match attempt {
            0 => path.with_file_name(format!(".{name}.{pid}.tmp")),
            n => path.with_file_name(format!(".{name}.{pid}.{n}.tmp")),
        };
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 1000 => {
                attempt += 1;
            }
            Err(error) => return Err(io_error(path)(error)),
        }
    };
    let written = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        // Only the file made here.
        let _ = fs::remove_file(&temporary);
    }
    written.map_err(io_error(path))
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
    fn a_project_reads_its_toml_composites_and_ui() {
        for read in [
            "project.toml",
            "composites/kick.toml",
            "ui/main.html",
            "ui/assets/a.png",
        ] {
            assert!(read_by_project(read), "{read}");
        }
        for not in [
            "notes.txt",
            "composites/a.txt",
            "composites/x/a.toml",
            "renders/a.wav",
            "ui",
        ] {
            assert!(!read_by_project(not), "{not}");
        }
    }
}
