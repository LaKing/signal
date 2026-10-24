//! Snapshots (ADR 0041, `project-format.md` §16): the values of a project's
//! parameters stored under a name a person gives, in the project's
//! `snapshots.toml`, so that they can be saved, restored and deleted.
//!
//! This module is the file: its names, its canonical text, and the writer
//! that saves or deletes one snapshot. A snapshot's values are keyed by the
//! parameter's full address (`/out.gain`: the instance's address and the
//! parameter's name), in byte order, so the same snapshots always give the
//! same bytes. Which parameters a snapshot holds and how a restore writes
//! them back into `project.toml` is the caller's (`signal-ui`'s panel: the
//! top-level controls, written where a parameter edit writes them).
//!
//! Every write goes through [`crate::transaction`] (ADR 0040): under the
//! project's writer lock, compared just before its rename, so a change made
//! meanwhile by another writer is [`SnapshotError::Changed`], never lost.
//! Control-thread code: it allocates and does I/O.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;

use signal_core::PortAddress;
use toml_edit::{DocumentMut, Item, Table, Value};

use crate::transaction::{self, ProjectLock};
use crate::write::number;

/// The snapshots file's path in a project folder.
pub const SNAPSHOTS_FILE: &str = "snapshots.toml";

/// The longest name, in characters (Unicode scalar values), after trimming.
pub const MAX_NAME_CHARS: usize = 64;

/// The file's schema version.
const SCHEMA_VERSION: i64 = 1;

/// The first line Signal writes, for a person who opens the file.
const HEADER: &str = "# Snapshots: parameter values under a name (docs/project-format.md §16).\n# Signal writes this file whole; comments are not kept.\n";

/// One snapshot's values: each parameter's full address (`/out.gain`) and
/// its value.
pub type Values = BTreeMap<String, f64>;

/// A project's snapshots, by name, and the undo snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshots {
    /// Every snapshot by name, in byte order.
    pub snapshots: BTreeMap<String, Values>,
    /// The undo snapshot (`[undo]`, ADR 0041 §7): the values the last
    /// restore (or undo) replaced, apart from the named ones so that no
    /// name can be taken for it.
    pub undo: Option<Values>,
}

/// Why a snapshot was not read, saved, restored or deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// Not a name: empty once trimmed, longer than [`MAX_NAME_CHARS`], or
    /// holding a control character.
    BadName(String),
    /// No snapshot has this name.
    Unknown(String),
    /// There is no undo snapshot (no restore was made).
    NoUndo,
    /// A snapshot has this name already, and overwriting it was not asked.
    Exists(String),
    /// `snapshots.toml` does not read as snapshots (it is never overwritten).
    Invalid(String),
    /// The file changed meanwhile (another writer): nothing was written.
    Changed(String),
    /// The file could not be read or written.
    Io(String),
    /// `snapshots.toml` is a symbolic link, which is never read or written
    /// through.
    Link(String),
}

impl SnapshotError {
    /// The UI protocol's error code for it.
    pub fn code(&self) -> &'static str {
        match self {
            SnapshotError::BadName(_) => "bad-value",
            SnapshotError::Unknown(_) | SnapshotError::NoUndo => "unknown-snapshot",
            SnapshotError::Exists(_) => "exists",
            SnapshotError::Invalid(_) => "does-not-load",
            SnapshotError::Changed(_) => "conflict",
            SnapshotError::Io(_) => "write-failed",
            SnapshotError::Link(_) => "bad-value",
        }
    }
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::BadName(message)
            | SnapshotError::Invalid(message)
            | SnapshotError::Changed(message)
            | SnapshotError::Io(message)
            | SnapshotError::Link(message) => f.write_str(message),
            SnapshotError::Unknown(name) => write!(f, "no snapshot is named \"{name}\""),
            SnapshotError::NoUndo => {
                f.write_str("there is nothing to undo: no restore has been made")
            }
            SnapshotError::Exists(name) => write!(
                f,
                "a snapshot is named \"{name}\" already; save with overwrite to replace it"
            ),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// `raw` as a snapshot name: trimmed, not empty, at most
/// [`MAX_NAME_CHARS`] characters, without control characters. Spaces,
/// accents and any other printable text are kept as given.
pub fn name(raw: &str) -> Result<String, SnapshotError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(SnapshotError::BadName(
            "a snapshot needs a name (empty once trimmed)".to_owned(),
        ));
    }
    let chars = name.chars().count();
    if chars > MAX_NAME_CHARS {
        return Err(SnapshotError::BadName(format!(
            "a snapshot's name is at most {MAX_NAME_CHARS} characters ({chars} given)"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(SnapshotError::BadName(
            "a snapshot's name cannot hold a control character (a tab, a line break)".to_owned(),
        ));
    }
    if raw
        .chars()
        .any(|c| format_char(c) || c == '\u{2028}' || c == '\u{2029}')
    {
        return Err(SnapshotError::BadName(
            "a snapshot's name cannot hold an invisible format character (a zero-width space, \
             a byte-order mark, a direction mark) or a line or paragraph separator"
                .to_owned(),
        ));
    }
    if name.chars().all(invisible) {
        return Err(SnapshotError::BadName(
            "a snapshot's name must show something (it is all blank characters)".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

/// Whether `c` is of the Unicode general category Cf (format: zero-width
/// and direction marks, the byte-order mark, …), Unicode 15.1. Names are not
/// normalised (no crate of the workspace offers it), so these are refused
/// rather than kept as invisible differences.
fn format_char(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2
        | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// Whether `c` shows nothing: white space, and the blank letters and
/// fillers that render as space (Hangul fillers, the blank Braille cell,
/// the combining grapheme joiner).
fn invisible(c: char) -> bool {
    c.is_whitespace()
        || format_char(c)
        || matches!(
            c as u32,
            0x034F | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x2800 | 0x3164 | 0xFFA0
        )
}

/// `text`, a `snapshots.toml`, read. An empty text (or only comments) is
/// no snapshots. Unknown keys, a name that is not one ([`name`], or not
/// trimmed), an address that is not a parameter's (`/instance.parameter`)
/// and a value that is not a finite number are errors.
pub fn parse(text: &str) -> Result<Snapshots, SnapshotError> {
    let invalid = |message: String| SnapshotError::Invalid(format!("{SNAPSHOTS_FILE}: {message}"));
    let document: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| invalid(e.to_string().trim_end().to_owned()))?;
    let mut snapshots = Snapshots::default();
    let mut version = None;
    for (key, item) in document.iter() {
        match key {
            "schema_version" => {
                version = item.as_integer();
                if version != Some(SCHEMA_VERSION) {
                    return Err(invalid(format!("schema_version must be {SCHEMA_VERSION}")));
                }
            }
            "snapshots" => {
                let table = item
                    .as_table_like()
                    .ok_or_else(|| invalid("`snapshots` must be a table".to_owned()))?;
                for (raw, values) in table.iter() {
                    let checked = name(raw).map_err(|e| invalid(format!("\"{raw}\": {e}")))?;
                    if checked != raw {
                        return Err(invalid(format!(
                            "\"{raw}\": a snapshot's name has no space around it"
                        )));
                    }
                    let values = values.as_table_like().ok_or_else(|| {
                        invalid(format!("snapshot \"{raw}\" must be a table of values"))
                    })?;
                    let read = values_of(values, &format!("snapshot \"{raw}\""))?;
                    snapshots.snapshots.insert(checked, read);
                }
            }
            "undo" => {
                let values = item
                    .as_table_like()
                    .ok_or_else(|| invalid("`undo` must be a table of values".to_owned()))?;
                snapshots.undo = Some(values_of(values, "the undo snapshot")?);
            }
            other => return Err(invalid(format!("unknown key `{other}`"))),
        }
    }
    if version.is_none() && (!snapshots.snapshots.is_empty() || snapshots.undo.is_some()) {
        return Err(invalid(format!(
            "schema_version = {SCHEMA_VERSION} is missing"
        )));
    }
    Ok(snapshots)
}

/// A table of values (`"/instance.parameter" = <number>`), `what` naming
/// it in errors.
fn values_of(values: &dyn toml_edit::TableLike, what: &str) -> Result<Values, SnapshotError> {
    let invalid = |message: String| SnapshotError::Invalid(format!("{SNAPSHOTS_FILE}: {message}"));
    let mut read = Values::new();
    for (address, value) in values.iter() {
        if address.parse::<PortAddress>().is_err() {
            return Err(invalid(format!(
                "{what}: \"{address}\" is not a parameter's address (`/instance.parameter`)"
            )));
        }
        let number = match value.as_value() {
            Some(Value::Integer(i)) => Some(*i.value() as f64),
            Some(Value::Float(f)) => Some(*f.value()),
            _ => None,
        }
        .filter(|v| v.is_finite())
        .ok_or_else(|| invalid(format!("{what}: \"{address}\" must be a finite number")))?;
        read.insert(address.to_owned(), number);
    }
    Ok(read)
}

/// The canonical text of `snapshots`: the header, `schema_version`, the
/// undo snapshot `[undo]` when there is one, then each snapshot as a table `[snapshots."<name>"]` in byte order of the
/// names, its values in byte order of their addresses, each written in the
/// shortest form that reads back to the same `f64` (an integer when it is
/// one). The same snapshots always give the same bytes.
pub fn render(snapshots: &Snapshots) -> String {
    let mut document = DocumentMut::new();
    document.insert("schema_version", toml_edit::value(SCHEMA_VERSION));
    if let Some(undo) = &snapshots.undo {
        let mut table = Table::new();
        table.decor_mut().set_prefix("\n");
        for (address, value) in undo {
            table.insert(address, Item::Value(number(*value)));
        }
        document.insert("undo", Item::Table(table));
    }
    let mut all = Table::new();
    all.set_implicit(true);
    for (name, values) in &snapshots.snapshots {
        let mut table = Table::new();
        table.decor_mut().set_prefix("\n");
        for (address, value) in values {
            table.insert(address, Item::Value(number(*value)));
        }
        all.insert(name, Item::Table(table));
    }
    document.insert("snapshots", Item::Table(all));
    format!("{HEADER}{document}")
}

/// The snapshots of the project folder `dir` and the file's text as read
/// (`None`: no file, which is no snapshots).
pub fn read(dir: &Path) -> Result<(Snapshots, Option<String>), SnapshotError> {
    let path = dir.join(SNAPSHOTS_FILE);
    // Never followed: its target's text would reach every reader (and an
    // error quoting it), whatever file the link names.
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(SnapshotError::Link(format!(
            "{SNAPSHOTS_FILE} is a link; snapshots are read only from the project's own file"
        )));
    }
    let bytes = transaction::current(&path)
        .map_err(|e| SnapshotError::Io(format!("{} could not be read: {e}", path.display())))?;
    match bytes {
        None => Ok((Snapshots::default(), None)),
        Some(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| {
                SnapshotError::Invalid(format!("{SNAPSHOTS_FILE}: the file is not UTF-8"))
            })?;
            Ok((parse(&text)?, Some(text)))
        }
    }
}

/// Edits the snapshots of `dir` (ADR 0040): under the project's writer
/// lock it reads `snapshots.toml`, lets `check` refuse the text it read (a
/// caller's `base`), applies `edit` and writes the canonical text, compared
/// just before its rename: a file that no longer holds what was read is
/// [`SnapshotError::Changed`] and nothing is written. A file that does not
/// read ([`parse`]) is refused and never overwritten; a file reached through
/// a link is refused. Gives `edit`'s result, the file's text as this call
/// left it (the text written, or the text read under the lock when it
/// already was the edit's), and whether it wrote.
pub fn edit<T>(
    dir: &Path,
    check: impl FnOnce(Option<&str>) -> Result<(), SnapshotError>,
    edit: impl FnOnce(&mut Snapshots) -> Result<T, SnapshotError>,
) -> Result<(T, String, bool), SnapshotError> {
    let path = dir.join(SNAPSHOTS_FILE);
    let lock = ProjectLock::acquire(dir)
        .map_err(|e| SnapshotError::Io(format!("{} was not written: {e}", path.display())))?;
    let edited = edit_locked(dir, &lock, check, edit)?;
    Ok((edited.result, edited.after, edited.wrote))
}

/// What [`edit_locked`] did.
#[derive(Debug)]
pub struct Edited<T> {
    /// The edit's result.
    pub result: T,
    /// The file's text as read under the lock (`None`: no file).
    pub before: Option<String>,
    /// The file's text as the edit left it.
    pub after: String,
    /// Whether it wrote.
    pub wrote: bool,
}

/// [`edit`] by a writer that already holds the project's writer `lock`
/// (ADR 0040: a writer never takes it twice), so that it can write
/// `snapshots.toml` and `project.toml` under one lock (a restore and its
/// undo snapshot, ADR 0041 §7).
pub fn edit_locked<T>(
    dir: &Path,
    _lock: &ProjectLock,
    check: impl FnOnce(Option<&str>) -> Result<(), SnapshotError>,
    edit: impl FnOnce(&mut Snapshots) -> Result<T, SnapshotError>,
) -> Result<Edited<T>, SnapshotError> {
    let path = dir.join(SNAPSHOTS_FILE);
    let failed =
        |error: String| SnapshotError::Io(format!("{} was not written: {error}", path.display()));
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(failed(
            "it is a link, and snapshots are written only into the project's own file".to_owned(),
        ));
    }
    let (mut snapshots, before) = read(dir)?;
    check(before.as_deref())?;
    let result = edit(&mut snapshots)?;
    let after = render(&snapshots);
    if before.as_deref() == Some(after.as_str()) {
        return Ok(Edited {
            result,
            before,
            after,
            wrote: false,
        });
    }
    let written = transaction::replace_unchanged(
        &path,
        before.as_deref().map(str::as_bytes),
        after.as_bytes(),
    )
    .map_err(|e| failed(e.to_string()))?;
    if !written {
        return Err(SnapshotError::Changed(format!(
            "{} changed while it was written (another writer); nothing was written",
            path.display()
        )));
    }
    Ok(Edited {
        result,
        before,
        after,
        wrote: true,
    })
}

/// Puts `snapshots.toml` back to `before` after a write of `written` whose
/// change must not stay (a restore whose `project.toml` write failed: its
/// undo snapshot goes back to the one before). Under the held `lock`, and
/// only while the file still holds `written` (compared, ADR 0040): gives
/// whether it was put back.
pub fn put_back(
    dir: &Path,
    _lock: &ProjectLock,
    written: &str,
    before: &str,
) -> Result<bool, SnapshotError> {
    let path = dir.join(SNAPSHOTS_FILE);
    transaction::replace_unchanged(&path, Some(written.as_bytes()), before.as_bytes())
        .map_err(|e| SnapshotError::Io(format!("{} was not put back: {e}", path.display())))
}

/// Saves `values` as the snapshot `name` of `dir` ([`edit`]): refused with
/// [`SnapshotError::Exists`] when one has that name and `overwrite` is
/// false. Gives whether it replaced one, then the file's text and whether
/// it wrote ([`edit`]).
pub fn save(
    dir: &Path,
    name: &str,
    values: Values,
    overwrite: bool,
    check: impl FnOnce(Option<&str>) -> Result<(), SnapshotError>,
) -> Result<(bool, String, bool), SnapshotError> {
    let name = self::name(name)?;
    edit(dir, check, |snapshots| {
        let replaced = snapshots.snapshots.contains_key(&name);
        if replaced && !overwrite {
            return Err(SnapshotError::Exists(name.clone()));
        }
        snapshots.snapshots.insert(name.clone(), values);
        Ok(replaced)
    })
}

/// Deletes the snapshot `name` of `dir` ([`edit`]): [`SnapshotError::Unknown`]
/// when none has that name. The file stays, with `schema_version` alone,
/// when the last one goes. Gives the snapshot's values, then the file's
/// text and whether it wrote ([`edit`]).
pub fn delete(
    dir: &Path,
    name: &str,
    check: impl FnOnce(Option<&str>) -> Result<(), SnapshotError>,
) -> Result<(Values, String, bool), SnapshotError> {
    let name = self::name(name)?;
    edit(dir, check, |snapshots| {
        snapshots
            .snapshots
            .remove(&name)
            .ok_or(SnapshotError::Unknown(name.clone()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(name("  Verse A ").unwrap(), "Verse A");
        assert_eq!(
            name("Árvíztűrő tükörfúrógép").unwrap(),
            "Árvíztűrő tükörfúrógép"
        );
        assert!(matches!(name("   "), Err(SnapshotError::BadName(_))));
        assert!(matches!(name("a\tb"), Err(SnapshotError::BadName(_))));
        for bad in [
            "a\u{200B}b",
            "\u{FEFF}Verse",
            "x\u{2028}y",
            "x\u{2029}y",
            "\u{200E}rtl",
            "\u{3164}",
            "\u{2800}\u{2800}",
            "\u{3000}",
        ] {
            assert!(
                matches!(name(bad), Err(SnapshotError::BadName(_))),
                "{bad:?}"
            );
        }
        // Composed and decomposed forms are not normalised: two names.
        assert_ne!(name("e\u{301}").unwrap(), name("\u{e9}").unwrap());
        let long = "é".repeat(MAX_NAME_CHARS);
        assert_eq!(name(&long).unwrap(), long, "64 characters, 128 bytes");
        assert!(matches!(
            name(&format!("{long}x")),
            Err(SnapshotError::BadName(_))
        ));
    }

    #[test]
    fn the_text_is_canonical_and_reads_back() {
        let mut snapshots = Snapshots::default();
        snapshots.snapshots.insert(
            "Verse A".to_owned(),
            Values::from([
                ("/out.gain".to_owned(), 0.25),
                ("/drums.kick_decay".to_owned(), 300.0),
                ("/a.freq".to_owned(), 0.1 + 0.2),
            ]),
        );
        snapshots.snapshots.insert(
            "chorus".to_owned(),
            Values::from([("/out.gain".to_owned(), -0.0)]),
        );
        let text = render(&snapshots);
        assert_eq!(
            text,
            format!(
                "{HEADER}schema_version = 1\n\n[snapshots.\"Verse A\"]\n\"/a.freq\" = 0.30000000000000004\n\
                 \"/drums.kick_decay\" = 300\n\"/out.gain\" = 0.25\n\n[snapshots.chorus]\n\"/out.gain\" = -0.0\n"
            )
        );
        let back = parse(&text).unwrap();
        assert_eq!(back, snapshots);
        let mut with_undo = snapshots.clone();
        with_undo.undo = Some(Values::from([("/out.gain".to_owned(), 0.5)]));
        let undone = render(&with_undo);
        assert!(
            undone.contains(
                "schema_version = 1\n\n[undo]\n\"/out.gain\" = 0.5\n\n[snapshots.\"Verse A\"]"
            ),
            "{undone}"
        );
        assert_eq!(parse(&undone).unwrap(), with_undo);
        assert_eq!(render(&parse(&undone).unwrap()), undone, "byte-stable");
        assert_eq!(render(&back), text, "byte-stable");
        assert_eq!(
            back.snapshots["chorus"]["/out.gain"].to_bits(),
            (-0.0f64).to_bits()
        );
    }

    #[test]
    fn bad_files_are_refused() {
        assert_eq!(parse("").unwrap(), Snapshots::default());
        assert_eq!(parse("# nothing yet\n").unwrap(), Snapshots::default());
        for bad in [
            "schema_version = 2\n",
            "schema_version = 1\nother = 1\n",
            "[snapshots.a]\n\"/out.gain\" = 1\n",
            "schema_version = 1\n[snapshots.a]\n\"out.gain\" = 1\n",
            "schema_version = 1\n[snapshots.a]\n\"/out.gain\" = \"loud\"\n",
            "schema_version = 1\n[snapshots.a]\n\"/out.gain\" = inf\n",
            "schema_version = 1\n[snapshots.\" a\"]\n\"/out.gain\" = 1\n",
            "schema_version = 1\n[snapshots.\"\"]\n\"/out.gain\" = 1\n",
            "schema_version = 1\nsnapshots = 3\n",
            "schema_version = [",
        ] {
            assert!(
                matches!(parse(bad), Err(SnapshotError::Invalid(_))),
                "{bad:?}"
            );
        }
    }
}
