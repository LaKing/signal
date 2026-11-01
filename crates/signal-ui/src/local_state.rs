//! Machine-local state of a `.signal` project (ADR 0038 §6, amendment of
//! 2026-11-01): this project's routing on this machine
//! (`devices.local.toml`) kept outside the project, because a `.signal`
//! project's working folder is removed when it closes and the file is never
//! packed. A folder project keeps its `devices.local.toml` in its folder, as
//! before; this module is for project files.
//!
//! - **Where:** `Local/<id>/` beside the Setup file (by default
//!   `~/Library/Application Support/Signal/Local/`; `--setup` and
//!   `SIGNAL_SETUP` move it with the Setup, which is how the tests use a
//!   scratch folder). `<id>/identity.toml` records the id and the file it
//!   was last seen as; nothing in it is read by a plan or a render.
//! - **The project's identity** is its id: 32 hex digits in the archive's
//!   comment (`signal_project::archive::project_id`), written by every save
//!   of Signal.app; never an entry of the archive, so never part of what
//!   the project means.
//! - **[`resolve`]:** a file with an id is that id. If the id has no state
//!   yet, the state last recorded for the same canonical path (another id)
//!   is copied to it: a file replaced in place, or one written by a tool
//!   that kept no id, keeps its routing. A file without an id takes the id
//!   recorded for its path, else a fresh one; its first save writes it.
//! - **Copies** of a file (the Finder's Duplicate) carry the same id and
//!   share one state until one of them is saved as another file: Save As
//!   gives the new file a fresh id and [`copy_state`] copies the state to
//!   it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use signal_project::archive::valid_project_id;

use crate::panel_write::sha256_hex;

/// The folder beside the Setup file that holds every project's state.
pub const DIR: &str = "Local";

/// Each state folder's record of its id and its file.
pub const IDENTITY: &str = "identity.toml";

/// Where the machine-local states of projects live: `Local/` beside the
/// Setup file `setup_path`.
pub fn root(setup_path: &Path) -> PathBuf {
    setup_path.with_file_name(DIR)
}

/// The state folder of the project id `id` under `root`.
pub fn dir(root: &Path, id: &str) -> PathBuf {
    root.join(id)
}

/// A fresh project id: 32 hex digits from the file's path, the time, the
/// process and a counter. Not semantic (it names where machine state is
/// kept), so the time and process are allowed here (hard rule 3 is about
/// what a project means and plays).
pub fn fresh_id(file: &Path) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seed = format!(
        "{}\n{nanos}\n{}\n{}",
        canonical(file).display(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    sha256_hex(seed.as_bytes())[..32].to_owned()
}

/// `file` canonical, else absolute, else as given.
fn canonical(file: &Path) -> PathBuf {
    fs::canonicalize(file)
        .or_else(|_| std::path::absolute(file))
        .unwrap_or_else(|_| file.to_owned())
}

/// The id of the project file `file`, whose archive records `archived` (or
/// none), and its state seeded as the module says. Never fails: a state
/// that cannot be read or copied is a state not found.
pub fn resolve(root: &Path, file: &Path, archived: Option<&str>) -> String {
    let file = canonical(file);
    let Some(id) = archived.filter(|id| valid_project_id(id)) else {
        return by_path(root, &file).unwrap_or_else(|| fresh_id(&file));
    };
    let own = dir(root, id);
    if !has_state(&own)
        && let Some(other) = by_path(root, &file).filter(|other| other != id)
    {
        let _ = copy_state(&dir(root, &other), &own);
    }
    // Moved: the record follows the file, so a later copy without an id
    // finds it by its new path.
    if own.is_dir() && recorded_file(&own).as_deref() != Some(file.as_path()) {
        let _ = record(&own, id, &file);
    }
    id.to_owned()
}

/// Before a write of the state of `id`, the project file `file`: its
/// folder, made if missing, and its record; the folder.
pub fn prepare(root: &Path, id: &str, file: &Path) -> io::Result<PathBuf> {
    let own = dir(root, id);
    fs::create_dir_all(&own)?;
    if recorded_file(&own).as_deref() != Some(canonical(file).as_path()) {
        record(&own, id, &canonical(file))?;
    }
    Ok(own)
}

/// Copies the state files of the folder `from` (every regular file but the
/// record) into `to`, made if missing; a file `to` already has is kept.
pub fn copy_state(from: &Path, to: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if name == IDENTITY || !entry.file_type()?.is_file() {
            continue;
        }
        fs::create_dir_all(to)?;
        let target = to.join(&name);
        if target.exists() {
            continue;
        }
        let bytes = fs::read(entry.path())?;
        signal_project::transaction::write_atomic(&target, &bytes)?;
    }
    Ok(())
}

/// Whether the folder holds a state file (anything but the record).
fn has_state(own: &Path) -> bool {
    fs::read_dir(own).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.file_name() != IDENTITY && e.file_type().is_ok_and(|t| t.is_file()))
    })
}

/// The file the folder's record names.
fn recorded_file(own: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(own.join(IDENTITY)).ok()?;
    let table = text.parse::<toml::Table>().ok()?;
    table.get("file")?.as_str().map(PathBuf::from)
}

fn record(own: &Path, id: &str, file: &Path) -> io::Result<()> {
    let mut table = toml::Table::new();
    table.insert("id".to_owned(), toml::Value::String(id.to_owned()));
    table.insert(
        "file".to_owned(),
        toml::Value::String(file.to_string_lossy().into_owned()),
    );
    let text = format!(
        "# Signal: this machine's state of the project file named here (ADR 0038 §6).\n{}",
        toml::to_string(&table).map_err(io::Error::other)?
    );
    signal_project::transaction::write_atomic(&own.join(IDENTITY), text.as_bytes())
}

/// The id whose record names `file` (canonical), the most recently
/// recorded when several do.
fn by_path(root: &Path, file: &Path) -> Option<String> {
    let mut found: Option<(SystemTime, String)> = None;
    for entry in fs::read_dir(root).ok()?.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !valid_project_id(&name) || recorded_file(&entry.path()).as_deref() != Some(file) {
            continue;
        }
        let when = fs::metadata(entry.path().join(IDENTITY))
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        if found.as_ref().is_none_or(|(at, _)| when > *at) {
            found = Some((when, name));
        }
    }
    found.map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "signal-ui-local-state-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn fresh_ids_are_valid_and_differ() {
        let a = fresh_id(Path::new("/x/a.signal"));
        let b = fresh_id(Path::new("/x/a.signal"));
        assert!(valid_project_id(&a), "{a}");
        assert_ne!(a, b);
    }

    /// The module's rules: by id; a file without an id by its path; an id
    /// without state seeded from the path's; a moved file keeps its id and
    /// its record follows it; copies share; Save As copies.
    #[test]
    fn states_are_found_by_id_then_by_path() {
        let base = scratch("rules");
        let root = base.join("Local");
        let file = base.join("a.signal");
        fs::write(&file, "x").unwrap();

        // No id, nothing recorded: a fresh id, nothing made.
        let first = resolve(&root, &file, None);
        assert!(!root.exists());
        // Written: found again by path while the file has no id.
        let own = prepare(&root, &first, &file).unwrap();
        fs::write(
            own.join("devices.local.toml"),
            "[play.cue]\nchannels = [3, 4]\n",
        )
        .unwrap();
        assert_eq!(resolve(&root, &file, None), first);
        // Saved with that id: the same state.
        assert_eq!(resolve(&root, &file, Some(&first)), first);

        // Replaced in place by a file with another id: seeded from the path.
        let other = fresh_id(&file);
        assert_eq!(resolve(&root, &file, Some(&other)), other);
        assert_eq!(
            fs::read_to_string(dir(&root, &other).join("devices.local.toml")).unwrap(),
            "[play.cue]\nchannels = [3, 4]\n"
        );

        // Moved, keeping its id: found by id, the record follows.
        let moved = base.join("moved.signal");
        fs::rename(&file, &moved).unwrap();
        assert_eq!(resolve(&root, &moved, Some(&first)), first);
        assert_eq!(recorded_file(&dir(&root, &first)), Some(canonical(&moved)));

        // Save As: a fresh id, the state copied, the new state its own.
        let copy = base.join("copy.signal");
        fs::write(&copy, "x").unwrap();
        let fresh = fresh_id(&copy);
        copy_state(&dir(&root, &first), &dir(&root, &fresh)).unwrap();
        prepare(&root, &fresh, &copy).unwrap();
        assert!(dir(&root, &fresh).join("devices.local.toml").is_file());
        assert_eq!(recorded_file(&dir(&root, &fresh)), Some(canonical(&copy)));
        assert_eq!(recorded_file(&dir(&root, &first)), Some(canonical(&moved)));

        // An id from an archive that is no id is never a folder name.
        let unsafe_id = resolve(&root, &moved, Some("../../etc"));
        assert!(valid_project_id(&unsafe_id));
        fs::remove_dir_all(&base).unwrap();
    }
}
